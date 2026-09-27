//! `ConnectionLimits`: the per-workspace connection cap (connects and tests
//! still in flight count, so concurrent calls can't pass it) and the pool
//! size Core hands the engine. A mock engine stands in, so nothing here
//! needs a database.
#![cfg(all(feature = "workspace", feature = "storage"))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use seaquel_core::{
    ConnectPolicy, ConnectRequest, ConnectionForm, ConnectionLimits, Core, Workspace,
    WorkspaceSpec, TOO_MANY_CONNECTIONS,
};
use seaquel_engine::{
    BoxStream, CancellationToken, ConnectConfig, DbError, Driver, Engine, ExecuteResult,
    OpenOptions, QueryResult, StreamBatch, Value,
};
use serde_json::json;
use tokio::sync::Semaphore;

struct MockDriver;

#[seaquel_runtime::async_trait]
impl Driver for MockDriver {
    async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
        unimplemented!()
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
        Ok(())
    }
}

/// Records the options of every open; each open waits for a permit from
/// `gate` when there is one, and fails while `fail` is set.
#[derive(Default)]
struct MockEngine {
    opens: Mutex<Vec<OpenOptions>>,
    gate: Option<Semaphore>,
    fail: AtomicBool,
}

#[seaquel_runtime::async_trait]
impl Engine for MockEngine {
    fn id(&self) -> &'static str {
        "postgres"
    }
    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        panic!("Core must call open_with");
    }
    async fn open_with(
        &self,
        _config: &ConnectConfig,
        options: OpenOptions,
    ) -> Result<Arc<dyn Driver>, DbError> {
        if let Some(gate) = &self.gate {
            gate.acquire().await.unwrap().forget();
        }
        if self.fail.load(Ordering::SeqCst) {
            return Err(DbError::connection_error("the mock refused"));
        }
        self.opens.lock().unwrap().push(options);
        Ok(Arc::new(MockDriver))
    }
}

struct Env {
    core: Core,
    engine: Arc<MockEngine>,
    dir: tempfile::TempDir,
}

