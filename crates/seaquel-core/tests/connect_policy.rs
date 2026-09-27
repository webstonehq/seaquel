//! `ConnectPolicy`: a Core without one refuses every connect and test; a
//! `Checked` policy refuses SSH before any tunnel opens and checks the final
//! config. Also the workspace events: `close_all` announces failed closes,
//! and dropped receivers are pruned. A mock engine stands in for SQLite, so
//! nothing here needs a database.
#![cfg(all(
    feature = "workspace",
    feature = "ssh",
    feature = "storage",
    feature = "secrets"
))]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::secrets::{MemoryStore, SecretError, SecretStore};
use seaquel_core::{
    ConnectPolicy, ConnectRequest, ConnectionForm, Core, CoreBuilder, Workspace, WorkspaceEvent,
    WorkspaceSpec, WORKSPACE_EVICTED,
};
use seaquel_engine::{
    BoxStream, CancellationToken, ConnectConfig, DbError, Driver, Engine, ExecuteResult,
    QueryResult, StreamBatch, Value,
};
use serde_json::json;

struct MockDriver {
    fail_close: bool,
}

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
        if self.fail_close {
            Err(DbError::query_error("close failed"))
        } else {
            Ok(())
        }
    }
}

/// Answers for `id`; counts the drivers it opened.
struct MockEngine {
    id: &'static str,
    opened: Arc<AtomicUsize>,
    fail_close: bool,
}

#[seaquel_runtime::async_trait]
impl Engine for MockEngine {
    fn id(&self) -> &'static str {
        self.id
    }

    async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(MockDriver {
            fail_close: self.fail_close,
        }))
    }
}

/// A `MemoryStore` that counts reads.
struct CountingStore {
    inner: MemoryStore,
    reads: Arc<AtomicUsize>,
}

#[seaquel_runtime::async_trait]
impl SecretStore for CountingStore {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.inner.get(key).await
    }
    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.inner.set(key, value).await
    }
    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.inner.delete(key).await
    }
}

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    opened: Arc<AtomicUsize>,
    /// Secret-store reads.
    reads: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
}

async fn env(policy: Option<ConnectPolicy>, fail_close: bool) -> Env {
    let opened = Arc::new(AtomicUsize::new(0));
    let dir = tempfile::tempdir().unwrap();
    let mut builder: CoreBuilder = Core::builder().ssh_known_hosts(dir.path().join("known_hosts"));
    for id in ["sqlite", "postgres"] {
        builder = builder.engine(Arc::new(MockEngine {
            id,
            opened: opened.clone(),
            fail_close,
        }));
    }
    if let Some(policy) = policy {
        builder = builder.connect_policy(policy);
    }
    let core = builder.build();
    let reads = Arc::new(AtomicUsize::new(0));
    let store = CountingStore {
        inner: MemoryStore::new(),
        reads: reads.clone(),
    };
    store.inner.set("ssh:c-ssh", "ssh-pw").await.unwrap();
    store.inner.set("db:c-ssh", "db-pw").await.unwrap();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_secrets(Arc::new(store)))
        .await
        .unwrap();
    Env {
        core,
        ws,
        opened,
        reads,
        _dir: dir,
    }
}

fn form(fields: serde_json::Value) -> ConnectionForm {
    let mut form = json!({ "name": "f", "type": "sqlite", "databaseName": "/tmp/unused.db" });
    for (k, v) in fields.as_object().unwrap() {
        form[k] = v.clone();
    }
    serde_json::from_value(form).unwrap()
}

/// A Postgres form through an SSH server nobody answers at, with a key
/// file that doesn't exist: any attempt to open the tunnel would hang or
/// fail on the network or the file.
fn ssh_form() -> ConnectionForm {
    form(json!({
        "type": "postgres", "host": "db.internal", "port": 5432, "username": "u",
        "databaseName": "app", "sshEnabled": true, "sshHost": "10.255.255.1",
        "sshPort": 22, "sshUsername": "tunnel", "sshAuthMethod": "key",
        "sshKeyPath": "/nonexistent/seaquel-test-key",
    }))
}

fn sqlite_config() -> ConnectConfig {
    serde_json::from_value(json!({ "driver": "sqlite", "connection_string": "sqlite://x" }))
        .unwrap()
}

#[tokio::test]
async fn without_a_policy_nothing_connects() {
    let e = env(None, false).await;
    let refused = |code: &str| assert_eq!(code, "NOT_SUPPORTED");
    refused(&e.core.connect(&sqlite_config()).await.unwrap_err().code);
    refused(&e.core.test(&sqlite_config()).await.unwrap_err().code);
    for req in [
        ConnectRequest::form(form(json!({}))),
        ConnectRequest::form(ssh_form()),
        ConnectRequest::saved("c1"),
    ] {
        refused(&e.ws.connect(&e.core, req.clone()).await.unwrap_err().code);
        refused(&e.ws.test(&e.core, req).await.unwrap_err().code);
    }
    assert_eq!(e.opened.load(Ordering::SeqCst), 0);
    assert_eq!(e.core.connection_count(), 0);
    assert_eq!(e.core.ssh_tunnel_count(), 0);
}

