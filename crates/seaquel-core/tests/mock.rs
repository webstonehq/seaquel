//! Core's stream lifecycle against mock drivers, so no database is needed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::{Core, StreamEvent};
use seaquel_engine::{
    BoxStream, CancellationToken, ConnectConfig, DbError, Driver, Engine, ExecuteResult,
    QueryResult, StreamBatch,
};
use serde_json::json;
use tokio::sync::Notify;
use tokio::time::timeout;

const LIMIT: Duration = Duration::from_secs(5);

/// How [`MockDriver::query_stream`] behaves.
#[derive(Clone, Copy)]
enum Mode {
    /// The default (non-streaming) `query_stream`, over a `query()` that never
    /// returns — like a slow MSSQL query.
    HangingQuery,
    /// Yield one batch, then wait for the cancel token, like the sqlx drivers.
    /// `close()` waits until the stream is dropped, like `pool.close()`
    /// waiting for a checked-out connection.
    HoldsConnection,
    /// Yield one batch, then an error, then a batch that must never be seen.
    FailsMidStream,
}

struct MockDriver {
    mode: Mode,
    /// Set when the stream holding the "pooled connection" is dropped.
    released: AtomicBool,
    release: Notify,
}

impl MockDriver {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            released: AtomicBool::new(false),
            release: Notify::new(),
        })
    }
}

/// Marks the driver's connection as released when dropped.
struct Checkout<'a>(&'a MockDriver);

impl Drop for Checkout<'_> {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::SeqCst);
        self.0.release.notify_waiters();
    }
}

fn batch(n: i64) -> StreamBatch {
    StreamBatch {
        columns: Some(vec!["n".into()]),
        rows: vec![vec![json!(n)]],
        is_final: false,
    }
}

#[seaquel_runtime::async_trait]
impl Driver for MockDriver {
    async fn query(
        &self,
        _sql: &str,
        _params: Vec<serde_json::Value>,
    ) -> Result<QueryResult, DbError> {
        futures::future::pending().await
    }

    async fn execute(
        &self,
        _sql: &str,
        _params: Vec<serde_json::Value>,
    ) -> Result<ExecuteResult, DbError> {
        unimplemented!()
    }

    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<serde_json::Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        match self.mode {
            Mode::HangingQuery => {
                // The trait's default body, which can't be called from an override.
                Box::pin(async_stream::try_stream! {
                    let result = self.query(&sql, params).await?;
                    yield StreamBatch {
                        columns: Some(result.columns),
                        rows: result.rows,
                        is_final: true,
                    };
                })
            }
            Mode::HoldsConnection => Box::pin(async_stream::stream! {
                let _checkout = Checkout(self);
                yield Ok(batch(1));
                cancel.cancelled().await;
            }),
            Mode::FailsMidStream => Box::pin(futures::stream::iter([
                Ok(batch(1)),
                Err(DbError::query_error("boom")),
                Ok(batch(2)),
            ])),
        }
    }

    async fn close(&self) -> Result<(), DbError> {
        if let Mode::HoldsConnection = self.mode {
            loop {
                let released = self.release.notified();
                if self.released.load(Ordering::SeqCst) {
                    break;
                }
                released.await;
            }
        }
        Ok(())
    }
}

struct MockEngine(Arc<MockDriver>);

#[seaquel_runtime::async_trait]
impl Engine for MockEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(self.0.clone())
    }
}

async fn connect(mode: Mode) -> (Core, String) {
    let core = Core::builder()
        .engine(Arc::new(MockEngine(MockDriver::new(mode))))
        .build();
    let config: ConnectConfig = serde_json::from_value(json!({ "driver": "sqlite" })).unwrap();
    let id = core.connect(&config).await.unwrap().connection_id;
    (core, id)
}

fn summary(events: &[StreamEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|event| match event {
            StreamEvent::Batch(_) => "batch",
            StreamEvent::Done => "done",
            StreamEvent::Error { .. } => "error",
        })
        .collect()
}

/// A stream ended by `disconnect` tells the client, which would otherwise wait
/// for a terminal event forever.
fn assert_connection_closed(event: &StreamEvent) {
    match event {
        StreamEvent::Error { code, .. } => assert_eq!(code, "CONNECTION_CLOSED"),
        other => panic!("expected CONNECTION_CLOSED, got {other:?}"),
    }
}

#[tokio::test]
async fn cancel_interrupts_a_driver_that_does_not_stream() {
    let (core, id) = connect(Mode::HangingQuery).await;
    let stream = core.query_stream("q1".into(), id, "SELECT 1".into(), vec![]);

    let (events, ()) = timeout(LIMIT, async {
        // `join!` polls the consumer first, so the query is in flight when
        // the cancel arrives.
        tokio::join!(stream.collect::<Vec<_>>(), async {
            core.cancel_stream("q1");
        })
    })
    .await
    .expect("cancelled stream did not end");

    assert!(events.is_empty(), "{:?}", summary(&events));
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn disconnect_cancels_the_connections_streams() {
    let (core, id) = connect(Mode::HoldsConnection).await;
    let mut stream = core.query_stream("q1".into(), id.clone(), "SELECT 1".into(), vec![]);

    let (events, disconnected) = timeout(LIMIT, async {
        tokio::join!(
            async {
                let mut events = Vec::new();
                while let Some(event) = stream.next().await {
                    events.push(event);
                }
                events
            },
            core.disconnect(&id),
        )
    })
    .await
    .expect("disconnect waited on the running stream");

    disconnected.unwrap();
    assert_eq!(summary(&events), vec!["batch", "error"]);
    assert_connection_closed(&events[1]);
    assert_eq!(core.running_stream_count(), 0);
    assert_eq!(core.connection_count(), 0);
}

#[tokio::test]
async fn disconnect_leaves_other_connections_streams_running() {
    let (core, id) = connect(Mode::FailsMidStream).await;
    // A second connection, on the same mock engine.
    let config: ConnectConfig = serde_json::from_value(json!({ "driver": "sqlite" })).unwrap();
    let other = core.connect(&config).await.unwrap().connection_id;

    let stream = core.query_stream("q1".into(), other, "SELECT 1".into(), vec![]);
    core.disconnect(&id).await.unwrap();

    let events = timeout(LIMIT, stream.collect::<Vec<_>>()).await.unwrap();
    assert_eq!(summary(&events), vec!["batch", "error"]);
}

#[tokio::test]
async fn driver_error_mid_stream_ends_with_one_error() {
    let (core, id) = connect(Mode::FailsMidStream).await;
    let events = timeout(
        LIMIT,
        core.query_stream("q1".into(), id, "SELECT 1".into(), vec![])
            .collect::<Vec<_>>(),
    )
    .await
    .unwrap();

    assert_eq!(summary(&events), vec!["batch", "error"]);
    match &events[1] {
        StreamEvent::Error { message, .. } => assert!(message.contains("boom"), "{message}"),
        other => panic!("expected an error, got {other:?}"),
    }
    assert_eq!(core.running_stream_count(), 0);
}

#[tokio::test]
async fn a_stream_whose_connection_closed_before_its_first_poll_reports_it() {
    let (core, id) = connect(Mode::FailsMidStream).await;
    let stream = core.query_stream("q1".into(), id.clone(), "SELECT 1".into(), vec![]);
    core.disconnect(&id).await.unwrap();

    let events = timeout(LIMIT, stream.collect::<Vec<_>>()).await.unwrap();
    assert_eq!(summary(&events), vec!["error"]);
    assert_connection_closed(&events[0]);
    assert_eq!(core.running_stream_count(), 0);
}
