//! Seaquel Core.
//!
//! Interfaces (the Tauri app, `seaquel-server`, and later the CLI, TUI and MCP
//! server) do database work only through [`Core`]. It owns the engine
//! registry, the open connections, and the cancellation tokens of running
//! streams. It grows into the full Core from
//! `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` over the
//! following phases.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use futures::StreamExt;
use log::{debug, info};
use seaquel_engine::{
    BatchStatement, BoxStream, CancellationToken, ConnectConfig, ConnectResult, DbError, Driver,
    Engine, EngineRegistry, ExecuteResult, QueryResult,
};
pub use seaquel_types::StreamEvent;

type StreamTokens = Mutex<HashMap<String, StreamEntry>>;

/// A running stream's entry in [`Core::streams`].
struct StreamEntry {
    /// Tells apart two streams that were given the same query id.
    id: u64,
    /// So `disconnect` can cancel the connection's streams.
    connection_id: String,
    token: CancellationToken,
    /// Set by `disconnect` before it cancels `token`, so the stream reports
    /// the closed connection instead of ending silently like a client cancel.
    closed: Arc<AtomicBool>,
}

pub struct Core {
    engines: EngineRegistry,
    /// `Arc` (not `Box`) so a handle can be cloned out of the map and the lock
    /// released before awaiting the driver. A long-running stream must never
    /// hold this lock, or it would block every other caller — disconnect,
    /// new queries, other connections — until it finished.
    connections: RwLock<HashMap<String, Arc<dyn Driver>>>,
    /// Cancellation tokens of running streams, keyed by the client's query id.
    streams: StreamTokens,
    next_stream: AtomicU64,
}

#[derive(Default)]
pub struct CoreBuilder {
    engines: EngineRegistry,
}

impl CoreBuilder {
    pub fn engine(mut self, engine: Arc<dyn Engine>) -> Self {
        self.engines.register(engine);
        self
    }

    pub fn build(self) -> Core {
        Core {
            engines: self.engines,
            connections: RwLock::default(),
            streams: Mutex::default(),
            next_stream: AtomicU64::new(0),
        }
    }
}

/// A builder with every plugin this build's Cargo features enable.
pub fn with_default_plugins() -> CoreBuilder {
    #[allow(unused_mut)]
    let mut builder = Core::builder();
    #[cfg(feature = "engine-postgres")]
    {
        builder = builder.engine(seaquel_engine_postgres::engine());
    }
    #[cfg(feature = "engine-mysql")]
    {
        builder = builder.engine(seaquel_engine_mysql::engine());
    }
    #[cfg(feature = "engine-sqlite")]
    {
        builder = builder.engine(seaquel_engine_sqlite::engine());
    }
    #[cfg(feature = "engine-mssql")]
    {
        builder = builder.engine(seaquel_engine_mssql::engine());
    }
    #[cfg(feature = "engine-duckdb")]
    {
        builder = builder.engine(seaquel_engine_duckdb::engine());
    }
    builder
}

/// The terminal event of a stream whose connection `disconnect` closed.
fn connection_closed() -> StreamEvent {
    StreamEvent::Error {
        message: "Connection was closed while the query was running".to_string(),
        code: "CONNECTION_CLOSED".to_string(),
    }
}

fn sql_keyword(sql: &str) -> String {
    sql.split_whitespace().next().unwrap_or("?").to_uppercase()
}

impl Core {
    pub fn builder() -> CoreBuilder {
        CoreBuilder::default()
    }

