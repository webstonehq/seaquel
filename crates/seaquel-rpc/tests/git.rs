//! The `git` group: bodies parsed from bytes, dispatched with `dispatch_git`
//! onto repositories in a temp dir, and the wire shapes of the results.
//!
//! libgit2's global, XDG and system config search paths point at an empty
//! temp dir before any repository is touched, so the user's git config is
//! never read.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use git2::ConfigLevel;
use seaquel_core::git::Git;
use seaquel_core::{Core, WorkspaceSpec};
use seaquel_rpc::{dispatch_git, dispatch_workspace, parse_request, Request, RpcError};
use serde_json::{json, Value as Json};
use tempfile::TempDir;

fn setup() {
    static EMPTY_CONFIG: OnceLock<TempDir> = OnceLock::new();
    EMPTY_CONFIG.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        for level in [
            ConfigLevel::Global,
            ConfigLevel::XDG,
            ConfigLevel::System,
            ConfigLevel::ProgramData,
        ] {
            // SAFETY: once, before any other libgit2 call in this process.
            unsafe { git2::opts::set_search_path(level, dir.path()).unwrap() };
        }
        dir
    });
}

/// Parse `body` as the interfaces do, route the `git` group to
/// `dispatch_git`, and give back the JSON the interfaces send.
async fn call(git: &Git, body: Json) -> Result<Json, RpcError> {
    let Request::Git(req) = parse_request(body.to_string().as_bytes())? else {
        panic!("not a git request: {body}");
    };
    let res = dispatch_git(git, req).await?;
    Ok(serde_json::to_value(&res).unwrap())
}

async fn git_call(git: &Git, method: &str, params: Json) -> Result<Json, RpcError> {
    let res = call(
        git,
        json!({"method": "git", "params": {"method": method, "params": params}}),
    )
    .await?;
    assert_eq!(res["method"], method, "{res}");
    Ok(res["result"].clone())
}

fn path_str(p: &Path) -> String {
    p.to_str().unwrap().to_string()
}

#[tokio::test]
async fn init_commit_push_status_through_the_rpc() {
    setup();
    let dir = tempfile::tempdir().unwrap();
    let git = Git::new(Some(dir.path().join("home")));
    let origin = dir.path().join("origin.git");
    git2::Repository::init_bare(&origin).unwrap();
    let repo = path_str(&dir.path().join("repo"));
    let origin_url = format!("file://{}", origin.display());

    assert_eq!(
        git_call(&git, "init", json!({"path": repo})).await.unwrap(),
        Json::Null
    );
    assert_eq!(
        git_call(&git, "remoteUrl", json!({"path": repo}))
            .await
            .unwrap(),
        Json::Null
    );
    assert_eq!(
        git_call(&git, "setRemote", json!({"path": repo, "url": origin_url}))
            .await
            .unwrap(),
        Json::Null
    );
    assert_eq!(
        git_call(&git, "remoteUrl", json!({"path": repo}))
            .await
            .unwrap(),
        json!(origin_url)
    );

    std::fs::write(dir.path().join("repo/q.sql"), "select 1;\n").unwrap();
    let status = git_call(&git, "status", json!({"path": repo}))
        .await
        .unwrap();
    // Snake_case, as the Tauri command sent it.
    assert_eq!(
        status,
        json!({
            "is_clean": false,
            "pending_changes": 1,
            "ahead_by": 0,
            "behind_by": 0,
            "has_conflicts": false,
            "current_branch": "main",
            "modified_files": [],
            "untracked_files": ["q.sql"],
            "conflict_files": [],
        })
    );

    let id = git_call(&git, "commit", json!({"path": repo, "message": "Add q"}))
        .await
        .unwrap();
    assert_eq!(id.as_str().unwrap().len(), 40);

    // No credentials key at all, then an explicit null.
    let pushed = git_call(&git, "push", json!({"path": repo})).await.unwrap();
    assert_eq!(
        pushed,
        json!({"success": true, "message": "Push successful", "conflicts": [], "files_changed": []})
    );
    let pulled = git_call(&git, "pull", json!({"path": repo, "credentials": null}))
        .await
        .unwrap();
    assert_eq!(pulled["message"], "Already up to date");

    let clone = path_str(&dir.path().join("clone"));
    let creds =
        json!({"username": null, "password": null, "ssh_key_path": null, "ssh_passphrase": null});
    assert_eq!(
        git_call(
            &git,
            "clone",
            json!({"url": origin_url, "path": clone, "credentials": creds})
        )
        .await
        .unwrap(),
        Json::Null
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("clone/q.sql")).unwrap(),
        "select 1;\n"
    );

    let content = git_call(
        &git,
        "conflictContent",
        json!({"path": clone, "filePath": "q.sql"}),
    )
    .await
    .unwrap();
    assert_eq!(content, json!({"base": "", "ours": "", "theirs": ""}));
}