impl Env {
    fn new(limits: Option<ConnectionLimits>, engine: MockEngine) -> Self {
        let engine = Arc::new(engine);
        let mut builder = Core::builder()
            .engine(engine.clone())
            .connect_policy(ConnectPolicy::Unrestricted);
        if let Some(limits) = limits {
            builder = builder.connection_limits(limits);
        }
        Self {
            core: builder.build(),
            engine,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    async fn workspace(&self, name: &str) -> Arc<Workspace> {
        self.core
            .open_workspace(WorkspaceSpec::new(self.dir.path().join(name)))
            .await
            .unwrap()
    }
}

fn form() -> ConnectionForm {
    serde_json::from_value(json!({
        "name": "f", "type": "postgres", "host": "db.example.com", "port": 5432,
        "databaseName": "app", "username": "u",
    }))
    .unwrap()
}

fn req() -> ConnectRequest {
    ConnectRequest::form(form())
}

fn per_workspace(cap: usize) -> ConnectionLimits {
    ConnectionLimits {
        per_workspace: Some(cap),
        max_pool_size: None,
    }
}

#[tokio::test]
async fn a_workspace_can_open_only_its_cap() {
    let env = Env::new(Some(per_workspace(2)), MockEngine::default());
    let a = env.workspace("a").await;
    let first = a.connect(&env.core, req()).await.unwrap();
    a.connect(&env.core, req()).await.unwrap();

    let err = a.connect(&env.core, req()).await.unwrap_err();
    assert_eq!(err.code, TOO_MANY_CONNECTIONS);
    assert!(err.message.contains('2'), "{}", err.message);
    // A test opens a connection too.
    let err = a.test(&env.core, req()).await.unwrap_err();
    assert_eq!(err.code, TOO_MANY_CONNECTIONS);
    // Nothing was opened for the refused calls.
    assert_eq!(env.engine.opens.lock().unwrap().len(), 2);

    // Another workspace has its own cap.
    let b = env.workspace("b").await;
    b.connect(&env.core, req()).await.unwrap();

    // Closing one frees a slot.
    a.disconnect(&env.core, &first).await.unwrap();
    a.connect(&env.core, req()).await.unwrap();
    assert_eq!(a.connection_ids(&env.core).len(), 2);
}

/// Connects still opening count: of five at once under a cap of 2, two
/// open and three are refused, whatever order they finish in.
#[tokio::test]
async fn connects_in_flight_count_toward_the_cap() {
    let engine = MockEngine {
        gate: Some(Semaphore::new(0)),
        ..MockEngine::default()
    };
    let env = Env::new(Some(per_workspace(2)), engine);
    let a = env.workspace("a").await;
    let calls = (0..5).map(|_| a.connect(&env.core, req()));
    let release = async {
        tokio::task::yield_now().await;
        // Only the two that got a slot ever wait for one.
        env.engine.gate.as_ref().unwrap().add_permits(2);
    };
    let (results, ()) = futures::join!(futures::future::join_all(calls), release);
    let opened = results.iter().filter(|r| r.is_ok()).count();
    let refused: Vec<_> = results
        .iter()
        .filter_map(|r| r.as_ref().err())
        .map(|e| e.code.as_str())
        .collect();
    assert_eq!(opened, 2);
    assert_eq!(refused, vec![TOO_MANY_CONNECTIONS; 3]);
    assert_eq!(a.connection_ids(&env.core).len(), 2);

    // A test in flight holds a slot as well.
    a.disconnect(&env.core, &a.connection_ids(&env.core)[0])
        .await
        .unwrap();
    let test = a.test(&env.core, req());
    let connect = async {
        tokio::task::yield_now().await;
        let r = a.connect(&env.core, req()).await;
        env.engine.gate.as_ref().unwrap().add_permits(1);
        r
    };
    let (tested, connected) = futures::join!(test, connect);
    assert_eq!(connected.unwrap_err().code, TOO_MANY_CONNECTIONS);
    tested.unwrap();
}

#[tokio::test]
async fn without_limits_nothing_is_capped() {
    let env = Env::new(None, MockEngine::default());
    let a = env.workspace("a").await;
    for _ in 0..40 {
        a.connect(&env.core, req()).await.unwrap();
    }
    assert!(env
        .engine
        .opens
        .lock()
        .unwrap()
        .iter()
        .all(|o| *o == OpenOptions::default()));
}

#[tokio::test]
async fn the_pool_size_reaches_the_engine() {
    let limits = ConnectionLimits {
        per_workspace: None,
        max_pool_size: Some(4),
    };
    let env = Env::new(Some(limits), MockEngine::default());
    let a = env.workspace("a").await;
    a.connect(&env.core, req()).await.unwrap();
    a.test(&env.core, req()).await.unwrap();
    let config: ConnectConfig = serde_json::from_value(json!({
        "driver": "postgres", "connection_string": "postgres://u@db.example.com/app"
    }))
    .unwrap();
    env.core.connect(&config).await.unwrap();
    env.core.test(&config).await.unwrap();
    let opens = env.engine.opens.lock().unwrap().clone();
    assert_eq!(opens.len(), 4);
    assert!(
        opens.iter().all(|o| o.max_pool_size == Some(4)),
        "{opens:?}"
    );
}

/// A connect that fails frees its slot.
#[tokio::test]
async fn a_failed_connect_frees_its_slot() {
    let env = Env::new(Some(per_workspace(1)), MockEngine::default());
    let a = env.workspace("a").await;
    env.engine.fail.store(true, Ordering::SeqCst);
    for _ in 0..3 {
        let err = a.connect(&env.core, req()).await.unwrap_err();
        assert_eq!(err.code, "CONNECTION_ERROR", "{}", err.message);
        let err = a.test(&env.core, req()).await.unwrap_err();
        assert_eq!(err.code, "CONNECTION_ERROR", "{}", err.message);
    }
    env.engine.fail.store(false, Ordering::SeqCst);
    a.connect(&env.core, req()).await.unwrap();
}

/// A connect whose future is dropped mid-open (a client gone, a request
/// timeout) frees its slot.
#[tokio::test]
async fn a_dropped_connect_frees_its_slot() {
    let engine = MockEngine {
        gate: Some(Semaphore::new(0)),
        ..MockEngine::default()
    };
    let env = Env::new(Some(per_workspace(1)), engine);
    let a = env.workspace("a").await;
    for _ in 0..3 {
        let pending = tokio::time::timeout(Duration::from_millis(20), a.connect(&env.core, req()));
        assert!(pending.await.is_err(), "the open should still be waiting");
        let pending = tokio::time::timeout(Duration::from_millis(20), a.test(&env.core, req()));
        assert!(pending.await.is_err(), "the open should still be waiting");
    }
    env.engine.gate.as_ref().unwrap().add_permits(1);
    a.connect(&env.core, req()).await.unwrap();
    assert_eq!(a.connection_ids(&env.core).len(), 1);
}
