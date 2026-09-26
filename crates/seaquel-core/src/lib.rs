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
    not_supported, BatchStatement, BoxStream, CancellationToken, ConnectConfig, ConnectResult,
    DatabaseStatistics, DbError, Dialect, Driver, Engine, EngineRegistry, ExecuteResult,
    ExplainResult, QueryResult, SchemaColumn, SchemaIndex, SchemaTable,
};
use seaquel_sql::read_only::read_only_error;
use seaquel_sql::SqlEngine;
pub use seaquel_types::{StreamEvent, Value};

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

/// An open connection: its driver, and the engine that opened it (for the
/// engine's dialect).
#[derive(Clone)]
struct Connection {
    engine: Arc<dyn Engine>,
    driver: Arc<dyn Driver>,
}

pub struct Core {
    engines: EngineRegistry,
    /// `Arc` (not `Box`) so a handle can be cloned out of the map and the lock
    /// released before awaiting the driver. A long-running stream must never
    /// hold this lock, or it would block every other caller — disconnect,
    /// new queries, other connections — until it finished.
    connections: RwLock<HashMap<String, Connection>>,
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

/// How [`Core::query_stream`] runs a query. Build it from
/// `QueryOptions::default()` and the `with_*` methods, so adding an option
/// later (a row limit) doesn't touch every caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct QueryOptions {
    /// Run through the AI's token check (`seaquel_sql::read_only`) and then
    /// [`Driver::query_read_only`], which the database enforces: the AI's
    /// `run_query` tool and dashboard widgets. Off by default (the editor).
    pub read_only: bool,
}

impl QueryOptions {
    #[must_use]
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }
}

/// The `seaquel_sql` engine whose token rules apply to a connection opened by
/// the engine with this id. No fallback: an id without rules is refused, not
/// checked under some other engine's rules. `mysql` also serves MariaDB,
/// whose `/*M!` comments the MySQL rules refuse.
fn sql_engine(engine_id: &str) -> Option<SqlEngine> {
    match engine_id {
        "postgres" => Some(SqlEngine::Postgres),
        "mysql" => Some(SqlEngine::Mysql),
        "sqlite" => Some(SqlEngine::Sqlite),
        "mssql" => Some(SqlEngine::Mssql),
        "duckdb" => Some(SqlEngine::Duckdb),
        _ => None,
    }
}

