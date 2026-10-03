//! Phase 6 probe F4: connections belong to the window (write origin) that
//! opened them. A window that connects a saved connection again replaces
//! its older connection for it, and `Workspace::close_owned_by` closes a
//! window's connections (the web server calls it for a window whose socket
//! stayed closed). Connections opened with no origin, or by another window,
//! are never touched. Also the connect timeout: a database that accepts and
//! never answers fails with `TIMEOUT`.
//!
//! Storage is a temp dir and the connections are SQLite files, so nothing
//! here needs a live database; the timeout cases use a local listener that
//! accepts and never answers.
// A native test: the silent server is a tokio task.
#![allow(clippy::disallowed_methods)]
#![cfg(all(
    feature = "storage",
    feature = "workspace",
    feature = "engine-postgres",
    feature = "engine-mysql",
    feature = "engine-sqlite",
    feature = "engine-mssql"
))]

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::storage::{connections, projects};
use seaquel_core::{
    ConnectRequest, ConnectionForm, Core, QueryOptions, StreamEvent, SuppliedSecrets, Workspace,
    WorkspaceEvent, WorkspaceSpec, WriteOrigin,
};
use seaquel_types::storage::{PersistedConnection, PersistedProject};
use serde_json::{json, Value};

const PROJECT: &str = "p1";
const MANY_ROWS: &str = "SELECT a.x, b.x FROM t a, t b";

struct Fixture {
    dir: tempfile::TempDir,
    core: Core,
    ws: Arc<Workspace>,
}

fn core_builder() -> seaquel_core::CoreBuilder {
    seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
}

async fn fixture_on(core: Core) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let project: PersistedProject = serde_json::from_value(json!({
        "id": PROJECT, "name": "Project", "customLabels": [],
        "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
    }))
    .unwrap();
    projects::save(ws.storage(), &project).await.unwrap();
    Fixture { dir, core, ws }
}

async fn fixture() -> Fixture {
    fixture_on(core_builder().build()).await
}

fn origin(o: &str) -> WriteOrigin {
    WriteOrigin::new(Some(o))
}

impl Fixture {
    /// Save SQLite row `id` on file `<id>.db`, with a table of many rows.
    async fn save(&self, id: &str) {
        let file = self.dir.path().join(format!("{id}.db"));
        let row: PersistedConnection = serde_json::from_value(json!({
            "id": id, "projectId": PROJECT, "name": format!("Saved {id}"), "type": "sqlite",
            "host": "localhost", "port": 0, "databaseName": file.to_str().unwrap(),
            "username": "", "savePassword": false, "saveSshPassword": false,
            "saveSshKeyPassphrase": false, "labelIds": [],
        }))
        .unwrap();
        connections::save(self.ws.storage(), &row).await.unwrap();
        // Create the file and its table through a connection of no window.
        let id = self
            .ws
            .connect(
                &self.core,
                ConnectRequest::form(sqlite_form(&file)).with_create_if_missing(true),
            )
            .await
            .unwrap();
        self.ws
            .execute(
                &self.core,
                &id,
                "CREATE TABLE IF NOT EXISTS t AS WITH RECURSIVE c(x) AS \
                 (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 200000) SELECT x FROM c",
                vec![],
            )
            .await
            .unwrap();
        self.ws.disconnect(&self.core, &id).await.unwrap();
    }

    /// Connect saved row `id` from window `from`.
    async fn connect_saved(&self, id: &str, from: &str) -> String {
        self.ws
            .connect(
                &self.core,
                ConnectRequest::saved(id).with_origin(origin(from)),
            )
            .await
            .unwrap_or_else(|e| panic!("{id} from {from}: {e}"))
    }

    fn ids(&self) -> Vec<String> {
        let mut ids = self.ws.connection_ids(&self.core);
        ids.sort();
        ids
    }

    fn stream(
        &self,
        stream_id: &str,
        connection_id: &str,
    ) -> futures::stream::BoxStream<'_, StreamEvent> {
        self.ws.query_stream(
            &self.core,
            stream_id.into(),
            connection_id.into(),
            MANY_ROWS.into(),
            vec![],
            QueryOptions::default(),
        )
    }
}

