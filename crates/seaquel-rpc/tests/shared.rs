//! Phase 5e over the wire: the `shared` and `imports` groups, the two
//! retired storage methods, `Debug` redaction, the `LocalFiles` check in
//! `dispatch_workspace`, and the `git` group under the repo lock.
//!
//! Repos are `git init`ed in temp dirs, the imports read an injected home,
//! and libgit2's config search paths point at an empty temp dir, so no test
//! reads the user's home, git config or files.

use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::git::Git;
use seaquel_core::secrets::MemoryStore;
use seaquel_core::{Core, ImportPaths, LocalFiles, Workspace, WorkspaceSpec};
use seaquel_rpc::{
    dispatch_git, dispatch_workspace, parse_request, workspace_events, Request, RpcError,
    WriteOrigin,
};
use serde_json::{json, Value as Json};

// ── Helpers ──

fn isolate_git() {
    static EMPTY: OnceLock<tempfile::TempDir> = OnceLock::new();
    EMPTY.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        for level in [
            git2::ConfigLevel::Global,
            git2::ConfigLevel::XDG,
            git2::ConfigLevel::System,
            git2::ConfigLevel::ProgramData,
        ] {
            // SAFETY: once, before this binary's other libgit2 calls.
            unsafe { git2::opts::set_search_path(level, dir.path()).unwrap() };
        }
        dir
    });
}

struct Env {
    core: Core,
    ws: Arc<Workspace>,
    dir: tempfile::TempDir,
}

const WINDOW: &str = "win-1";

/// A desktop-like Core (`LocalFiles` when `files`, an injected home) on a
/// temp workspace with a memory keychain.
async fn env(files: bool) -> Env {
    isolate_git();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("home")).unwrap();
    let mut builder = seaquel_core::with_plugins(|id| id == "postgres")
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .import_paths(ImportPaths::new(dir.path().join("home")));
    if files {
        builder = builder.local_files(LocalFiles::Allowed);
    }
    let core = builder.build();
    let ws = core
        .open_workspace(
            WorkspaceSpec::new(dir.path().join("data")).with_secrets(Arc::new(MemoryStore::new())),
        )
        .await
        .unwrap();
    Env { core, ws, dir }
}

impl Env {
    async fn call(&self, body: &Json) -> Result<Json, RpcError> {
        let req = parse_request(body.to_string().as_bytes())?;
        let res =
            dispatch_workspace(&self.core, &self.ws, req, WriteOrigin::new(Some(WINDOW))).await?;
        Ok(serde_json::to_value(res).unwrap())
    }

    /// One call of `group`; its result, checked to name its method.
    async fn group(&self, group: &str, method: &str, params: Json) -> Result<Json, RpcError> {
        let inner = if params.is_null() {
            json!({"method": method})
        } else {
            json!({"method": method, "params": params})
        };
        let res = self
            .call(&json!({"method": group, "params": inner}))
            .await?;
        assert_eq!(res["method"], group, "{res}");
        assert_eq!(res["result"]["method"], method, "{res}");
        Ok(res["result"]["result"].clone())
    }

    async fn shared(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.group("shared", method, params).await
    }

    async fn imports(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.group("imports", method, params).await
    }

    async fn lib(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.group("library", method, params).await
    }

    async fn project(&self, name: &str) -> String {
        self.lib("projectCreate", json!({"project": {"name": name}}))
            .await
            .unwrap()["value"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// A `git init`ed folder under the temp dir.
    fn repo(&self, name: &str) -> String {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        git2::Repository::init(&path).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// A DBeaver `data-sources.json` with one Postgres connection, at a
    /// path of its own (the dialog's file picker).
    fn dbeaver_file(&self) -> String {
        let path = self.dir.path().join("data-sources.json");
        std::fs::write(
            &path,
            r#"{"connections": {"pg-1": {"provider": "postgresql", "name": "Shop",
                "configuration": {"host": "db.example.com", "port": "5432", "database": "shop",
                "user": "app"}}}}"#,
        )
        .unwrap();
        path.to_string_lossy().into_owned()
    }
}