#[tokio::test]
async fn unrestricted_connects() {
    let e = env(Some(ConnectPolicy::Unrestricted), false).await;
    let id =
        e.ws.connect(&e.core, ConnectRequest::form(form(json!({}))))
            .await
            .unwrap();
    e.ws.disconnect(&e.core, &id).await.unwrap();
    e.core.connect(&sqlite_config()).await.unwrap();
}

/// `allow_ssh: false` refuses before the tunnel opens: at once, with the
/// policy's error, not a network or key file error.
#[tokio::test]
async fn ssh_is_refused_before_the_tunnel_opens() {
    let e = env(Some(ConnectPolicy::checked(|_| Ok(()), false)), false).await;
    for test in [false, true] {
        let req = ConnectRequest::form(ssh_form());
        let call = async {
            if test {
                e.ws.test(&e.core, req).await
            } else {
                e.ws.connect(&e.core, req).await.map(|_| ())
            }
        };
        let err = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .expect("refused without trying the SSH server")
            .unwrap_err();
        assert_eq!(err.code, "NOT_SUPPORTED", "{err}");
        assert!(err.message.contains("SSH"), "{err}");
    }
    assert_eq!(e.core.ssh_tunnel_count(), 0);
    assert_eq!(e.opened.load(Ordering::SeqCst), 0);
}

/// The check runs on the finished config for connect and test (form and
/// Core's own), and its error comes back as it is, before the engine opens.
#[tokio::test]
async fn the_check_sees_the_final_config() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let policy = ConnectPolicy::checked(
        move |config: &ConnectConfig| {
            log.lock()
                .unwrap()
                .push(config.connection_string.clone().unwrap_or_default());
            Err(DbError {
                code: "CONNECTION_OPTION_NOT_ALLOWED".into(),
                message: "no".into(),
            })
        },
        false,
    );
    let e = env(Some(policy), false).await;
    let req = ConnectRequest::form(form(json!({ "databaseName": "/tmp/p.db" })));
    let err = e.ws.connect(&e.core, req.clone()).await.unwrap_err();
    assert_eq!(err.code, "CONNECTION_OPTION_NOT_ALLOWED");
    let err = e.ws.test(&e.core, req).await.unwrap_err();
    assert_eq!(err.code, "CONNECTION_OPTION_NOT_ALLOWED");
    let err = e.core.connect(&sqlite_config()).await.unwrap_err();
    assert_eq!(err.code, "CONNECTION_OPTION_NOT_ALLOWED");
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 3);
    assert!(seen[0].contains("/tmp/p.db"), "{seen:?}");
    assert_eq!(e.opened.load(Ordering::SeqCst), 0);
}

/// A connection whose driver fails to close is still announced: it's out
/// of Core either way.
#[tokio::test]
async fn close_all_announces_failed_closes() {
    let e = env(Some(ConnectPolicy::Unrestricted), true).await;
    let mut events = e.ws.events();
    let id =
        e.ws.connect(&e.core, ConnectRequest::form(form(json!({}))))
            .await
            .unwrap();
    e.ws.close_all(&e.core).await;
    let event = tokio::time::timeout(Duration::from_secs(2), events.next())
        .await
        .unwrap()
        .unwrap();
    let WorkspaceEvent::ConnectionClosed {
        connection_id,
        code,
        ..
    } = event
    else {
        panic!("{event:?}")
    };
    assert_eq!(connection_id, id);
    assert_eq!(code, WORKSPACE_EVICTED);
    assert_eq!(e.core.connection_count(), 0);
}

/// Dropped receivers are pruned when the next one subscribes, not only
/// when an event is sent.
#[tokio::test]
async fn dropped_receivers_are_pruned() {
    let e = env(None, false).await;
    let first = e.ws.events();
    let second = e.ws.events();
    assert_eq!(e.ws.event_subscriber_count(), 2);
    drop(first);
    drop(second);
    let _third = e.ws.events();
    assert_eq!(e.ws.event_subscriber_count(), 1);
}

/// A saved Postgres row with an enabled SSH tunnel whose passwords are in
/// the store.
async fn save_ssh_row(ws: &Workspace) {
    use seaquel_core::storage::{connections, projects};
    let project = serde_json::from_value(json!({
        "id": "p1", "name": "P", "customLabels": [],
        "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
    }))
    .unwrap();
    projects::save(ws.storage(), &project).await.unwrap();
    let row = serde_json::from_value(json!({
        "id": "c-ssh", "projectId": "p1", "name": "Behind SSH", "type": "postgres",
        "host": "db.internal", "port": 5432, "databaseName": "app", "username": "u",
        "labelIds": [], "savePassword": true, "saveSshPassword": true,
        "sshTunnel": { "enabled": true, "host": "10.255.255.1", "port": 22,
                       "username": "tunnel", "authMethod": "password" },
    }))
    .unwrap();
    connections::save(ws.storage(), &row).await.unwrap();
}

