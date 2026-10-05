//! A window's reconnect to a file one connection holds exclusively (the
//! desktop DuckDB helper plan, Decision 21): the older connection is closed
//! only once nothing can refuse the new one before it opens (the engine,
//! the connect policy, `Engine::preflight`), and it is announced
//! `CONNECTION_REPLACED` before its close is awaited, so a reconnect
//! dropped mid-close still announces it. A mock engine stands in for the
//! remote DuckDB one, so nothing here needs a helper.
#![cfg(all(feature = "workspace", feature = "storage"))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::{
    ConnectPolicy, ConnectRequest, ConnectionForm, Core, Workspace, WorkspaceEvent, WorkspaceSpec,
    WriteOrigin,
};
use seaquel_engine::{
    BoxStream, CancellationToken, ConnectConfig, DbError, Driver, Engine, ExecuteResult,
    QueryResult, StreamBatch, Value,
};
use serde_json::json;

#[derive(Default)]
struct Flags {
    /// The connect policy refuses.
    policy_refuses: AtomicBool,
    /// `Engine::preflight` refuses.
    preflight_refuses: AtomicBool,
    /// `close` never returns.
    close_hangs: AtomicBool,
}

struct MockDriver {
    flags: Arc<Flags>,
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
        if self.flags.close_hangs.load(Ordering::SeqCst) {
            futures::future::pending::<()>().await;
        }
        Ok(())
    }
}

/// `sqlite`, holding its file exclusively as the remote DuckDB engine does.
struct ExclusiveEngine {
    flags: Arc<Flags>,
}

#[seaquel_runtime::async_trait]
impl Engine for ExclusiveEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(MockDriver {
            flags: self.flags.clone(),
        }))
    }

    fn preflight(&self, _config: &ConnectConfig) -> Result<(), DbError> {
        if self.flags.preflight_refuses.load(Ordering::SeqCst) {
            return Err(DbError {
                code: "ENGINE_NOT_INSTALLED".into(),
                message: "not installed".into(),
            });
        }
        Ok(())
    }

    fn exclusive_file(&self, _config: &ConnectConfig) -> bool {
        true
    }
}

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    flags: Arc<Flags>,
    _dir: tempfile::TempDir,
}

async fn env() -> Env {
    let flags = Arc::new(Flags::default());
    let dir = tempfile::tempdir().unwrap();
    let policy_flags = flags.clone();
    let core = Core::builder()
        .engine(Arc::new(ExclusiveEngine {
            flags: flags.clone(),
        }))
        .connect_policy(ConnectPolicy::checked(
            move |_| {
                if policy_flags.policy_refuses.load(Ordering::SeqCst) {
                    Err(DbError {
                        code: "CONNECTION_OPTION_NOT_ALLOWED".into(),
                        message: "refused".into(),
                    })
                } else {
                    Ok(())
                }
            },
            false,
        ))
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    Env {
        core,
        ws,
        flags,
        _dir: dir,
    }
}

impl Env {
    /// The saved connection `conn-saved`, from window `win-1`.
    async fn connect(&self) -> Result<String, seaquel_core::CoreError> {
        let form: ConnectionForm = serde_json::from_value(json!({
            "name": "file", "type": "sqlite", "databaseName": "/nowhere/file.db"
        }))
        .unwrap();
        self.ws
            .connect(
                &self.core,
                ConnectRequest::form(form)
                    .with_saved_connection_id(Some("conn-saved".to_string()))
                    .with_origin(WriteOrigin::new(Some("win-1"))),
            )
            .await
    }
}

/// The `(connection id, code)` of each `ConnectionClosed` within `within`.
async fn closed_events(
    events: &mut BoxStream<'static, WorkspaceEvent>,
    within: Duration,
) -> Vec<(String, String)> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + within;
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let WorkspaceEvent::ConnectionClosed {
            connection_id,
            code,
            ..
        } = event
        {
            seen.push((connection_id, code));
        }
    }
    seen
}

#[tokio::test]
async fn a_reconnect_the_policy_refuses_keeps_the_old_connection() {
    let env = env().await;
    let mut events = env.ws.events();
    let old = env.connect().await.unwrap();
    env.flags.policy_refuses.store(true, Ordering::SeqCst);
    let e = env.connect().await.unwrap_err();
    assert_eq!(e.code, "CONNECTION_OPTION_NOT_ALLOWED", "{e:?}");
    assert_eq!(env.ws.connection_ids(&env.core), vec![old]);
    assert!(closed_events(&mut events, Duration::from_millis(200))
        .await
        .is_empty());
}

#[tokio::test]
async fn a_reconnect_the_engine_refuses_up_front_keeps_the_old_connection() {
    let env = env().await;
    let mut events = env.ws.events();
    let old = env.connect().await.unwrap();
    env.flags.preflight_refuses.store(true, Ordering::SeqCst);
    let e = env.connect().await.unwrap_err();
    assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e:?}");
    assert_eq!(env.ws.connection_ids(&env.core), vec![old]);
    assert!(closed_events(&mut events, Duration::from_millis(200))
        .await
        .is_empty());
}

#[tokio::test]
async fn a_reconnect_replaces_the_old_connection_before_opening() {
    let env = env().await;
    let mut events = env.ws.events();
    let old = env.connect().await.unwrap();
    let new = env.connect().await.unwrap();
    assert_eq!(
        closed_events(&mut events, Duration::from_millis(200)).await,
        vec![(old, "CONNECTION_REPLACED".to_string())]
    );
    assert_eq!(env.ws.connection_ids(&env.core), vec![new]);
}

/// Review follow-up 2: the old connection's close never returns and the
/// reconnect is dropped while it waits: the old connection was still taken
/// out and announced.
#[tokio::test]
async fn a_reconnect_dropped_mid_close_still_announces_the_replaced_connection() {
    let env = env().await;
    let mut events = env.ws.events();
    let old = env.connect().await.unwrap();
    env.flags.close_hangs.store(true, Ordering::SeqCst);
    let dropped = tokio::time::timeout(Duration::from_millis(300), env.connect()).await;
    assert!(dropped.is_err(), "the reconnect didn't wait on the close");
    assert_eq!(
        closed_events(&mut events, Duration::from_millis(200)).await,
        vec![(old.clone(), "CONNECTION_REPLACED".to_string())]
    );
    assert!(env.ws.alive(&env.core, &[old]).is_empty());
}