fn sqlite_form(file: &std::path::Path) -> ConnectionForm {
    form(json!({ "databaseName": file.to_str().unwrap() }))
}

fn form(fields: Value) -> ConnectionForm {
    let mut form = json!({
        "name": "Form", "type": "sqlite", "host": "localhost", "port": 0,
        "databaseName": "", "username": "", "connectionString": "",
        "sshEnabled": false, "sshHost": "", "sshPort": 22, "sshUsername": "",
        "sshAuthMethod": "password", "sshKeyPath": "",
        "savePassword": false, "saveSshPassword": false, "saveSshKeyPassphrase": false,
    });
    for (k, v) in fields.as_object().unwrap() {
        form[k] = v.clone();
    }
    serde_json::from_value(form).unwrap()
}

/// Drain `stream` to its end and return its last event.
async fn last_event(
    mut stream: futures::stream::BoxStream<'_, StreamEvent>,
) -> Option<StreamEvent> {
    let mut last = None;
    while let Some(event) = stream.next().await {
        last = Some(event);
    }
    last
}

/// `fut`, or a panic after 20 s (a stream that isn't closed never ends).
async fn bounded<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), fut)
        .await
        .expect("still running after 20 s")
}

fn closed_with(event: &Option<StreamEvent>, code: &str) -> bool {
    matches!(event, Some(StreamEvent::Error { code: c, .. }) if c == code)
}

/// The `ConnectionClosed` events waiting on `events`, as (id, code).
fn closed_events(
    events: &mut futures::stream::BoxStream<'static, WorkspaceEvent>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Some(Some(event)) = events.next().now_or_never() {
        if let WorkspaceEvent::ConnectionClosed {
            connection_id,
            code,
            ..
        } = event
        {
            out.push((connection_id, code));
        }
    }
    out
}

use futures::FutureExt;

// ── Replace on reconnect ──

/// A window that connects a saved connection again (a reload) gets a new
/// connection, and its older one for that saved connection is closed: its
/// stream ends `CONNECTION_CLOSED`, and the close is announced as
/// `CONNECTION_REPLACED`.
#[tokio::test]
async fn reconnecting_from_the_same_window_closes_the_old_connection() {
    let f = fixture().await;
    f.save("c1").await;
    let mut events = f.ws.events();
    let old = f.connect_saved("c1", "win-a").await;
    let mut stream = f.stream("s1", &old);
    assert!(matches!(stream.next().await, Some(StreamEvent::Batch(_))));

    let (new, last) =
        bounded(async { tokio::join!(f.connect_saved("c1", "win-a"), last_event(stream)) }).await;
    assert_ne!(new, old);
    assert!(closed_with(&last, "CONNECTION_CLOSED"), "{last:?}");
    assert_eq!(f.ids(), vec![new.clone()]);
    assert_eq!(
        closed_events(&mut events),
        vec![(old.clone(), "CONNECTION_REPLACED".to_string())]
    );
    // The new one works; the old one is gone.
    f.ws.query(&f.core, &new, "SELECT 1", vec![]).await.unwrap();
    let err =
        f.ws.query(&f.core, &old, "SELECT 1", vec![])
            .await
            .unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
}

/// Five reloads leave one connection per saved connection, and a form
/// connect naming the saved connection (`savedConnectionId`, the reconnect
/// tab) replaces too.
#[tokio::test]
async fn five_reloads_leave_one_connection_per_saved_connection() {
    let f = fixture().await;
    f.save("c1").await;
    f.save("c2").await;
    for _ in 0..5 {
        f.connect_saved("c1", "win-a").await;
        f.connect_saved("c2", "win-a").await;
    }
    assert_eq!(f.ids().len(), 2);
    let file = f.dir.path().join("c1.db");
    let by_form =
        f.ws.connect(
            &f.core,
            ConnectRequest::form(sqlite_form(&file))
                .with_saved_connection_id(Some("c1".into()))
                .with_origin(origin("win-a")),
        )
        .await
        .unwrap();
    let ids = f.ids();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&by_form));
}