    /// Ids of the engines in this build, sorted.
    pub fn engine_ids(&self) -> Vec<&'static str> {
        self.engines.ids()
    }

    pub async fn connect(&self, config: &ConnectConfig) -> Result<ConnectResult, DbError> {
        let driver_name = config.driver.as_str();
        info!(activity = "db.connect", driver = driver_name; "Connecting");

        let driver = self.engines.open(config).await?;
        let connection_id = format!("{}-{}", driver_name, uuid::Uuid::new_v4());
        self.connections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(connection_id.clone(), driver);

        info!(activity = "db.connect", driver = driver_name, connection_id = connection_id.as_str(); "Connected");
        Ok(ConnectResult { connection_id })
    }

    /// Open and close a connection without registering it ("Test connection").
    pub async fn test(&self, config: &ConnectConfig) -> Result<(), DbError> {
        debug!(activity = "db.test", driver = config.driver.as_str(); "Testing connection");
        let driver = self.engines.open(config).await?;
        driver.close().await
    }

    /// Close a connection. Idempotent: an unknown id succeeds.
    ///
    /// Cancels the connection's running streams first, since the driver's
    /// `close()` waits for them to hand back their pooled connections. Each
    /// ends with a `CONNECTION_CLOSED` error so clients stop waiting. A
    /// cancelled stream only lets go of its connection when it is next polled
    /// or dropped, so a caller holding a stream it never polls again delays
    /// this until it drops that stream.
    ///
    /// Only the newest stream under each query id is tracked, so if a client
    /// reused a query id, an older stream with it is not cancelled here (it
    /// still stops when dropped).
    pub async fn disconnect(&self, connection_id: &str) -> Result<(), DbError> {
        info!(activity = "db.disconnect", connection_id = connection_id; "Disconnecting");
        let driver = self
            .connections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(connection_id);
        if let Some(driver) = driver {
            self.cancel_streams_of(connection_id);
            // Waits for in-flight queries to return their pooled connections.
            // Cancelled streams return theirs as soon as they are polled.
            driver.close().await?;
        }
        Ok(())
    }

    pub fn connection_count(&self) -> usize {
        self.connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn driver(&self, connection_id: &str) -> Result<Arc<dyn Driver>, DbError> {
        self.connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(connection_id)
            .cloned()
            .ok_or_else(|| DbError::connection_not_found(connection_id))
    }

    pub async fn query(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<QueryResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.query", connection_id = connection_id, keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Query");
        self.driver(connection_id)?.query(sql, params).await
    }

    pub async fn execute(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<serde_json::Value>,
    ) -> Result<ExecuteResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.execute", connection_id = connection_id, keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Execute");
        self.driver(connection_id)?.execute(sql, params).await
    }

    pub async fn transaction(
        &self,
        connection_id: &str,
        statements: Vec<BatchStatement>,
    ) -> Result<(), DbError> {
        debug!(activity = "db.transaction", connection_id = connection_id, statements = statements.len(); "Executing transaction");
        self.driver(connection_id)?.transaction(statements).await
    }

    /// Run a query and deliver its results as client events: zero or more
    /// `Batch` events, then exactly one `Done` or `Error`.
    ///
    /// After [`Core::cancel_stream`] with the same `query_id`, or when the
    /// returned stream is dropped, the driver stops fetching and the stream
    /// ends with no terminal event. After [`Core::disconnect`] of its
    /// connection it stops too, but ends with a `CONNECTION_CLOSED` error: the
    /// client didn't ask for that and would otherwise wait for a terminal
    /// event forever.
    pub fn query_stream(
        &self,
        query_id: String,
        connection_id: String,
        sql: String,
        params: Vec<serde_json::Value>,
    ) -> BoxStream<'_, StreamEvent> {
        let keyword = sql_keyword(&sql);
        debug!(activity = "db.query_stream", query_id = query_id.as_str(), connection_id = connection_id.as_str(), keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Query stream");

        // Registered now, not on first poll, so a cancel that arrives before
        // the stream starts still counts.
        let (token, closed, guard) = self.register_stream(query_id, connection_id.clone());
        Box::pin(async_stream::stream! {
            let _guard = guard;
            // Cancelled before the first poll, e.g. by `disconnect`.
            if token.is_cancelled() {
                if closed.load(Ordering::SeqCst) {
                    yield connection_closed();
                }
                return;
            }
            let driver = match self.driver(&connection_id) {
                Ok(driver) => driver,
                Err(e) => {
                    yield StreamEvent::from(e);
                    return;
                }
            };
            // `take_until` ends the stream on cancel even while the driver is
            // awaiting a query it can't interrupt (the default, non-streaming
            // `query_stream`, e.g. MSSQL); the driver's stream is dropped when
            // this ends. It can't help a driver that blocks the thread instead
            // of awaiting (DuckDB today): that query runs to completion first.
            let mut batches = std::pin::pin!(driver
                .query_stream(sql, params, token.clone())
                .take_until(token.cancelled()));
            while let Some(item) = batches.next().await {
                if token.is_cancelled() {
                    break;
                }
                match item {
                    Ok(batch) => yield StreamEvent::Batch(batch),
                    Err(e) => {
                        yield StreamEvent::from(e);
                        return;
                    }
                }
            }
            if !token.is_cancelled() {
                yield StreamEvent::Done;
            } else if closed.load(Ordering::SeqCst) {
                yield connection_closed();
            }
        })
    }

    /// Cancel a running stream. Unknown or finished query ids are ignored.
    ///
    /// If a query id is reused while an earlier stream with it still runs,
    /// only the newest one can be cancelled by id; the older ones still stop
    /// when dropped.
    pub fn cancel_stream(&self, query_id: &str) {
        debug!(activity = "db.cancel_stream", query_id = query_id; "Cancel stream");
        let token = self
            .streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(query_id)
            .map(|entry| entry.token.clone());
        // Cancelled outside the lock: `cancel()` runs wakers.
        if let Some(token) = token {
            token.cancel();
        }
    }

    /// Cancel every running stream on one connection.
    fn cancel_streams_of(&self, connection_id: &str) {
        let tokens: Vec<CancellationToken> = self
            .streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|entry| entry.connection_id == connection_id)
            .map(|entry| {
                // Before the cancel, so the woken stream sees it.
                entry.closed.store(true, Ordering::SeqCst);
                entry.token.clone()
            })
            .collect();
        for token in tokens {
            token.cancel();
        }
    }

    pub fn running_stream_count(&self) -> usize {
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn register_stream(
        &self,
        query_id: String,
        connection_id: String,
    ) -> (CancellationToken, Arc<AtomicBool>, StreamGuard<'_>) {
        let token = CancellationToken::new();
        let closed = Arc::new(AtomicBool::new(false));
        let id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let entry = StreamEntry {
            id,
            connection_id,
            token: token.clone(),
            closed: closed.clone(),
        };
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(query_id.clone(), entry);
        let guard = StreamGuard {
            streams: &self.streams,
            query_id,
            id,
        };
        (token, closed, guard)
    }
}

/// Removes a stream's cancellation token when the stream finishes or is
/// dropped. It only removes its own entry: if a newer stream reused the query
/// id, that one stays cancellable.
struct StreamGuard<'a> {
    streams: &'a StreamTokens,
    query_id: String,
    id: u64,
}

impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        if streams
            .get(&self.query_id)
            .is_some_and(|entry| entry.id == self.id)
        {
            streams.remove(&self.query_id);
        }
    }
}
