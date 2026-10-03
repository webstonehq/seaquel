//! Phase 5e on the web: the `shared` and `imports` groups are refused, and
//! no library write publishes a file, on the Core the server builds
//! (`web_core`, no `LocalFiles`), with every feature compiled in: this
//! crate's dev-dependencies turn on Core's `git` and `imports`, as a
//! workspace test build's feature unification would.

use std::sync::Arc;

use axum::http::StatusCode;
use serde_json::{json, Value as Json};

mod common;
use common::Env;

/// [`Env`] on the server's own Core.
fn web_env() -> Env {
    Env::with_core(
        Arc::new(seaquel_server::web_core(
            seaquel_core::ai::AiEgress::Public,
            None,
        )),
        4,
        Arc::default(),
    )
}

async fn call(env: &Env, group: &str, method: &str, params: Json) -> (StatusCode, Json) {
    let inner = if params.is_null() {
        json!({"method": method})
    } else {
        json!({"method": method, "params": params})
    };
    env.rpc_from(
        "alice",
        &["win-alice"],
        &json!({"method": group, "params": inner}),
    )
    .await
}

async fn ok(env: &Env, group: &str, method: &str, params: Json) -> Json {
    let (status, body) = call(env, group, method, params).await;
    assert_eq!(status, StatusCode::OK, "{group}.{method}: {body}");
    body["result"]["result"].clone()
}

/// A `git init`ed folder with a DBeaver file beside it.
fn repo(env: &Env) -> (String, String) {
    let path = env.dir.path().join("team-repo");
    std::fs::create_dir_all(&path).unwrap();
    std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&path)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .status()
        .map(|s| assert!(s.success()))
        .unwrap_or_else(|_| std::fs::create_dir_all(path.join(".git")).unwrap());
    let file = env.dir.path().join("data-sources.json");
    std::fs::write(
        &file,
        r#"{"connections": {"pg-1": {"provider": "postgresql", "name": "Shop",
            "configuration": {"host": "db.example.com", "port": "5432", "database": "shop",
            "user": "app"}}}}"#,
    )
    .unwrap();
    (
        path.to_string_lossy().into_owned(),
        file.to_string_lossy().into_owned(),
    )
}

/// What the folder holds besides `.git`.
fn written(path: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(path)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n != ".git")
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn shared_and_imports_are_not_supported_on_web() {
    // Both features are compiled in: these names exist only with them.
    let _ = seaquel_core::SyncTarget::Project(String::new());
    let _ = seaquel_core::ImportPaths::new("/nowhere");
    let env = web_env();
    assert_eq!(env.state.core.local_files(), None);
    let (path, file) = repo(&env);
    ok(&env, "library", "projectEnsureDefault", Json::Null).await;
    let project = "default-seaquel";
    for (group, method, params) in [
        ("shared", "reposList", Json::Null),
        (
            "shared",
            "repoRegister",
            json!({"path": path, "name": "n", "remoteUrl": "u"}),
        ),
        (
            "shared",
            "repoUpdate",
            json!({"id": "r", "patch": {"branch": "b"}}),
        ),
        ("shared", "repoRemove", json!({"id": "r"})),
        (
            "shared",
            "linkProject",
            json!({"projectId": project, "path": path, "share": []}),
        ),
        (
            "shared",
            "unlinkProject",
            json!({"projectId": project, "removeImported": true}),
        ),
        ("shared", "scan", json!({"path": path})),
        (
            "shared",
            "importProjects",
            json!({"path": path, "dirs": ["team"]}),
        ),
        ("shared", "sync", json!({"projectId": project})),
        ("shared", "syncRepo", json!({"repoId": "r"})),
        (
            "imports",
            "candidates",
            json!({"source": "dbeaver", "projectId": project, "path": file}),
        ),
        (
            "imports",
            "candidates",
            json!({"source": "tableplus", "projectId": project}),
        ),
        (
            "imports",
            "create",
            json!({"source": "dbeaver", "projectId": project, "keys": ["pg-1"], "path": file}),
        ),
        // The git group, which takes the repo lock on the desktop.
        ("git", "pull", json!({"path": path})),
        ("git", "status", json!({"path": path})),
    ] {
        let (status, body) = call(&env, group, method, params).await;
        assert_eq!(
            status,
            StatusCode::NOT_IMPLEMENTED,
            "{group}.{method}: {body}"
        );
        assert_eq!(body["code"], "NOT_SUPPORTED", "{group}.{method}: {body}");
    }
    // Nothing was registered, imported or written.
    assert!(written(&path).is_empty(), "{:?}", written(&path));
    let connections = ok(&env, "library", "connectionsList", Json::Null).await;
    assert_eq!(connections["value"], json!([]), "{connections}");
    // The storage group's repo methods are gone too.
    let (status, body) = call(&env, "storage", "sharedReposLoadAll", Json::Null).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "INVALID_ARGUMENT", "{body}");
}