/// Under `allow_ssh: false`, a saved row with a tunnel is refused as soon
/// as it's read: no secret-store read, no credential check, no tunnel.
#[tokio::test]
async fn a_saved_ssh_row_is_refused_before_any_secret_read() {
    let e = env(Some(ConnectPolicy::checked(|_| Ok(()), false)), false).await;
    save_ssh_row(&e.ws).await;
    for test in [false, true] {
        let req = ConnectRequest::saved("c-ssh");
        let result = if test {
            e.ws.test(&e.core, req).await
        } else {
            e.ws.connect(&e.core, req).await.map(|_| ())
        };
        let err = result.unwrap_err();
        assert_eq!(err.code, "NOT_SUPPORTED", "{err}");
        assert!(err.message.contains("SSH"), "{err}");
    }
    assert_eq!(e.reads.load(Ordering::SeqCst), 0, "no secret was read");
    assert_eq!(e.core.ssh_tunnel_count(), 0);
    assert_eq!(e.opened.load(Ordering::SeqCst), 0);
    // A form asking for SSH with no password is refused for SSH, not as
    // `CREDENTIALS_REQUIRED`.
    let mut form = ssh_form();
    form.ssh_auth_method = "password".into();
    let err =
        e.ws.connect(&e.core, ConnectRequest::form(form))
            .await
            .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED", "{err}");
    // So is a TablePlus `+ssh` URL.
    let url = form_with_string("postgres+ssh://tunnel@10.255.255.1:22/u:pw@db.internal:5432/app");
    let err =
        e.ws.connect(&e.core, ConnectRequest::form(url))
            .await
            .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED", "{err}");
}

fn form_with_string(s: &str) -> ConnectionForm {
    form(json!({ "type": "postgres", "connectionString": s }))
}

/// `Core::ssh_open` follows the policy too: nothing without one, nothing
/// under `allow_ssh: false`, and neither tries the server.
#[tokio::test]
async fn ssh_open_follows_the_policy() {
    let config: seaquel_core::ssh::TunnelConfig = serde_json::from_value(json!({
        "sshHost": "10.255.255.1", "sshPort": 22, "sshUsername": "tunnel",
        "authMethod": "password", "password": "pw",
        "remoteHost": "db.internal", "remotePort": 5432,
    }))
    .unwrap();
    for policy in [None, Some(ConnectPolicy::checked(|_| Ok(()), false))] {
        let e = env(policy, false).await;
        let err = tokio::time::timeout(Duration::from_secs(2), e.core.ssh_open(&config))
            .await
            .expect("refused without trying the SSH server")
            .unwrap_err();
        assert_eq!(err.code, "NOT_SUPPORTED", "{err}");
        assert_eq!(e.core.ssh_tunnel_count(), 0);
    }
}

/// Run `stream_id` on `ws`'s connection `id` to the end; its events.
async fn run(core: &Core, ws: &Workspace, id: &str, stream_id: &str) -> Vec<String> {
    ws.query_stream(
        core,
        stream_id.into(),
        id.into(),
        "SELECT 1".into(),
        vec![],
        seaquel_core::QueryOptions::default(),
    )
    .map(|event| format!("{event:?}"))
    .collect()
    .await
}

/// Early cancels are kept per workspace: another workspace's 300 cancels
/// don't push out this one's.
#[tokio::test]
async fn early_cancels_are_capped_per_workspace() {
    let e = env(Some(ConnectPolicy::Unrestricted), false).await;
    let b = e
        .core
        .open_workspace(WorkspaceSpec::new(e._dir.path().join("b")))
        .await
        .unwrap();
    let id =
        e.ws.connect(&e.core, ConnectRequest::form(form(json!({}))))
            .await
            .unwrap();
    e.ws.cancel(&e.core, "early");
    for i in 0..300 {
        b.cancel(&e.core, &format!("b-{i}"));
    }
    // Cancelled before it started: no events at all.
    assert!(run(&e.core, &e.ws, &id, "early").await.is_empty());
    // An id nobody cancelled runs to `Done`.
    assert_eq!(run(&e.core, &e.ws, &id, "other").await, ["Done"]);
}

/// A cancel for a stream that already ran and finished is dropped, not
/// remembered for a later stream.
#[tokio::test]
async fn a_late_cancel_isnt_remembered() {
    let e = env(Some(ConnectPolicy::Unrestricted), false).await;
    let id =
        e.ws.connect(&e.core, ConnectRequest::form(form(json!({}))))
            .await
            .unwrap();
    assert_eq!(run(&e.core, &e.ws, &id, "s").await, ["Done"]);
    e.ws.cancel(&e.core, "s");
    assert_eq!(run(&e.core, &e.ws, &id, "s").await, ["Done"]);
}
