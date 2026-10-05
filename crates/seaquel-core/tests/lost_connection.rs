//! A connection lost without being asked to close:
//! a driver whose `Driver::closed()` resolves with
//! an error is taken out of Core and announced once as `ConnectionClosed`
//! with `CONNECTION_CLOSED` and the driver's message. One that ends as
//! asked (a disconnect, a close) announces nothing. A mock engine stands in
//! for SQLite, so nothing here needs a database; `duckdb_remote.rs` runs
//! the same through a real helper.
#![cfg(all(feature = "workspace", feature = "storage"))]

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::{
    ConnectPolicy, ConnectRequest, ConnectionForm, Core, Workspace, WorkspaceEvent, WorkspaceSpec,
    CONNECTION_CLOSED,
};
use seaquel_engine::{
    BoxStream, CancellationToken, ConnectConfig, DbError, Driver, Engine, ExecuteResult,
    QueryResult, StreamBatch, Value,
};
use seaquel_runtime::BoxFuture;
use serde_json::json;
use tokio::sync::watch;

/// `None` while open; `Some(None)` ended as asked; `Some(Some(e))` lost.
type Ending = Option<Option<DbError>>;

struct MockDriver {
    ending: Arc<watch::Sender<Ending>>,
}

#[seaquel_runtime::async_trait]
impl Driver for MockDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        Ok(QueryResult {
            columns: vec!["a".into()],
            rows: vec![vec![Value::Int(1)]],
        })
    }

    async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        unimplemented!()
    }

    fn query_stream<'a>(
        &'a self,
        _sql: String,
        _params: Vec<Value>,
        _cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        Box::pin(futures::stream::empty())
    }

    async fn close(&self) -> Result<(), DbError> {
        self.ending.send_replace(Some(None));
        Ok(())
    }

    fn closed(&self) -> Option<BoxFuture<'static, Option<DbError>>> {
        let mut rx = self.ending.subscribe();
        Some(Box::pin(async move {
            match rx.wait_for(Option::is_some).await {
                Ok(ending) => ending.as_ref().and_then(|e| e.as_ref()).map(|e| DbError {
                    code: e.code.clone(),
                    message: e.message.clone(),
                }),
                // The driver was dropped: ended as asked.
                Err(_) => None,
            }
        }))
    }
}

/// Opens [`MockDriver`]s as `sqlite` and keeps a handle on each one's end.
#[derive(Default)]
struct MockEngine {
    opened: Mutex<Vec<Weak<watch::Sender<Ending>>>>,
}

impl MockEngine {
    /// Loses the `n`th connection opened, as a helper killed by a signal.
    fn lose(&self, n: usize) {
        let sender = self.opened.lock().unwrap()[n].upgrade().expect("open");
        sender.send_replace(Some(Some(DbError {
            code: "CONNECTION_CLOSED".into(),
            message: "The DuckDB helper stopped (signal 9). Reconnect to continue.".into(),
        })));
    }
}

#[seaquel_runtime::async_trait]
impl Engine for MockEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        let ending = Arc::new(watch::channel(None).0);
        self.opened.lock().unwrap().push(Arc::downgrade(&ending));
        Ok(Arc::new(MockDriver { ending }))
    }
}

/// So the engine stays reachable from the test while Core owns it.
struct Shared(Arc<MockEngine>);

#[seaquel_runtime::async_trait]
impl Engine for Shared {
    fn id(&self) -> &'static str {
        self.0.id()
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        self.0.open(config).await
    }
}

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    engine: Arc<MockEngine>,
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let engine = Arc::new(MockEngine::default());
    let dir = tempfile::tempdir().unwrap();
    let core = Core::builder()
        .engine(Arc::new(Shared(engine.clone())))
        .connect_policy(ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    Env {
        core,
        ws,
        engine,
        _dir: dir,
    }
}

impl Env {
    async fn connect(&self) -> String {
        let form: ConnectionForm = serde_json::from_value(json!({
            "name": "mock", "type": "sqlite", "databaseName": "/nowhere/mock.db"
        }))
        .unwrap();
        self.ws
            .connect(&self.core, ConnectRequest::form(form))
            .await
            .unwrap()
    }
}

/// The `ConnectionClosed` events that arrive within `within`.
async fn closed_events(
    events: &mut BoxStream<'static, WorkspaceEvent>,
    within: Duration,
) -> Vec<(String, String, String)> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let WorkspaceEvent::ConnectionClosed {
            connection_id,
            code,
            message,
        } = event
        {
            seen.push((connection_id, code, message));
        }
    }
    seen
}

#[tokio::test]
async fn a_lost_connection_is_taken_out_and_announced_once() {
    let env = env().await;
    let mut events = env.ws.events();
    let lost = env.connect().await;
    let kept = env.connect().await;
    env.engine.lose(0);
    let seen = closed_events(&mut events, Duration::from_millis(500)).await;
    assert_eq!(
        seen,
        vec![(
            lost.clone(),
            CONNECTION_CLOSED.to_string(),
            "The DuckDB helper stopped (signal 9). Reconnect to continue.".to_string()
        )]
    );
    assert_eq!(
        env.ws.alive(&env.core, &[lost.clone(), kept.clone()]),
        vec![kept.clone()]
    );
    let e = env
        .ws
        .query(&env.core, &lost, "SELECT 1", vec![])
        .await
        .unwrap_err();
    assert_eq!(e.code, "CONNECTION_NOT_FOUND", "{e:?}");
    // The other connection goes on.
    env.ws
        .query(&env.core, &kept, "SELECT 1", vec![])
        .await
        .unwrap();
    // A disconnect of the lost one now is an unknown id, and announces
    // nothing more.
    let _ = env.ws.disconnect(&env.core, &lost).await;
    assert!(closed_events(&mut events, Duration::from_millis(200))
        .await
        .is_empty());
}

#[tokio::test]
async fn a_disconnect_announces_nothing() {
    let env = env().await;
    let mut events = env.ws.events();
    let id = env.connect().await;
    env.ws.disconnect(&env.core, &id).await.unwrap();
    assert!(closed_events(&mut events, Duration::from_millis(300))
        .await
        .is_empty());
    // Losing it afterwards (its driver is gone) can't announce it either.
    assert!(env.engine.opened.lock().unwrap()[0].upgrade().is_none());
}

/// A connection no workspace owns (`Core::connect`, the engine tests) is
/// taken out too, with no one to tell.
#[tokio::test]
async fn a_lost_connection_of_no_workspace_is_taken_out() {
    let env = env().await;
    let config: ConnectConfig =
        serde_json::from_value(json!({"driver": "sqlite", "path": "/nowhere/mock.db"})).unwrap();
    let id = env.core.connect(&config).await.unwrap().connection_id;
    assert_eq!(env.core.connection_count(), 1);
    env.engine.lose(0);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while env.core.connection_count() > 0 {
        assert!(tokio::time::Instant::now() < deadline, "{id} stayed open");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