/// A project pointed at a repo, with shared rows edited every way that
/// publishes on the desktop: no answer has a `projection`, and the folder
/// stays empty.
#[tokio::test]
async fn a_web_library_write_never_publishes() {
    let env = web_env();
    let (path, _) = repo(&env);
    let project = ok(
        &env,
        "library",
        "projectCreate",
        json!({"project": {"name": "Team"}}),
    )
    .await["value"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut answers = vec![
        ok(
            &env,
            "library",
            "projectUpdate",
            json!({"id": project, "patch": {"gitRepoPath": path}}),
        )
        .await,
    ];
    let conn = ok(
        &env,
        "library",
        "connectionCreate",
        json!({"connection": {"projectId": project, "name": "C", "type": "postgres",
            "host": "h", "port": 5432, "databaseName": "d", "username": "u"}}),
    )
    .await;
    let conn_id = conn["value"]["id"].clone();
    answers.push(conn);
    answers.push(
        ok(
            &env,
            "library",
            "connectionUpdate",
            json!({"id": conn_id, "patch": {"isLocalOnly": false, "host": "h2"}}),
        )
        .await,
    );
    let q = ok(
        &env,
        "library",
        "savedQueryCreate",
        json!({"query": {"projectId": project, "name": "Q", "query": "SELECT 1",
            "shared": true}}),
    )
    .await;
    let q_id = q["value"]["id"].clone();
    answers.push(q);
    answers.push(
        ok(
            &env,
            "library",
            "savedQueryUpdate",
            json!({"id": q_id, "patch": {"query": "SELECT 2", "name": "Q2"}}),
        )
        .await,
    );
    let d = ok(
        &env,
        "library",
        "dashboardCreate",
        json!({"dashboard": {"projectId": project, "name": "D", "shared": true,
            "widgets": [], "viewport": {"x": 0, "y": 0, "zoom": 1}}}),
    )
    .await;
    let d_id = d["value"]["id"].clone();
    answers.push(d);
    answers.push(
        ok(
            &env,
            "library",
            "dashboardUpdate",
            json!({"id": d_id, "patch": {"name": "D2"}}),
        )
        .await,
    );
    answers.push(
        ok(
            &env,
            "library",
            "projectUpdate",
            json!({"id": project, "patch": {"name": "Renamed"}}),
        )
        .await,
    );
    answers.push(
        ok(
            &env,
            "library",
            "savedQueryUpdate",
            json!({"id": q_id, "patch": {"shared": false}}),
        )
        .await,
    );
    answers.push(ok(&env, "library", "savedQueryRemove", json!({"id": q_id})).await);
    answers.push(ok(&env, "library", "dashboardRemove", json!({"id": d_id})).await);
    answers.push(ok(&env, "library", "connectionRemove", json!({"id": conn_id})).await);
    for answer in &answers {
        assert!(answer.get("projection").is_none(), "{answer}");
    }
    assert!(written(&path).is_empty(), "{:?}", written(&path));
}