fn seq_ok(v: &Json) {
    assert!(v["seq"]["epoch"].is_string(), "{v}");
    assert!(v["seq"]["n"].is_u64(), "{v}");
}

// ── Every method ──

/// Each new method, through `dispatch_workspace` on a desktop-like Core,
/// answers under its own name with the shape the GUI reads.
#[tokio::test]
async fn every_shared_and_imports_method_answers_with_its_own_name() {
    let env = env(true).await;
    let repo = env.repo("team");
    let project = env.project("Team").await;

    // The repo list.
    let registered = env
        .shared("repoRegister", json!({"path": repo, "name": "team-repo"}))
        .await
        .unwrap();
    seq_ok(&registered);
    let repo_id = registered["value"]["id"].as_str().unwrap().to_string();
    assert!(repo_id.starts_with("repo-"), "{registered}");
    assert_eq!(registered["value"]["name"], "team-repo");
    let list = env.shared("reposList", Json::Null).await.unwrap();
    seq_ok(&list);
    assert_eq!(list["value"][0]["id"], repo_id.as_str(), "{list}");
    let updated = env
        .shared(
            "repoUpdate",
            json!({"id": repo_id, "patch": {"branch": "main"}}),
        )
        .await
        .unwrap();
    assert_eq!(updated["value"]["branch"], "main", "{updated}");

    // Link, scan, sync.
    let linked = env
        .shared(
            "linkProject",
            json!({"projectId": project, "path": repo, "share": []}),
        )
        .await
        .unwrap();
    seq_ok(&linked);
    assert_eq!(linked["value"]["conflicted"], false, "{linked}");
    assert!(linked["value"]["notices"].is_array(), "{linked}");
    assert!(
        linked["value"].get("failures").is_none(),
        "no failures, no key: {linked}"
    );
    let preview = env.shared("scan", json!({"path": repo})).await.unwrap();
    assert_eq!(preview["conflicted"], false, "{preview}");
    let dir = preview["projects"][0]["dir"].as_str().unwrap().to_string();
    assert_eq!(
        preview["projects"][0]["linkedProjectIds"],
        json!([project]),
        "{preview}"
    );
    let synced = env
        .shared("sync", json!({"projectId": project}))
        .await
        .unwrap();
    seq_ok(&synced);
    assert_eq!(synced["value"]["rowsChanged"], 0, "{synced}");
    let synced = env
        .shared("syncRepo", json!({"repoId": repo_id}))
        .await
        .unwrap();
    seq_ok(&synced);

    // What an unlink would remove: nothing came from the repo here.
    let preview = env
        .shared("unlinkPreview", json!({"projectId": project}))
        .await
        .unwrap();
    assert_eq!(preview, json!({"importedConnectionIds": []}));

    // Importing the directory a project here links is refused per
    // directory; a second directory imports as a project,
    // then both are unlinked.
    std::fs::create_dir_all(Path::new(&repo).join(".seaquel/projects/ops")).unwrap();
    std::fs::write(
        Path::new(&repo).join(".seaquel/projects/ops/project.yaml"),
        "name: Ops\n",
    )
    .unwrap();
    let imported = env
        .shared(
            "importProjects",
            json!({"path": repo, "dirs": [dir, "ops"]}),
        )
        .await
        .unwrap();
    seq_ok(&imported);
    assert_eq!(
        imported["value"]["failures"][0]["code"], "PROJECT_ALREADY_LINKED",
        "{imported}"
    );
    assert_eq!(imported["value"]["failures"][0]["dir"], dir.as_str());
    let second = imported["value"]["projectIds"][0]
        .as_str()
        .unwrap()
        .to_string();
    let unlinked = env
        .shared(
            "unlinkProject",
            json!({"projectId": second, "removeImported": true}),
        )
        .await
        .unwrap();
    assert_eq!(
        unlinked["value"],
        json!({"removedConnectionIds": [], "keptConnectionIds": [], "repoRemoved": false})
    );
    // A repo a project links to stays.
    let err = env
        .shared("repoRemove", json!({"id": repo_id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "REPO_IN_USE", "{err}");
    let unlinked = env
        .shared(
            "unlinkProject",
            json!({"projectId": project, "removeImported": true}),
        )
        .await
        .unwrap();
    assert_eq!(unlinked["value"]["repoRemoved"], true, "{unlinked}");
    let err = env
        .shared("repoRemove", json!({"id": repo_id}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "REPO_NOT_FOUND", "{err}");
    let again = env
        .shared("repoRegister", json!({"path": repo}))
        .await
        .unwrap();
    let removed = env
        .shared("repoRemove", json!({"id": again["value"]["id"]}))
        .await
        .unwrap();
    seq_ok(&removed);
    assert_eq!(removed["value"], Json::Null);

    // The imports: the default location (the injected home) has nothing;
    // a picked file has one candidate, which imports once.
    let none = env
        .imports(
            "candidates",
            json!({"source": "dbeaver", "projectId": project}),
        )
        .await
        .unwrap();
    assert_eq!(none, json!({"found": false}));
    let file = env.dbeaver_file();
    let found = env
        .imports(
            "candidates",
            json!({"source": "dbeaver", "projectId": project, "path": file}),
        )
        .await
        .unwrap();
    assert_eq!(found["found"], true, "{found}");
    assert_eq!(found["candidates"][0]["key"], "pg-1", "{found}");
    assert_eq!(found["candidates"][0]["type"], "postgres", "{found}");
    let created = env
        .imports(
            "create",
            json!({"source": "dbeaver", "projectId": project, "keys": ["pg-1"], "path": file}),
        )
        .await
        .unwrap();
    seq_ok(&created);
    assert_eq!(created["value"]["results"][0]["status"], "imported");
    let created = env
        .imports(
            "create",
            json!({"source": "dbeaver", "projectId": project, "keys": ["pg-1"], "path": file}),
        )
        .await
        .unwrap();
    assert_eq!(
        created["value"]["results"][0]["status"], "duplicate",
        "{created}"
    );
}

/// The new codes reach the wire as Core's.
#[tokio::test]
async fn shared_refusals_keep_their_codes() {
    let env = env(true).await;
    let project = env.project("Solo").await;
    let err = env
        .shared("sync", json!({"projectId": project}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "PROJECT_NOT_LINKED");
    let err = env
        .shared("syncRepo", json!({"repoId": "repo-nope"}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "REPO_NOT_FOUND");
    let missing = env
        .dir
        .path()
        .join("missing")
        .to_string_lossy()
        .into_owned();
    let err = env
        .shared(
            "linkProject",
            json!({"projectId": project, "path": missing, "share": []}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "FILE_ERROR");
    let a = env.repo("a");
    let b = env.repo("b");
    env.shared(
        "linkProject",
        json!({"projectId": project, "path": a, "share": []}),
    )
    .await
    .unwrap();
    let err = env
        .shared(
            "linkProject",
            json!({"projectId": project, "path": b, "share": []}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "PROJECT_ALREADY_LINKED");
    let err = env
        .imports(
            "create",
            json!({"source": "dbeaver", "projectId": project, "keys": ["pg-1"]}),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "IMPORT_SOURCE_UNREADABLE");
}

/// `dispatch_workspace` refuses both groups on a Core without `LocalFiles`
/// (the web server's), before Core is reached.
#[tokio::test]
async fn both_groups_are_not_supported_without_local_files() {
    let env = env(false).await;
    let repo = env.repo("r");
    let file = env.dbeaver_file();
    for (group, method, params) in [
        ("shared", "reposList", Json::Null),
        ("shared", "repoRegister", json!({"path": repo})),
        ("shared", "repoUpdate", json!({"id": "r", "patch": {}})),
        ("shared", "repoRemove", json!({"id": "r"})),
        (
            "shared",
            "linkProject",
            json!({"projectId": "p", "path": repo, "share": []}),
        ),
        (
            "shared",
            "unlinkProject",
            json!({"projectId": "p", "removeImported": true}),
        ),
        ("shared", "scan", json!({"path": repo})),
        (
            "shared",
            "importProjects",
            json!({"path": repo, "dirs": ["d"]}),
        ),
        ("shared", "sync", json!({"projectId": "p"})),
        ("shared", "syncRepo", json!({"repoId": "r"})),
        (
            "imports",
            "candidates",
            json!({"source": "tableplus", "projectId": "p", "path": file}),
        ),
        (
            "imports",
            "create",
            json!({"source": "dbeaver", "projectId": "p", "keys": ["pg-1"], "path": file}),
        ),
    ] {
        let err = env.group(group, method, params).await.unwrap_err();
        assert_eq!(err.code, "NOT_SUPPORTED", "{group}.{method}: {err}");
        // The dispatcher's own refusal, not Core's ("… aren't available
        // here"): the check doesn't rely on Core alone.
        assert!(
            err.message.ends_with("isn't supported here"),
            "{group}.{method}: {err}"
        );
    }
    assert!(
        !Path::new(&repo).join(".seaquel").exists(),
        "nothing was written"
    );
}

// ── The wire ──

#[test]
fn new_requests_round_trip_byte_for_byte() {
    for text in [
        r#"{"method":"shared","params":{"method":"reposList"}}"#,
        r#"{"method":"shared","params":{"method":"repoRegister","params":{"path":"/r"}}}"#,
        r#"{"method":"shared","params":{"method":"repoRegister","params":{"path":"/r","name":"n","remoteUrl":"u"}}}"#,
        r#"{"method":"shared","params":{"method":"repoUpdate","params":{"id":"r","patch":{}}}}"#,
        r#"{"method":"shared","params":{"method":"repoUpdate","params":{"id":"r","patch":{"name":"n","remoteUrl":"u","branch":"b"}}}}"#,
        r#"{"method":"shared","params":{"method":"repoRemove","params":{"id":"r"}}}"#,
        r#"{"method":"shared","params":{"method":"linkProject","params":{"projectId":"p","path":"/r","share":["c1","c2"]}}}"#,
        r#"{"method":"shared","params":{"method":"unlinkProject","params":{"projectId":"p","removeImported":false}}}"#,
        r#"{"method":"shared","params":{"method":"unlinkPreview","params":{"projectId":"p"}}}"#,
        r#"{"method":"shared","params":{"method":"scan","params":{"path":"/r"}}}"#,
        r#"{"method":"shared","params":{"method":"importProjects","params":{"path":"/r","dirs":["a","b"]}}}"#,
        r#"{"method":"shared","params":{"method":"sync","params":{"projectId":"p"}}}"#,
        r#"{"method":"shared","params":{"method":"syncRepo","params":{"repoId":"r"}}}"#,
        r#"{"method":"imports","params":{"method":"candidates","params":{"source":"tableplus","projectId":"p"}}}"#,
        r#"{"method":"imports","params":{"method":"candidates","params":{"source":"dbeaver","projectId":"p","path":"/f"}}}"#,
        r#"{"method":"imports","params":{"method":"create","params":{"source":"dbeaver","projectId":"p","keys":["k"]}}}"#,
        r#"{"method":"imports","params":{"method":"create","params":{"source":"tableplus","projectId":"p","keys":["id:1","pos:2"],"path":"/f"}}}"#,
    ] {
        let req = parse_request(text.as_bytes()).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert_eq!(serde_json::to_string(&req).unwrap(), text, "{text}");
    }
}

/// Every params object, each patch and both envelopes refuse a field they
/// don't know; `share` and `dirs` are required.
#[test]
fn unknown_request_fields_are_refused() {
    for bad in [
        r#"{"method":"shared","params":{"method":"reposList","params":{}}}"#,
        r#"{"method":"shared","params":{"method":"reposList"},"x":1}"#,
        r#"{"method":"shared","params":{"method":"reposList","x":1}}"#,
        r#"{"method":"shared","params":{"method":"repoRegister","params":{"path":"/r","x":1}}}"#,
        r#"{"method":"shared","params":{"method":"repoRegister","params":{"path":"/r","activeRepoId":"r"}}}"#,
        r#"{"method":"shared","params":{"method":"repoUpdate","params":{"id":"r","patch":{},"x":1}}}"#,
        r#"{"method":"shared","params":{"method":"repoUpdate","params":{"id":"r","patch":{"lastSyncAt":"t"}}}}"#,
        r#"{"method":"shared","params":{"method":"repoUpdate","params":{"id":"r","patch":{"syncStatus":"synced"}}}}"#,
        r#"{"method":"shared","params":{"method":"repoUpdate","params":{"id":"r","patch":{"path":"/x"}}}}"#,
        r#"{"method":"shared","params":{"method":"repoRemove","params":{"id":"r","x":1}}}"#,
        r#"{"method":"shared","params":{"method":"linkProject","params":{"projectId":"p","path":"/r","share":[],"x":1}}}"#,
        r#"{"method":"shared","params":{"method":"linkProject","params":{"projectId":"p","path":"/r"}}}"#,
        r#"{"method":"shared","params":{"method":"unlinkProject","params":{"projectId":"p","removeImported":true,"x":1}}}"#,
        r#"{"method":"shared","params":{"method":"unlinkPreview","params":{"projectId":"p","x":1}}}"#,
        r#"{"method":"shared","params":{"method":"unlinkProject","params":{"projectId":"p"}}}"#,
        r#"{"method":"shared","params":{"method":"scan","params":{"path":"/r","x":1}}}"#,
        r#"{"method":"shared","params":{"method":"importProjects","params":{"path":"/r","dirs":[],"x":1}}}"#,
        r#"{"method":"shared","params":{"method":"importProjects","params":{"path":"/r"}}}"#,
        r#"{"method":"shared","params":{"method":"sync","params":{"projectId":"p","x":1}}}"#,
        r#"{"method":"shared","params":{"method":"sync","params":{"repoId":"r"}}}"#,
        r#"{"method":"shared","params":{"method":"syncRepo","params":{"repoId":"r","x":1}}}"#,
        r#"{"method":"shared","params":{"method":"pull","params":{"path":"/r"}}}"#,
        r#"{"method":"imports","params":{"method":"candidates","params":{"source":"dbeaver","projectId":"p","x":1}}}"#,
        r#"{"method":"imports","params":{"method":"candidates","params":{"source":"navicat","projectId":"p"}}}"#,
        r#"{"method":"imports","params":{"method":"create","params":{"source":"dbeaver","projectId":"p","keys":[],"x":1}}}"#,
        r#"{"method":"imports","params":{"method":"create","params":{"source":"dbeaver","projectId":"p"}}}"#,
        r#"{"method":"imports","params":{"method":"candidates"},"x":1}"#,
    ] {
        let err = parse_request(bad.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{bad}");
    }
}

/// `sharedReposLoadAll` and `sharedReposSaveAll` left the storage group;
/// the `shared` group's repo calls replace them.
#[test]
fn a_retired_storage_method_is_unknown() {
    for (method, params) in [
        ("sharedReposLoadAll", Json::Null),
        (
            "sharedReposSaveAll",
            json!({"repos": [], "activeRepoId": null}),
        ),
    ] {
        let inner = if params.is_null() {
            json!({"method": method})
        } else {
            json!({"method": method, "params": params})
        };
        let body = json!({"method": "storage", "params": inner}).to_string();
        let err = parse_request(body.as_bytes()).unwrap_err();
        assert_eq!(err.code, "INVALID_ARGUMENT", "{method}");
        assert!(
            err.message.contains("unknown variant"),
            "{method}: {}",
            err.message
        );
    }
}

/// No `Debug` of a new request shows a path, a name, a key, a directory or
/// an id list.
#[test]
fn debug_redacts_every_new_params_type() {
    let bodies = [
        json!({"method": "shared", "params": {"method": "repoRegister", "params": {
            "path": "/home/canary/repo", "name": "canary-name", "remoteUrl": "git@canary:x"}}}),
        json!({"method": "shared", "params": {"method": "repoUpdate", "params": {"id": "r",
            "patch": {"name": "canary-name", "remoteUrl": "https://canary", "branch": "canary-b"}}}}),
        json!({"method": "shared", "params": {"method": "linkProject", "params": {
            "projectId": "p", "path": "/home/canary/repo", "share": ["c"]}}}),
        json!({"method": "shared", "params": {"method": "scan", "params": {
            "path": "/home/canary/repo"}}}),
        json!({"method": "shared", "params": {"method": "importProjects", "params": {
            "path": "/home/canary/repo", "dirs": ["canary-dir"]}}}),
        json!({"method": "imports", "params": {"method": "candidates", "params": {
            "source": "dbeaver", "projectId": "p", "path": "/home/canary/data-sources.json"}}}),
        json!({"method": "imports", "params": {"method": "create", "params": {
            "source": "tableplus", "projectId": "p", "keys": ["id:canary-key"],
            "path": "/home/canary/Connections.plist"}}}),
    ];
    for body in bodies {
        let req = parse_request(body.to_string().as_bytes()).unwrap();
        let text = format!("{req:?} {req:#?}");
        assert!(!text.contains("canary"), "{text}");
    }
}

/// No response's `Debug` shows a repo's path or name, a directory or a
/// candidate's host.
#[tokio::test]
async fn responses_debug_shows_no_values() {
    let env = env(true).await;
    let repo = env.repo("canary-repo");
    let project = env.project("Canary").await;
    let file = env.dbeaver_file();
    for body in [
        json!({"method": "shared", "params": {"method": "repoRegister", "params": {
            "path": repo, "name": "canary-name"}}}),
        json!({"method": "shared", "params": {"method": "reposList"}}),
        json!({"method": "shared", "params": {"method": "linkProject", "params": {
            "projectId": project, "path": repo, "share": []}}}),
        json!({"method": "shared", "params": {"method": "scan", "params": {"path": repo}}}),
        json!({"method": "imports", "params": {"method": "candidates", "params": {
            "source": "dbeaver", "projectId": project, "path": file}}}),
    ] {
        let req = parse_request(body.to_string().as_bytes()).unwrap();
        let res = dispatch_workspace(&env.core, &env.ws, req, WriteOrigin::none())
            .await
            .unwrap();
        let text = format!("{res:?} {res:#?}");
        for canary in ["canary", "Canary", "db.example.com", "Shop"] {
            assert!(!text.contains(canary), "{text}");
        }
    }
}

/// `reposList` answers each stored repo as its stored text, so fields an
/// older release wrote survive byte for byte.
#[tokio::test]
async fn repos_list_keeps_stored_bytes() {
    let env = env(true).await;
    let repo = r#"{"id":"r1","port":2.2e1,"name":"b","enabled":true,"a":{"z":-0.0}}"#;
    let raw = serde_json::value::RawValue::from_string(repo.to_string()).unwrap();
    seaquel_core::storage::shared_repos::save_all(env.ws.storage(), &[raw], None)
        .await
        .unwrap();
    let body = r#"{"method":"shared","params":{"method":"reposList"}}"#;
    let req = parse_request(body.as_bytes()).unwrap();
    let res = dispatch_workspace(&env.core, &env.ws, req, WriteOrigin::none())
        .await
        .unwrap();
    let text = serde_json::to_string(&res).unwrap();
    assert!(text.contains(&format!(r#""value":[{repo}]"#)), "{text}");
}

/// A repo write crosses as `storageChanged` of kind `sharedRepo` with the
/// writer's origin and the repo's id.
#[tokio::test]
async fn a_repo_write_crosses_as_storage_changed_with_its_origin() {
    let env = env(true).await;
    let mut events = workspace_events(&env.ws);
    let repo = env.repo("r");
    let registered = env
        .shared("repoRegister", json!({"path": repo}))
        .await
        .unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(event).unwrap(),
        json!({"type": "storageChanged", "kind": "sharedRepo", "scope": null,
            "ids": [registered["value"]["id"]], "origin": WINDOW, "seq": registered["seq"]})
    );
}

// ── The git group under the repo lock ──

async fn git_call(
    core: &Core,
    ws: Option<&Workspace>,
    git: &Git,
    method: &str,
    params: Json,
) -> Result<Json, RpcError> {
    let body = json!({"method": "git", "params": {"method": method, "params": params}});
    let Request::Git(req) = parse_request(body.to_string().as_bytes())? else {
        unreachable!()
    };
    let res = dispatch_git(core, ws, git, req, &WriteOrigin::new(Some(WINDOW))).await?;
    Ok(serde_json::to_value(res).unwrap())
}

/// Pull, push, commit and conflict resolution wait for the repo's lock,
/// with a workspace and without one (the desktop before
/// storage opens); status doesn't.
#[tokio::test]
async fn git_calls_that_change_the_tree_wait_for_the_repo_lock() {
    let env = env(true).await;
    let git = Git::new(Some(env.dir.path().join("home")));
    let repo = env.repo("locked");
    std::fs::write(Path::new(&repo).join("a.txt"), "a").unwrap();
    for ws in [Some(&*env.ws), None] {
        for (method, params) in [
            ("pull", json!({"path": repo})),
            ("push", json!({"path": repo})),
            ("commit", json!({"path": repo, "message": "m"})),
            (
                "resolveConflict",
                json!({"path": repo, "filePath": "a.txt", "resolution": "ours"}),
            ),
        ] {
            let lock = env.core.repo_lock(Path::new(&repo)).await;
            let call = git_call(&env.core, ws, &git, method, params);
            tokio::pin!(call);
            assert!(
                tokio::time::timeout(Duration::from_millis(300), &mut call)
                    .await
                    .is_err(),
                "{method} ran while the repo was locked"
            );
            drop(lock);
            // It finishes once the lock is free, whatever git answers.
            let _ = tokio::time::timeout(Duration::from_secs(30), call)
                .await
                .unwrap_or_else(|_| panic!("{method} never finished"));
        }
        // A status doesn't wait.
        let lock = env.core.repo_lock(Path::new(&repo)).await;
        tokio::time::timeout(
            Duration::from_secs(30),
            git_call(&env.core, ws, &git, "status", json!({"path": repo})),
        )
        .await
        .expect("status waited for the lock")
        .unwrap();
        drop(lock);
    }
}

/// With a workspace, a successful push sets the repo's `lastSyncAt`,
/// through Core.
#[tokio::test]
async fn a_push_through_the_workspace_records_last_sync() {
    let env = env(true).await;
    let git = Git::new(Some(env.dir.path().join("home")));
    let origin = env.dir.path().join("origin.git");
    git2::Repository::init_bare(&origin).unwrap();
    let repo = env.repo("pushed");
    let url = format!("file://{}", origin.display());
    git_call(
        &env.core,
        None,
        &git,
        "setRemote",
        json!({"path": repo, "url": url}),
    )
    .await
    .unwrap();
    std::fs::write(Path::new(&repo).join("a.txt"), "a").unwrap();
    git_call(
        &env.core,
        Some(&env.ws),
        &git,
        "commit",
        json!({"path": repo, "message": "first"}),
    )
    .await
    .unwrap();
    let registered = env
        .shared("repoRegister", json!({"path": repo}))
        .await
        .unwrap();
    assert!(
        registered["value"].get("lastSyncAt").is_none()
            || registered["value"]["lastSyncAt"].is_null()
    );
    let pushed = git_call(
        &env.core,
        Some(&env.ws),
        &git,
        "push",
        json!({"path": repo}),
    )
    .await
    .unwrap();
    assert_eq!(pushed["result"]["success"], true, "{pushed}");
    let list = env.shared("reposList", Json::Null).await.unwrap();
    assert!(list["value"][0]["lastSyncAt"].is_string(), "{list}");
}

/// The new groups' deepest calls (link, sync, a repo's sync, an import of
/// projects, the imports' create, a pull through the workspace) fit a
/// 2 MiB worker with 768 KiB of it taken first, as the library's calls do
/// (`library_calls_fit_a_2_mib_stack` in tests/state.rs). A stack
/// overflow aborts this test binary.
#[test]
fn shared_calls_fit_a_2_mib_stack() {
    #[inline(never)]
    fn reserved<R>(f: impl FnOnce() -> R) -> R {
        let pad = [0u8; 768 * 1024];
        std::hint::black_box(&pad);
        let r = f();
        std::hint::black_box(&pad);
        r
    }
    std::thread::Builder::new()
        .stack_size(2 * 1024 * 1024)
        .spawn(|| {
            reserved(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(deep_calls())
            })
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn deep_calls() {
    let env = env(true).await;
    let repo = env.repo("deep");
    let project = env.project("Deep").await;
    let conn = env
        .lib(
            "connectionCreate",
            json!({"connection": {"projectId": project, "name": "C", "type": "postgres",
                "host": "h", "port": 5432, "databaseName": "d", "username": "u"}}),
        )
        .await
        .unwrap()["value"]["id"]
        .clone();
    env.shared(
        "linkProject",
        json!({"projectId": project, "path": repo, "share": [conn]}),
    )
    .await
    .unwrap();
    let queries = Path::new(&repo).join(".seaquel/projects/deep/queries");
    std::fs::create_dir_all(&queries).unwrap();
    std::fs::write(
        queries.join("teammate.sql"),
        "---\nname: Teammate\n---\nSELECT 2\n",
    )
    .unwrap();
    let synced = env
        .shared("sync", json!({"projectId": project}))
        .await
        .unwrap();
    assert_eq!(synced["value"]["rowsChanged"], 1, "{synced}");
    let repo_id = env.shared("reposList", Json::Null).await.unwrap()["value"][0]["id"].clone();
    env.shared("syncRepo", json!({"repoId": repo_id}))
        .await
        .unwrap();
    env.shared("importProjects", json!({"path": repo, "dirs": ["deep"]}))
        .await
        .unwrap();
    let file = env.dbeaver_file();
    env.imports(
        "create",
        json!({"source": "dbeaver", "projectId": project, "keys": ["pg-1"], "path": file}),
    )
    .await
    .unwrap();
    let git = Git::new(Some(env.dir.path().join("home")));
    let _ = git_call(
        &env.core,
        Some(&env.ws),
        &git,
        "pull",
        json!({"path": repo}),
    )
    .await;
}

/// Without a workspace the git calls key the lock as the
/// workspace route does (`repo_path_key`), so a path with a trailing
/// separator waits for the same lock. A `/` alone makes no difference to
/// a `PathBuf` key, so the separator here is a `\`, which on Unix is part
/// of the name unless `repo_path_key` trims it; the folder doesn't exist,
/// so the lock's canonical form can't hide the difference.
#[tokio::test]
async fn the_plain_route_keys_the_lock_like_the_workspace_route() {
    let env = env(false).await;
    let git = Git::new(Some(env.dir.path().join("home")));
    let repo = env.dir.path().join("gone").to_string_lossy().into_owned();
    let lock = env.core.repo_lock(Path::new(&repo)).await;
    let call = git_call(
        &env.core,
        None,
        &git,
        "pull",
        json!({"path": format!("{repo}\\")}),
    );
    tokio::pin!(call);
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut call)
            .await
            .is_err(),
        "the pull ran while the repo was locked"
    );
    drop(lock);
    let _ = tokio::time::timeout(Duration::from_secs(30), call)
        .await
        .expect("the pull never finished");
}

/// The settings dialog clears a remote by saving an empty URL: `remoteUrl:
/// ""` stores `""` (the row's field is a required string, which older
/// releases read).
#[tokio::test]
async fn an_empty_remote_url_clears_it() {
    let env = env(true).await;
    let repo = env.repo("remote");
    let registered = env
        .shared(
            "repoRegister",
            json!({"path": repo, "remoteUrl": "git@example:x"}),
        )
        .await
        .unwrap();
    let id = registered["value"]["id"].clone();
    let updated = env
        .shared("repoUpdate", json!({"id": id, "patch": {"remoteUrl": ""}}))
        .await
        .unwrap();
    assert_eq!(updated["value"]["remoteUrl"], "", "{updated}");
}