/// Another window's connection for the same saved connection stays, and so
/// do connections opened with no origin (the CLI, MCP, today's callers).
#[tokio::test]
async fn another_window_and_no_window_keep_their_connections() {
    let f = fixture().await;
    f.save("c1").await;
    let a = f.connect_saved("c1", "win-a").await;
    let b = f.connect_saved("c1", "win-b").await;
    let none1 =
        f.ws.connect(&f.core, ConnectRequest::saved("c1"))
            .await
            .unwrap();
    let none2 =
        f.ws.connect(&f.core, ConnectRequest::saved("c1"))
            .await
            .unwrap();
    let mut want = vec![a, b, none1, none2];
    want.sort();
    assert_eq!(f.ids(), want);
}

/// A reconnect that fails leaves the window's old connection open.
#[tokio::test]
async fn a_failed_reconnect_keeps_the_old_connection() {
    let f = fixture().await;
    f.save("c1").await;
    let old = f.connect_saved("c1", "win-a").await;
    // The reconnect tab's form, pointing at a file that doesn't exist (and
    // isn't to be created): the driver refuses it.
    let missing = f.dir.path().join("missing.db");
    let err =
        f.ws.connect(
            &f.core,
            ConnectRequest::form(sqlite_form(&missing))
                .with_saved_connection_id(Some("c1".into()))
                .with_origin(origin("win-a")),
        )
        .await
        .unwrap_err();
    assert_ne!(err.code, "", "{err}");
    assert_eq!(f.ids(), vec![old.clone()]);
    f.ws.query(&f.core, &old, "SELECT 1", vec![]).await.unwrap();
}

/// `bindSaved` (a connection made before its row existed) replaces the
/// window's older connection for that saved connection too.
#[tokio::test]
async fn binding_a_saved_id_replaces_the_windows_older_connection() {
    let f = fixture().await;
    f.save("c1").await;
    let old = f.connect_saved("c1", "win-a").await;
    let file = f.dir.path().join("c1.db");
    let fresh =
        f.ws.connect(
            &f.core,
            ConnectRequest::form(sqlite_form(&file)).with_origin(origin("win-a")),
        )
        .await
        .unwrap();
    // Not bound yet: both stay.
    assert_eq!(f.ids().len(), 2);
    f.ws.bind_saved_connection(&f.core, &fresh, "c1")
        .await
        .unwrap();
    assert_eq!(f.ids(), vec![fresh]);
    let err =
        f.ws.query(&f.core, &old, "SELECT 1", vec![])
            .await
            .unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
}

/// Only older connections are replaced: when two connects of one window for
/// one saved connection overlap, the one that finishes its replace last
/// must not close the newer one (else both would go). Here the older
/// connection is bound to the saved id after a newer one opened for it.
#[tokio::test]
async fn a_replace_never_closes_a_newer_connection() {
    let f = fixture().await;
    f.save("c1").await;
    let file = f.dir.path().join("c1.db");
    let older =
        f.ws.connect(
            &f.core,
            ConnectRequest::form(sqlite_form(&file)).with_origin(origin("win-a")),
        )
        .await
        .unwrap();
    let newer = f.connect_saved("c1", "win-a").await;
    f.ws.bind_saved_connection(&f.core, &older, "c1")
        .await
        .unwrap();
    let mut want = vec![older, newer];
    want.sort();
    assert_eq!(f.ids(), want);
}

/// Under `per_workspace`, the connection a reconnect replaces doesn't count
/// against the cap: a window at the cap can still reload.
#[tokio::test]
async fn a_replaced_connection_doesnt_count_against_the_cap() {
    let f = fixture_on(
        core_builder()
            .connection_limits(seaquel_core::ConnectionLimits {
                per_workspace: Some(1),
                max_pool_size: None,
            })
            .build(),
    )
    .await;
    f.save("c1").await;
    f.connect_saved("c1", "win-a").await;
    let new = f.connect_saved("c1", "win-a").await;
    assert_eq!(f.ids(), vec![new]);
    // A different saved connection is still refused at the cap.
    f.save_without_connection("c2").await;
    let err =
        f.ws.connect(
            &f.core,
            ConnectRequest::saved("c2").with_origin(origin("win-a")),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "TOO_MANY_CONNECTIONS", "{err}");
}