#[tokio::test]
async fn errors_keep_the_git_codes() {
    setup();
    let dir = tempfile::tempdir().unwrap();
    let git = Git::new(None);
    let err = git_call(&git, "status", json!({"path": path_str(dir.path())}))
        .await
        .unwrap_err();
    assert_eq!(err.code, "REPO_OPEN_ERROR");
    assert!(
        err.message.starts_with("Failed to open repository: "),
        "{err:?}"
    );

    let err = git_call(&git, "status", json!({})).await.unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

#[tokio::test]
async fn a_workspace_without_git_answers_not_supported() {
    let dir = tempfile::tempdir().unwrap();
    let core = Core::builder().build();
    let ws: Arc<_> = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let req = parse_request(
        br#"{"method":"git","params":{"method":"status","params":{"path":"/nowhere"}}}"#,
    )
    .unwrap();
    let err = dispatch_workspace(&core, &ws, req, seaquel_rpc::WriteOrigin::none())
        .await
        .unwrap_err();
    assert_eq!(err.code, "NOT_SUPPORTED");
}

// ── Wire snapshot ──

#[test]
fn git_request_snapshot() {
    let text = r#"{"method":"git","params":{"method":"resolveConflict","params":{"path":"/r","filePath":"q.sql","resolution":"x\n"}}}"#;
    let req = parse_request(text.as_bytes()).unwrap();
    assert_eq!((req.group(), req.method()), ("git", "resolveConflict"));
    assert_eq!(serde_json::to_string(&req).unwrap(), text);

    // Credentials go out without the key when unset.
    let req = Request::Git(seaquel_rpc::GitRequest::Pull {
        path: "/r".into(),
        credentials: None,
    });
    assert_eq!(
        serde_json::to_string(&req).unwrap(),
        r#"{"method":"git","params":{"method":"pull","params":{"path":"/r"}}}"#
    );

    let res = seaquel_rpc::Response::Git(seaquel_rpc::GitResponse::Commit("abc".into()));
    assert_eq!(
        serde_json::to_string(&res).unwrap(),
        r#"{"method":"git","result":{"method":"commit","result":"abc"}}"#
    );
}

// ── Credentials never reach the log ──

const TOKEN: &str = "ghp_never-logged-0123456789";

static RECORDS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        struct Kvs(String);
        impl<'kvs> log::kv::VisitSource<'kvs> for Kvs {
            fn visit_pair(
                &mut self,
                key: log::kv::Key<'kvs>,
                value: log::kv::Value<'kvs>,
            ) -> Result<(), log::kv::Error> {
                self.0.push_str(&format!(" {key}={value}"));
                Ok(())
            }
        }
        let mut kvs = Kvs(String::new());
        record.key_values().visit(&mut kvs).unwrap();
        RECORDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("{} {}{}", record.target(), record.args(), kvs.0));
    }

    fn flush(&self) {}
}

#[tokio::test]
async fn dispatch_git_logs_the_method_and_never_the_credentials() {
    setup();
    log::set_logger(&Capture).unwrap();
    log::set_max_level(log::LevelFilter::Trace);

    let dir = tempfile::tempdir().unwrap();
    let git = Git::new(None);
    let creds = json!({"username": "alice", "password": TOKEN, "ssh_key_path": null, "ssh_passphrase": TOKEN});
    // A failing clone: logs its method and its error code.
    let missing = format!("file://{}", dir.path().join("missing.git").display());
    let err = git_call(
        &git,
        "clone",
        json!({"url": missing, "path": path_str(&dir.path().join("c")), "credentials": creds}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code, "CLONE_ERROR");

    let records = RECORDS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for record in &records {
        assert!(!record.contains(TOKEN), "{record}");
    }
    assert!(
        records.iter().any(|r| r.contains("group=git")
            && r.contains("method=clone")
            && r.contains("code=CLONE_ERROR")),
        "{records:#?}"
    );
}
