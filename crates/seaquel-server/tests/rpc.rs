//! Integration test: `POST /rpc` runs a workspace call for the user in
//! `X-Seaquel-User`, on `DATA_DIR/users/<id>/meta.db`, through an LRU of
//! open workspaces.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use seaquel_server::{build_router, AppState, Workspaces};
use serde_json::{json, Value};
use tower::ServiceExt;

struct Env {
    app: axum::Router,
    workspaces: Arc<Workspaces>,
    dir: tempfile::TempDir,
}

fn env_with_capacity(capacity: usize) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let workspaces = Arc::new(Workspaces::with_capacity(dir.path(), capacity));
    let state = AppState {
        core: Arc::new(seaquel_core::Core::builder().build()),
        workspaces: Arc::clone(&workspaces),
        license: Arc::new(seaquel_core::license::server::LicenseServer::new(
            seaquel_core::license::server::ServerConfig::new(dir.path().join("auth.db")),
        )),
        internal_secret: None,
    };
    Env {
        app: build_router(state),
        workspaces,
        dir,
    }
}

fn env() -> Env {
    env_with_capacity(16)
}

impl Env {
    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// POST `body` to /rpc with the given `X-Seaquel-User` values and return
    /// the status and the raw response body.
    async fn post_raw(&self, users: &[&str], body: &str) -> (StatusCode, String) {
        let mut req = Request::builder()
            .method("POST")
            .uri("/rpc")
            .header("content-type", "application/json");
        for user in users {
            req = req.header("x-seaquel-user", *user);
        }
        let response = self
            .app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    async fn post(&self, user: &str, body: Value) -> (StatusCode, Value) {
        let (status, text) = self.post_raw(&[user], &body.to_string()).await;
        (status, serde_json::from_str(&text).unwrap())
    }

    /// A storage call that must succeed; returns its result.
    async fn storage(&self, user: &str, method: &str, params: Value) -> Value {
        let (status, body) = self
            .post(
                user,
                json!({"method": "storage", "params": {"method": method, "params": params}}),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{method} for {user}: {body}");
        assert_eq!(body["result"]["method"], method);
        body["result"]["result"].clone()
    }

    async fn set(&self, user: &str, key: &str, value: &str) {
        self.storage(user, "appStateSet", json!({"key": key, "value": value}))
            .await;
    }

    async fn get(&self, user: &str, key: &str) -> Value {
        self.storage(user, "appStateGet", json!({"key": key})).await
    }

    /// Wait (up to 5 s) until `n` workspaces have finished closing.
    async fn wait_closed(&self, n: usize) {
        for _ in 0..500 {
            if self.workspaces.closed() >= n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!(
            "expected {n} closed workspaces, got {}",
            self.workspaces.closed()
        );
    }
}

fn users_dir_is_empty(root: &Path) -> bool {
    std::fs::read_dir(root.join("users")).map_or(true, |mut d| d.next().is_none())
}

const LOAD_PROJECTS: &str = r#"{"method":"storage","params":{"method":"projectsLoadAll"}}"#;

// ── The header ──

#[tokio::test]
async fn no_header_is_a_400() {
    let env = env();
    let (status, body) = env.post_raw(&[], LOAD_PROJECTS).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let body: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["code"], "INVALID_ARGUMENT", "{body}");
    assert!(
        body["message"].as_str().unwrap().contains("X-Seaquel-User"),
        "{body}"
    );
    assert!(users_dir_is_empty(env.root()));
}

#[tokio::test]
async fn an_unsafe_user_id_is_a_400() {
    let env = env();
    for bad in [
        "", ".", "..", "../other", "a/b", "/etc", "a\\b", "x..y", "u1 ", "\tu1",
    ] {
        let (status, body) = env.post_raw(&[bad], LOAD_PROJECTS).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad:?}: {body}");
        let body: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(body["code"], "INVALID_ARGUMENT", "{bad:?}: {body}");
    }
    assert!(users_dir_is_empty(env.root()));
    assert!(!env.root().join("meta.db").exists());
    assert_eq!(env.workspaces.opened(), 0);
}

#[tokio::test]
async fn a_repeated_header_is_a_400() {
    let env = env();
    let (status, _) = env.post_raw(&["u1", "u2"], LOAD_PROJECTS).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(env.workspaces.opened(), 0);
}

// ── Bodies and errors ──

#[tokio::test]
async fn a_bad_body_is_a_400_and_opens_nothing() {
    let env = env();
    for body in [
        "not json",
        r#"{"params":{"method":"projectsLoadAll"},"method":"storage"}"#,
        r#"{"method":"nope"}"#,
    ] {
        let (status, text) = env.post_raw(&["u1"], body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}: {text}");
        let err: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(err["code"], "INVALID_ARGUMENT", "{text}");
    }
    assert_eq!(env.workspaces.opened(), 0);
    assert!(!env.root().join("users/u1/meta.db").exists());
}

#[tokio::test]
async fn secrets_are_not_supported_on_the_web() {
    let env = env();
    let (status, body) = env
        .post(
            "u1",
            json!({"method": "secret", "params": {"method": "get", "params": {"key": "db:c1"}}}),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["code"], "NOT_SUPPORTED");
}

/// SSH tunnels are desktop-only. `/rpc` refuses them even in a build where
/// Cargo unified Core's `ssh` feature in (the workspace test build).
#[tokio::test]
async fn ssh_tunnels_are_not_supported_on_the_web() {
    let env = env();
    let open = json!({"method": "ssh", "params": {"method": "open", "params": {"config": {
        "sshHost": "127.0.0.1", "sshPort": 1, "sshUsername": "u", "authMethod": "password",
        "password": "pw", "remoteHost": "db", "remotePort": 5432
    }}}});
    let close =
        json!({"method": "ssh", "params": {"method": "close", "params": {"tunnelId": "tunnel-1"}}});
    for body in [open, close] {
        let (status, res) = env.post("u1", body).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{res}");
        assert_eq!(res["code"], "NOT_SUPPORTED");
    }
}

/// Shared projects (git) are desktop-only. `/rpc` refuses the group even in
/// a build where Cargo unified Core's `git` feature in (the workspace test
/// build, through seaquel-rpc's own dev-dependency), and touches nothing.
#[tokio::test]
async fn git_is_not_supported_on_the_web() {
    let env = env();
    let repo = env.root().join("would-be-repo");
    let calls = [
        json!({"method": "git", "params": {"method": "init", "params": {"path": repo.display().to_string()}}}),
        json!({"method": "git", "params": {"method": "clone", "params": {
            "url": "file:///etc", "path": repo.display().to_string()
        }}}),
        json!({"method": "git", "params": {"method": "status", "params": {"path": "/"}}}),
    ];
    for body in calls {
        let (status, res) = env.post("u1", body).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{res}");
        assert_eq!(res["code"], "NOT_SUPPORTED");
    }
    assert!(
        !repo.exists(),
        "a refused git call created {}",
        repo.display()
    );
}

/// The desktop license client (activate/validate/deactivate against the
/// license service) is desktop-only; the web server's licensing is
/// `/internal/license/*`. `/rpc` refuses the group even when Cargo unified
/// Core's `license-desktop` feature in.
#[tokio::test]
async fn desktop_licensing_is_not_supported_on_the_web() {
    let env = env();
    let calls = [
        json!({"method": "license", "params": {"method": "activate", "params": {
            "key": "SQ-TEST-KEY", "instanceName": "web"
        }}}),
        json!({"method": "license", "params": {"method": "validate", "params": {
            "key": "SQ-TEST-KEY", "instanceId": "i1"
        }}}),
        json!({"method": "license", "params": {"method": "deactivate", "params": {
            "key": "SQ-TEST-KEY", "instanceId": "i1"
        }}}),
    ];
    for body in calls {
        let (status, res) = env.post("u1", body).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{res}");
        assert_eq!(res["code"], "NOT_SUPPORTED");
        assert!(!res.to_string().contains("SQ-TEST-KEY"), "{res}");
    }
}

/// Stored JSON comes back as the bytes that went in: the route passes the
/// body to `parse_request` as it arrived.
#[tokio::test]
async fn json_keeps_its_bytes_through_http() {
    let env = env();
    let odd = r#"{"zeta":1e+21,"alpha":{"b":1.50,"a":-0.0},"mid":"éé","big":12345678901234567890}"#;
    let (status, text) = env
        .post_raw(
            &["u1"],
            &format!(
                r#"{{"method":"storage","params":{{"method":"onboardingSave","params":{{"data":{odd}}}}}}}"#
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    let (status, text) = env
        .post_raw(
            &["u1"],
            r#"{"method":"storage","params":{"method":"onboardingLoad"}}"#,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{text}");
    assert_eq!(
        text,
        format!(r#"{{"method":"storage","result":{{"method":"onboardingLoad","result":{odd}}}}}"#)
    );
}

// ── Users and files ──

#[tokio::test]
async fn a_users_file_lands_at_data_dir_users_id_meta_db() {
    let env = env();
    env.set("tTWa8mP780qWiJrRHni8dww7Oz4xBY2y", "k", "v").await;
    let file = env
        .root()
        .join("users/tTWa8mP780qWiJrRHni8dww7Oz4xBY2y/meta.db");
    assert!(file.is_file(), "{} is missing", file.display());
    let entries: Vec<_> = std::fs::read_dir(env.root().join("users"))
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["tTWa8mP780qWiJrRHni8dww7Oz4xBY2y"]);
}

#[tokio::test]
async fn two_users_dont_see_each_others_saves() {
    let env = env();
    env.set("alice", "theme", "dark").await;
    env.set("bob", "theme", "light").await;
    env.storage(
        "alice",
        "projectsSave",
        json!({"project": {
            "id": "p1", "name": "Alice's", "createdAt": "2026-01-02T03:04:05.000Z",
            "updatedAt": "2026-01-02T03:04:05.000Z", "customLabels": [],
        }}),
    )
    .await;

    assert_eq!(env.get("alice", "theme").await, "dark");
    assert_eq!(env.get("bob", "theme").await, "light");
    assert_eq!(
        env.storage("alice", "projectsLoadAll", Value::Null)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        env.storage("bob", "projectsLoadAll", Value::Null).await,
        json!([])
    );
    assert_eq!(env.get("carol", "theme").await, Value::Null);
}

// ── The LRU ──

#[tokio::test]
async fn the_lru_evicts_closes_and_reopens_cleanly() {
    let env = env_with_capacity(2);
    env.set("u1", "k", "one").await;
    env.set("u2", "k", "two").await;
    assert_eq!(env.workspaces.len(), 2);
    assert_eq!(env.workspaces.opened(), 2);

    // u1 is the least recently used: a third user evicts it and it closes.
    env.set("u3", "k", "three").await;
    assert_eq!(env.workspaces.len(), 2);
    assert!(!env.workspaces.contains("u1"));
    assert!(env.workspaces.contains("u2") && env.workspaces.contains("u3"));
    env.wait_closed(1).await;
    // A closed WAL database checkpoints and removes its -wal file.
    assert!(!env.root().join("users/u1/meta.db-wal").exists());

    // u1 reopens with its data; u2 (now the oldest) goes.
    assert_eq!(env.get("u1", "k").await, "one");
    assert_eq!(env.workspaces.opened(), 4);
    assert!(!env.workspaces.contains("u2"));
    env.wait_closed(2).await;

    // Using a workspace makes it recent: touch u3, then u2 evicts u1.
    assert_eq!(env.get("u3", "k").await, "three");
    assert_eq!(env.get("u2", "k").await, "two");
    assert!(env.workspaces.contains("u3") && env.workspaces.contains("u2"));
    assert!(!env.workspaces.contains("u1"));
    env.wait_closed(3).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_first_requests_open_the_workspace_once() {
    let env = env();
    let mut tasks = Vec::new();
    for i in 0..16 {
        let app = env.app.clone();
        tasks.push(tokio::spawn(async move {
            let body = json!({"method": "storage", "params": {
                "method": "appStateSet", "params": {"key": format!("k{i}"), "value": "v"}
            }});
            let response = app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/rpc")
                        .header("x-seaquel-user", "same-user")
                        .body(Body::from(body.to_string()))
                        .unwrap(),
                )
                .await
                .unwrap();
            response.status()
        }));
    }
    for task in tasks {
        assert_eq!(task.await.unwrap(), StatusCode::OK);
    }
    assert_eq!(env.workspaces.opened(), 1);
    for i in 0..16 {
        assert_eq!(env.get("same-user", &format!("k{i}")).await, "v");
    }
    assert_eq!(env.workspaces.opened(), 1);
}

/// A request holds its workspace for as long as it runs: evicting it only
/// drops the LRU's reference, and the close waits for the request.
#[tokio::test]
async fn an_evicted_workspace_in_use_keeps_working_until_released() {
    let env = env_with_capacity(2);
    let core = seaquel_core::Core::builder().build();
    env.set("u1", "k", "before").await;

    // An in-flight request for u1.
    let held = env.workspaces.get(&core, "u1").await.unwrap();

    // Two more users evict u1 from the LRU.
    env.set("u2", "k", "v").await;
    env.set("u3", "k", "v").await;
    assert!(!env.workspaces.contains("u1"));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(env.workspaces.closed(), 0, "closed while still in use");

    // The held workspace still reads and writes.
    let ws = held.workspace();
    seaquel_core::storage::app_state::set(ws.storage(), "k", Some("during"))
        .await
        .unwrap();
    assert_eq!(
        seaquel_core::storage::app_state::get(ws.storage(), "k")
            .await
            .unwrap()
            .as_deref(),
        Some("during")
    );

    // Releasing it closes it, and u1 reopens with the write.
    drop(held);
    env.wait_closed(1).await;
    assert_eq!(env.get("u1", "k").await, "during");
}

#[tokio::test]
async fn a_file_that_fails_to_open_isnt_kept() {
    let env = env();
    let dir = env.root().join("users/broken");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("meta.db"), b"this is not a sqlite file at all").unwrap();

    let (status, body) = env
        .post("broken", serde_json::from_str(LOAD_PROJECTS).unwrap())
        .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
    assert_eq!(body["code"], "STORAGE_CORRUPT", "{body}");
    // The browser never sees where the server keeps its files.
    let message = body["message"].as_str().unwrap();
    let canonical = std::fs::canonicalize(env.root()).unwrap();
    assert!(
        !message.contains(&*env.root().to_string_lossy())
            && !message.contains(&*canonical.to_string_lossy()),
        "{message}"
    );
    assert!(
        message.contains("DATA_DIR/users/broken/meta.db"),
        "{message}"
    );
    assert!(!env.workspaces.contains("broken"));
    assert_eq!(
        std::fs::read(dir.join("meta.db")).unwrap(),
        b"this is not a sqlite file at all"
    );
}