impl Fixture {
    /// Save SQLite row `id` without opening anything (its file is created
    /// empty).
    async fn save_without_connection(&self, id: &str) {
        let file = self.dir.path().join(format!("{id}.db"));
        std::fs::write(&file, b"").unwrap();
        let row: PersistedConnection = serde_json::from_value(json!({
            "id": id, "projectId": PROJECT, "name": format!("Saved {id}"), "type": "sqlite",
            "host": "localhost", "port": 0, "databaseName": file.to_str().unwrap(),
            "username": "", "savePassword": false, "saveSshPassword": false,
            "saveSshKeyPassphrase": false, "labelIds": [],
        }))
        .unwrap();
        connections::save(self.ws.storage(), &row).await.unwrap();
    }
}

/// Review M2: the discount for replaced connections is netted across
/// connects in flight. A tab at the cap that reconnects all of its saved
/// connections at once (a reload) gets every one back.
#[tokio::test]
async fn a_tab_at_the_cap_reconnects_several_saved_ids_in_parallel() {
    let f = fixture_on(
        core_builder()
            .connection_limits(seaquel_core::ConnectionLimits {
                per_workspace: Some(3),
                max_pool_size: None,
            })
            .build(),
    )
    .await;
    for id in ["c1", "c2", "c3"] {
        f.save(id).await;
        f.connect_saved(id, "win-a").await;
    }
    let old = f.ids();
    let (a, b, c) = tokio::join!(
        f.ws.connect(
            &f.core,
            ConnectRequest::saved("c1").with_origin(origin("win-a"))
        ),
        f.ws.connect(
            &f.core,
            ConnectRequest::saved("c2").with_origin(origin("win-a"))
        ),
        f.ws.connect(
            &f.core,
            ConnectRequest::saved("c3").with_origin(origin("win-a"))
        ),
    );
    let mut new = vec![a.unwrap(), b.unwrap(), c.unwrap()];
    new.sort();
    assert_eq!(f.ids(), new);
    assert!(new.iter().all(|id| !old.contains(id)));
    // Still at the cap: one more saved connection is refused.
    f.save_without_connection("c4").await;
    let err =
        f.ws.connect(
            &f.core,
            ConnectRequest::saved("c4").with_origin(origin("win-a")),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "TOO_MANY_CONNECTIONS", "{err}");
}

/// Two connects in flight that would replace the same connection discount
/// it once: the cap still holds.
#[tokio::test]
async fn one_replaced_connection_is_discounted_once() {
    let f = fixture_on(
        core_builder()
            .connection_limits(seaquel_core::ConnectionLimits {
                per_workspace: Some(1),
                max_pool_size: None,
            })
            .build(),
    )
    .await;
    f.save("c1").await;
    f.connect_saved("c1", "win-a").await;
    let (a, b) = tokio::join!(
        f.ws.connect(
            &f.core,
            ConnectRequest::saved("c1").with_origin(origin("win-a"))
        ),
        f.ws.connect(
            &f.core,
            ConnectRequest::saved("c1").with_origin(origin("win-a"))
        ),
    );
    let ok = [a.is_ok(), b.is_ok()].iter().filter(|x| **x).count();
    assert_eq!(ok, 1, "{a:?} {b:?}");
    assert_eq!(f.ids().len(), 1);
}

// ── alive ──

/// `db.alive`: which of the ids asked this workspace still holds, in the
/// order asked; closed ones, unknown ones and another workspace's aren't.
#[tokio::test]
async fn alive_answers_only_this_workspaces_open_connections() {
    let f = fixture().await;
    f.save("c1").await;
    f.save("c2").await;
    let a = f.connect_saved("c1", "win-a").await;
    let b = f.connect_saved("c2", "win-a").await;
    let other = f
        .core
        .open_workspace(WorkspaceSpec::new(f.dir.path().join("other")))
        .await
        .unwrap();
    let file = f.dir.path().join("c1.db");
    let theirs = other
        .connect(&f.core, ConnectRequest::form(sqlite_form(&file)))
        .await
        .unwrap();
    f.ws.disconnect(&f.core, &a).await.unwrap();
    let asked = vec![b.clone(), a, theirs, "sqlite-nope".to_string()];
    assert_eq!(f.ws.alive(&f.core, &asked), vec![b]);
}