/// Fix 14's token check for a read-only query on an engine, as
/// [`DbError::read_only`] with the exact text the TS shows.
fn check_read_only(sql: &str, engine_id: &str) -> Result<(), DbError> {
    let refusal = match sql_engine(engine_id) {
        Some(engine) => read_only_error(sql, engine),
        None => Some(seaquel_sql::read_only::READ_ONLY_MESSAGE),
    };
    refusal.map_or(Ok(()), |message| Err(DbError::read_only(message)))
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

        let engine = self
            .engines
            .get(driver_name)
            .ok_or_else(|| DbError::engine_not_available(driver_name))?;
        let driver = engine.open(config).await?;
        let connection_id = format!("{}-{}", driver_name, uuid::Uuid::new_v4());
        self.connections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(connection_id.clone(), Connection { engine, driver });

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
        let connection = self
            .connections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(connection_id);
        if let Some(Connection { driver, .. }) = connection {
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

    /// A clone of the connection's handles, so the lock is released before
    /// anything is awaited.
    fn connection(&self, connection_id: &str) -> Result<Connection, DbError> {
        self.connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(connection_id)
            .cloned()
            .ok_or_else(|| DbError::connection_not_found(connection_id))
    }

    fn driver(&self, connection_id: &str) -> Result<Arc<dyn Driver>, DbError> {
        Ok(self.connection(connection_id)?.driver)
    }

    /// The engine that opened a connection.
    pub fn engine(&self, connection_id: &str) -> Result<Arc<dyn Engine>, DbError> {
        Ok(self.connection(connection_id)?.engine)
    }

    /// Run `f` with the connection's SQL dialect. `NOT_SUPPORTED` when the
    /// engine's dialect still lives in TypeScript.
    ///
    /// A closure rather than a returned `&dyn Dialect`: the dialect borrows
    /// from the engine, which is only reachable through a cloned `Arc` once
    /// the connections lock is released. Dialects are pure, so `f` is sync.
    pub fn with_dialect<R>(
        &self,
        connection_id: &str,
        f: impl FnOnce(&dyn Dialect) -> R,
    ) -> Result<R, DbError> {
        let engine = self.engine(connection_id)?;
        let dialect = engine
            .dialect()
            .ok_or_else(|| not_supported("The Rust SQL dialect"))?;
        Ok(f(dialect))
    }

    pub async fn query(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<QueryResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.query", connection_id = connection_id, keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Query");
        self.driver(connection_id)?.query(sql, params).await
    }

    pub async fn execute(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
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

    // ── Introspection ──

    pub async fn list_schemas(&self, connection_id: &str) -> Result<Vec<String>, DbError> {
        debug!(activity = "db.list_schemas", connection_id = connection_id; "List schemas");
        self.driver(connection_id)?.list_schemas().await
    }

    pub async fn schema_tables(&self, connection_id: &str) -> Result<Vec<SchemaTable>, DbError> {
        debug!(activity = "db.schema_tables", connection_id = connection_id; "Schema tables");
        self.driver(connection_id)?.schema_tables().await
    }

    pub async fn table_metadata(
        &self,
        connection_id: &str,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        debug!(activity = "db.table_metadata", connection_id = connection_id, schema = schema, table = table; "Table metadata");
        self.driver(connection_id)?
            .table_metadata(schema, table)
            .await
    }

    pub async fn statistics(&self, connection_id: &str) -> Result<DatabaseStatistics, DbError> {
        debug!(activity = "db.statistics", connection_id = connection_id; "Statistics");
        self.driver(connection_id)?.statistics().await
    }

    pub async fn explain(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.explain", connection_id = connection_id, keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(), analyze = analyze; "Explain");
        self.driver(connection_id)?
            .explain(sql, params, analyze)
            .await
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
    ///
    /// With [`QueryOptions::read_only`], the SQL first goes through the AI's
    /// token check for the connection's engine; a refusal ends the stream
    /// with `READ_ONLY` and never reaches the driver. Then the driver's
    /// [`Driver::query_read_only`] runs, under the same cancellation, and
    /// its result is one final `Batch` and `Done`.
    pub fn query_stream(
        &self,
        query_id: String,
        connection_id: String,
        sql: String,
        params: Vec<Value>,
        options: QueryOptions,
    ) -> BoxStream<'_, StreamEvent> {
        let keyword = sql_keyword(&sql);
        debug!(activity = "db.query_stream", query_id = query_id.as_str(), connection_id = connection_id.as_str(), keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(), read_only = options.read_only; "Query stream");

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
            let connection = match self.connection(&connection_id) {
                Ok(connection) => connection,
                Err(e) => {
                    yield StreamEvent::from(e);
                    return;
                }
            };
            if options.read_only {
                // The check runs before the driver is touched.
                if let Err(e) = check_read_only(&sql, connection.engine.id()) {
                    yield StreamEvent::from(e);
                    return;
                }
                // Dropping the driver's future is how a read-only query is
                // cancelled: `take_until` drops it on cancel or disconnect,
                // and dropping this stream drops it too.
                let mut result = std::pin::pin!(futures::stream::once(
                    connection.driver.query_read_only(&sql, params)
                )
                .take_until(token.cancelled()));
                match result.next().await {
                    _ if token.is_cancelled() => {
                        if closed.load(Ordering::SeqCst) {
                            yield connection_closed();
                        }
                    }
                    Some(Ok(result)) => {
                        yield StreamEvent::Batch(seaquel_engine::StreamBatch {
                            columns: Some(result.columns),
                            rows: result.rows,
                            is_final: true,
                        });
                        yield StreamEvent::Done;
                    }
                    Some(Err(e)) => yield StreamEvent::from(e),
                    // `take_until` ends without an item only on cancel.
                    None => {}
                }
                return;
            }
            let driver = connection.driver;
            // `take_until` ends the stream on cancel even while the driver is
            // awaiting a query it can't interrupt (the default, non-streaming
            // `query_stream`, e.g. MSSQL); the driver's stream is dropped when
            // this ends. Drivers that run blocking work elsewhere (DuckDB, on
            // `spawn_blocking`) interrupt the query when their stream drops.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_id_has_token_rules() {
        for (id, engine) in [
            ("postgres", SqlEngine::Postgres),
            ("mysql", SqlEngine::Mysql),
            ("sqlite", SqlEngine::Sqlite),
            ("mssql", SqlEngine::Mssql),
            ("duckdb", SqlEngine::Duckdb),
        ] {
            assert_eq!(sql_engine(id), Some(engine), "{id}");
        }
        // Every engine this build can open is covered.
        for id in with_default_plugins().build().engine_ids() {
            assert!(sql_engine(id).is_some(), "{id} has no token rules");
        }
    }

    #[test]
    fn an_unknown_engine_id_is_refused_without_a_fallback() {
        for id in ["", "oracle", "mariadb", "Postgres"] {
            assert_eq!(sql_engine(id), None, "{id}");
            let err = check_read_only("SELECT 1", id).unwrap_err();
            assert_eq!(err.code, "READ_ONLY");
            assert_eq!(err.message, seaquel_sql::read_only::READ_ONLY_MESSAGE);
        }
        assert!(check_read_only("SELECT 1", "postgres").is_ok());
    }
}
