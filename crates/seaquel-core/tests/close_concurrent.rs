//! A window's connections close side by side (the desktop DuckDB helper
//! plan, Task 4 review I2): `Workspace::close_owned_by` (a reloaded or
//! closed webview) doesn't wait for one connection's close before starting
//! the next, so one slow close (a DuckDB helper checkpointing) can't hold
//! the others open. A mock engine whose drivers' `close` waits until the
//! test lets it go stands in for SQLite.
#![cfg(all(feature = "workspace", feature = "storage"))]
// A test: tokio drives it, as in the other Core tests.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use seaquel_core::{
    ConnectPolicy, ConnectRequest, ConnectionForm, Core, WorkspaceSpec, WriteOrigin,
};
use seaquel_engine::{
    BoxStream, CancellationToken, ConnectConfig, DbError, Driver, Engine, ExecuteResult,
    QueryResult, StreamBatch, Value,
};
use serde_json::json;
use tokio::sync::watch;

/// Counts the closes that started and finished; each close waits until
/// `release` says so.
#[derive(Default)]
struct Closes {
    started: AtomicUsize,
    finished: AtomicUsize,
}

struct SlowDriver {
    closes: Arc<Closes>,
    release: watch::Receiver<bool>,
}

#[seaquel_runtime::async_trait]
impl Driver for SlowDriver {
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
        self.closes.started.fetch_add(1, Ordering::SeqCst);
        let mut release = self.release.clone();
        let _ = release.wait_for(|go| *go).await;
        self.closes.finished.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct SlowEngine {
    closes: Arc<Closes>,
    release: watch::Receiver<bool>,
}

#[seaquel_runtime::async_trait]
impl Engine for SlowEngine {
    fn id(&self) -> &'static str {
        "sqlite"
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        Ok(Arc::new(SlowDriver {
            closes: self.closes.clone(),
            release: self.release.clone(),
        }))
    }
}

fn form(name: &str) -> ConnectionForm {
    serde_json::from_value(json!({
        "name": name, "type": "sqlite", "databaseName": format!("/nowhere/{name}.db")
    }))
    .unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_windows_connections_close_side_by_side() {
    let closes = Arc::new(Closes::default());
    let (release, rx) = watch::channel(false);
    let core = Arc::new(
        Core::builder()
            .engine(Arc::new(SlowEngine {
                closes: closes.clone(),
                release: rx,
            }))
            .connect_policy(ConnectPolicy::Unrestricted)
            .executor(Arc::new(seaquel_runtime::TokioExecutor))
            .build(),
    );
    let dir = tempfile::tempdir().unwrap();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    for name in ["a", "b"] {
        ws.connect(
            &core,
            ConnectRequest::form(form(name)).with_origin(WriteOrigin::new(Some("main"))),
        )
        .await
        .unwrap();
    }

    let closing = {
        let (core, ws) = (core.clone(), ws.clone());
        tokio::spawn(async move { ws.close_owned_by(&core, "main").await })
    };
    // Both closes start while neither may finish: one close in flight
    // doesn't hold the other back.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while closes.started.load(Ordering::SeqCst) < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {} of 2 closes started while the first waits",
            closes.started.load(Ordering::SeqCst)
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(closes.finished.load(Ordering::SeqCst), 0);
    // Both are out of Core already.
    assert_eq!(core.connection_count(), 0);

    release.send_replace(true);
    let closed = tokio::time::timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(closed, 2);
    assert_eq!(closes.finished.load(Ordering::SeqCst), 2);
}