// ── close_owned_by ──

/// `close_owned_by` closes exactly the window's connections, ends their
/// streams `CONNECTION_CLOSED`, announces each as `WINDOW_CLOSED`, and
/// leaves another window's and the unowned ones alone.
#[tokio::test]
async fn close_owned_by_closes_only_that_windows_connections() {
    let f = fixture().await;
    f.save("c1").await;
    f.save("c2").await;
    let mut events = f.ws.events();
    let a1 = f.connect_saved("c1", "win-a").await;
    let a2 = f.connect_saved("c2", "win-a").await;
    let b1 = f.connect_saved("c1", "win-b").await;
    let unowned =
        f.ws.connect(&f.core, ConnectRequest::saved("c2"))
            .await
            .unwrap();
    let mut stream = f.stream("s1", &a1);
    assert!(matches!(stream.next().await, Some(StreamEvent::Batch(_))));
    let mut b_stream = f.stream("s2", &b1);
    assert!(matches!(b_stream.next().await, Some(StreamEvent::Batch(_))));

    let (closed, last) =
        bounded(async { tokio::join!(f.ws.close_owned_by(&f.core, "win-a"), last_event(stream)) })
            .await;
    assert_eq!(closed, 2);
    assert!(closed_with(&last, "CONNECTION_CLOSED"), "{last:?}");
    let mut want = vec![b1.clone(), unowned];
    want.sort();
    assert_eq!(f.ids(), want);
    let mut got = closed_events(&mut events);
    got.sort();
    let mut want = vec![
        (a1, "WINDOW_CLOSED".to_string()),
        (a2, "WINDOW_CLOSED".to_string()),
    ];
    want.sort();
    assert_eq!(got, want);
    // win-b's stream still runs.
    assert!(matches!(b_stream.next().await, Some(StreamEvent::Batch(_))));
    drop(b_stream);
    // Nothing left of win-a: a second call closes nothing.
    assert_eq!(f.ws.close_owned_by(&f.core, "win-a").await, 0);
}

// ── Connect timeout ──

/// A local server that accepts every connection and never sends a byte.
async fn silent_listener() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    (port, task)
}

/// Postgres, MySQL and SQL Server against a server that never answers:
/// `connect` and `test` fail with `TIMEOUT` at Core's limit, here 300 ms,
/// instead of hanging, and nothing stays open.
#[tokio::test]
async fn a_connect_to_a_silent_server_times_out() {
    let f = fixture_on(
        core_builder()
            .connect_timeout(Duration::from_millis(300))
            .build(),
    )
    .await;
    let (port, _server) = silent_listener().await;
    for ty in ["postgres", "mysql", "mssql"] {
        let req = || {
            ConnectRequest::form(form(json!({
                "type": ty, "host": "127.0.0.1", "port": port, "databaseName": "app",
                "username": "u", "sslMode": "disable",
            })))
            .with_secrets(SuppliedSecrets {
                db: Some("pw-not-real".into()),
                ssh: None,
                ssh_key: None,
            })
            .with_origin(origin("win-a"))
        };
        let started = tokio::time::Instant::now();
        let err = tokio::time::timeout(Duration::from_secs(10), f.ws.connect(&f.core, req()))
            .await
            .unwrap_or_else(|_| panic!("{ty}: connect still hanging after 10 s"))
            .unwrap_err();
        assert_eq!(err.code, "TIMEOUT", "{ty}: {err}");
        assert!(started.elapsed() < Duration::from_secs(5), "{ty}");
        let err = tokio::time::timeout(Duration::from_secs(10), f.ws.test(&f.core, req()))
            .await
            .unwrap_or_else(|_| panic!("{ty}: test still hanging after 10 s"))
            .unwrap_err();
        assert_eq!(err.code, "TIMEOUT", "{ty}: {err}");
    }
    assert!(f.ids().is_empty());
}
