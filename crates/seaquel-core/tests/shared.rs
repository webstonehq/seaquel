//! The shared projection through Core (phase 5e, Task 5): the fixture
//! replay on real temp repos, and the tests of what the fixtures can't
//! record (real symlinks, git, the repo lock against the storage lock,
//! events, logs, the CLI's read-only workspace).

// The shared test helpers' clock reads the wall clock (native tests).
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
#![cfg(unix)]

mod common;
#[path = "shared/replay.rs"]
mod replay;
#[path = "shared/world.rs"]
mod world;

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use seaquel_core::domain::library::{ConnectionPatch, SavedQueryDraft, SavedQueryPatch};
use seaquel_core::domain::shared::{PublishStatus, SkipReason, SyncNotice};
use seaquel_core::{
    StorageChange, StoredKind, SyncTarget, WorkspaceEvent, WorkspaceSpec, WriteOrigin,
};
use serde_json::{json, Value};

use common::{capture_logs, dump, insert_rows, logged, T0};
use world::{commit_all, git, World};

const Q: &str = "/repos/a/.seaquel/projects/team/queries";
const C: &str = "/repos/a/.seaquel/projects/team/connections";

fn origin() -> WriteOrigin {
    WriteOrigin::new(Some("main"))
}

/// `p1` "Team" linked to `/repos/a` (a git repo with `project.yaml`, its
/// seed pushed to a bare origin) and `repo-a`; `p2` "Solo" unlinked.
async fn team(w: &World) -> String {
    w.put(
        "/repos/a/.seaquel/projects/team/project.yaml",
        "name: Team\n",
    );
    let repo = w.git_repo("a");
    let path = repo.to_string_lossy().into_owned();
    insert_rows(
        w.ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Team", "description": null, "git_repo_path": path,
                "created_at": T0, "updated_at": T0}),
            json!({"id": "p2", "name": "Solo", "description": null, "git_repo_path": null,
                "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    let data = json!({"id": "repo-a", "name": "team-repo", "path": path, "remoteUrl": "",
        "branch": "main", "lastSyncAt": null, "syncStatus": "synced"});
    insert_rows(
        w.ws.storage(),
        "shared_repos",
        &[json!({"id": "repo-a", "data": data.to_string()})],
    )
    .await;
    path
}

fn query_row(id: &str, name: &str, text: &str, shared: bool) -> Value {
    json!({"id": id, "project_id": "p1", "name": name, "query": text, "parameters": null,
        "starred": 0, "shared": i64::from(shared), "description": null, "database_type": null,
        "tags": null, "folder": null, "created_at": T0, "updated_at": T0})
}

fn connection_row(id: &str, name: &str, shared: Option<&str>) -> Value {
    json!({"id": id, "project_id": "p1", "name": name, "type": "postgres", "host": "db.internal",
        "port": 5432, "database_name": "app", "username": "alice", "ssl_mode": null,
        "connection_string": null, "last_connected": null, "ssh_tunnel": null,
        "save_password": 0, "save_ssh_password": 0, "save_ssh_key_passphrase": 0,
        "is_local_only": 0, "shared_connection_id": shared, "ai_share_schema": null,
        "ai_share_data": null, "active_ai_provider_id": null, "active_ai_model": null})
}

async fn sync(w: &World) -> seaquel_core::SyncReport {
    w.ws.shared_sync(&w.core, &origin(), SyncTarget::Project("p1".into()))
        .await
        .unwrap()
        .value
}

async fn row(w: &World, id: &str) -> Value {
    dump(w.ws.storage(), "saved_queries", "rowid")
        .await
        .into_iter()
        .find(|r| r["id"] == id)
        .unwrap_or(Value::Null)
}

async fn drain<S: futures::Stream<Item = WorkspaceEvent> + Unpin>(
    events: &mut S,
) -> Vec<StorageChange> {
    let mut out = Vec::new();
    while let Ok(Some(e)) = tokio::time::timeout(Duration::from_millis(50), events.next()).await {
        if let WorkspaceEvent::StorageChanged(c) = e {
            out.push(c);
        }
    }
    out
}

// ── Decision 32 ──

#[tokio::test(flavor = "multi_thread")]
async fn a_symlinked_file_is_skipped_and_a_symlinked_dir_refuses_writes() {
    let w = World::new().await;
    team(&w).await;
    w.put("/home/user/.ssh/id_ed25519", "PRIVATE KEY");
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    symlink(
        w.abs("/home/user/.ssh/id_ed25519"),
        w.abs(&format!("{Q}/x.sql")),
    )
    .unwrap();
    let report = sync(&w).await;
    let notices = serde_json::to_value(&report.notices).unwrap();
    assert_eq!(
        notices,
        json!([{"type": "skipped", "path": "projects/team/queries/x.sql", "why": "symlink"}])
    );
    let rows = dump(w.ws.storage(), "saved_queries", "rowid").await;
    assert_eq!(rows.len(), 1, "only orders.sql became a query");
    assert!(rows
        .iter()
        .all(|r| !r["query"].as_str().unwrap().contains("PRIVATE")));

    // A symlinked `dashboards/` directory: a shared dashboard's write is
    // refused, nothing outside the repo changes, and the row keeps no path.
    std::fs::create_dir_all(w.abs("/outside/dashboards")).unwrap();
    symlink(
        w.abs("/outside/dashboards"),
        w.abs("/repos/a/.seaquel/projects/team/dashboards"),
    )
    .unwrap();
    let made =
        w.ws.create_dashboard(
            &w.core,
            &origin(),
            serde_json::from_value(json!({"projectId": "p1", "name": "Sales", "shared": true,
                "widgets": [], "viewport": {"x": 0, "y": 0, "zoom": 1}}))
            .unwrap(),
        )
        .await
        .unwrap();
    let p = made.projection.unwrap();
    assert_eq!(p.status, PublishStatus::Failed);
    assert_eq!(p.code.as_deref(), Some("FILE_ERROR"));
    assert_eq!(
        std::fs::read_dir(w.abs("/outside/dashboards"))
            .unwrap()
            .count(),
        0
    );
    let d = dump(w.ws.storage(), "dashboards", "rowid").await;
    assert_eq!(d[0]["shared_path"], Value::Null, "a refusal stores nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_path_with_dot_dot_is_refused() {
    let w = World::new().await;
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    // A folder that can't be a path in the repo is refused before the
    // commit (Decision 32), on a shared query in a linked project.
    let err =
        w.ws.update_saved_query(
            &w.core,
            &origin(),
            "q1",
            serde_json::from_value(json!({"folder": "../../escape"})).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
    assert_eq!(row(&w, "q1").await["folder"], Value::Null);
    // A stored path that climbs out of the repo (a hand-edited file) is
    // never written to.
    sqlx::query("UPDATE saved_queries SET shared_path = ?1 WHERE id = 'q1'")
        .bind(".seaquel/projects/team/queries/../../../../escape.sql")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    let out =
        w.ws.update_saved_query(
            &w.core,
            &origin(),
            "q1",
            serde_json::from_value(json!({"query": "SELECT 2"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(out.projection.unwrap().status, PublishStatus::Failed);
    assert!(!w.root.join("escape.sql").exists());
    assert!(!w.root.parent().unwrap().join("escape.sql").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_past_16_mib_is_skipped_and_named() {
    let w = World::new().await;
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Big", "SELECT 1", true)],
    )
    .await;
    sqlx::query("UPDATE saved_queries SET shared_path = ?1 WHERE id = 'q1'")
        .bind(".seaquel/projects/team/queries/big.sql")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    let mut text = String::from("---\nname: Big\n---\n");
    text.push_str(&"x".repeat(16 * 1024 * 1024 + 1));
    w.put(&format!("{Q}/big.sql"), &text);
    let report = sync(&w).await;
    assert_eq!(
        serde_json::to_value(&report.notices).unwrap(),
        json!([{"type": "skipped", "path": "projects/team/queries/big.sql", "why": "tooLarge"}])
    );
    let q1 = row(&w, "q1").await;
    assert_eq!(q1["shared"], 1, "a skipped file is never missing");
    assert_eq!(q1["query"], "SELECT 1");
    assert_eq!(report.rows_changed, 0);
}

// ── Decision 35 ──

#[tokio::test(flavor = "multi_thread")]
async fn a_conflicted_repo_is_not_synced() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    // A teammate and the user change the same line: a real merge conflict.
    let mate = w.teammate("a", 1);
    std::fs::write(
        mate.join(".seaquel/projects/team/queries/orders.sql"),
        "---\nname: Orders\n---\nSELECT 'theirs'\n",
    )
    .unwrap();
    commit_all(&mate, "theirs");
    git(&mate, &["push", "-q", "origin", "main"]);
    let repo = w.abs("/repos/a");
    std::fs::write(
        repo.join(".seaquel/projects/team/queries/orders.sql"),
        "---\nname: Orders\n---\nSELECT 'mine'\n",
    )
    .unwrap();
    commit_all(&repo, "mine");
    let pulled =
        w.ws.shared_git_pull(
            &w.core,
            &origin(),
            &w.git_client(),
            &repo.to_string_lossy(),
            None,
        )
        .await
        .unwrap();
    assert!(!pulled.success, "{pulled:?}");
    let on_disk = w.read(&format!("{Q}/orders.sql")).unwrap();
    assert!(on_disk.contains("<<<<<<<"), "{on_disk}");
    let before = dump(w.ws.storage(), "saved_queries", "rowid").await;
    let report =
        w.ws.shared_sync(&w.core, &origin(), SyncTarget::Repo("repo-a".into()))
            .await
            .unwrap()
            .value;
    assert!(report.conflicted);
    assert_eq!(report.rows_changed + report.files_written, 0);
    assert!(report.notices.is_empty());
    assert_eq!(dump(w.ws.storage(), "saved_queries", "rowid").await, before);
    assert_eq!(w.read(&format!("{Q}/orders.sql")).unwrap(), on_disk);
    // A link onto a conflicted repo is refused.
    let err =
        w.ws.shared_link_project(&w.core, &origin(), "p2", &repo.to_string_lossy(), &[])
            .await
            .unwrap_err();
    assert_eq!(err.code, "REPO_CONFLICTED");
}

/// Probe fix 7: a project past the scan's bounds is named on every sync,
/// not once per session, and the report marks the project skipped (the
/// repo's sync too), so the GUI can show it.
#[tokio::test(flavor = "multi_thread")]
async fn a_project_past_the_bounds_is_named_on_every_sync() {
    let w = World::new().await;
    team(&w).await;
    let dir = w.abs(Q);
    std::fs::create_dir_all(&dir).unwrap();
    for i in 0..20_001 {
        std::fs::write(dir.join(format!("q{i}.sql")), "").unwrap();
    }
    let skipped = |r: &seaquel_core::SyncReport| {
        r.notices
            .iter()
            .filter(|n| {
                matches!(
                    n,
                    SyncNotice::Skipped {
                        why: SkipReason::TooMany,
                        ..
                    }
                )
            })
            .count()
    };
    for _ in 0..2 {
        let report = sync(&w).await;
        assert_eq!(skipped(&report), 1, "{report:?}");
        assert_eq!(report.skipped_projects.len(), 1, "{report:?}");
        assert_eq!(report.skipped_projects[0].project_id, "p1");
        assert_eq!(report.skipped_projects[0].why, SkipReason::TooMany);
    }
    let report =
        w.ws.shared_sync(&w.core, &origin(), SyncTarget::Repo("repo-a".into()))
            .await
            .unwrap()
            .value;
    assert_eq!(skipped(&report), 1, "{report:?}");
    assert_eq!(report.skipped_projects[0].project_id, "p1");

    // Back under the bounds: the next sync reports no skipped project.
    std::fs::remove_dir_all(&dir).unwrap();
    let report = sync(&w).await;
    assert!(report.skipped_projects.is_empty(), "{report:?}");
}

/// Probe fix 5 in Core: resolving a conflict by keeping a deletion, under
/// the repo lock, deletes the file.
#[tokio::test(flavor = "multi_thread")]
async fn resolving_by_deletion_deletes_the_file() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    let mate = w.teammate("a", 1);
    std::fs::remove_file(mate.join(".seaquel/projects/team/queries/orders.sql")).unwrap();
    commit_all(&mate, "theirs");
    git(&mate, &["push", "-q", "origin", "main"]);
    let repo = w.abs("/repos/a");
    std::fs::write(
        repo.join(".seaquel/projects/team/queries/orders.sql"),
        "---\nname: Orders\n---\nSELECT 'mine'\n",
    )
    .unwrap();
    commit_all(&repo, "mine");
    let path = repo.to_string_lossy().into_owned();
    let pulled =
        w.ws.shared_git_pull(&w.core, &origin(), &w.git_client(), &path, None)
            .await
            .unwrap();
    assert!(!pulled.success);
    w.ws.shared_git_resolve(
        &w.core,
        &w.git_client(),
        &path,
        ".seaquel/projects/team/queries/orders.sql",
        None,
    )
    .await
    .unwrap();
    assert!(w.read(&format!("{Q}/orders.sql")).is_none());
    let report = sync(&w).await;
    assert!(!report.conflicted, "{report:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_directory_unshares_nothing() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    let dir = w.abs(Q);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let report = sync(&w).await;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        serde_json::to_value(&report.notices).unwrap(),
        json!([{"type": "skipped", "path": "projects/team/queries", "why": "unreadable"}])
    );
    let q1 = row(&w, "q1").await;
    assert_eq!(q1["shared"], 1);
    assert_eq!(
        q1["shared_path"],
        ".seaquel/projects/team/queries/orders.sql"
    );
}

// ── Decisions 36 and 37 ──

#[tokio::test(flavor = "multi_thread")]
async fn publish_writes_the_row_read_under_the_lock() {
    let w = Arc::new(World::new().await);
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    let repo = team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    // Hold the repo lock: both updates commit, and their publishes wait.
    let lock = w.core.repo_lock(Path::new(&repo)).await;
    let edit = |text: &'static str| {
        let w = Arc::clone(&w);
        tokio::spawn(async move {
            w.ws.update_saved_query(
                &w.core,
                &origin(),
                "q1",
                serde_json::from_value(json!({"query": text})).unwrap(),
            )
            .await
            .unwrap()
        })
    };
    let a = edit("SELECT 'a'");
    while row(&w, "q1").await["query"] != "SELECT 'a'" {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let b = edit("SELECT 'b'");
    while row(&w, "q1").await["query"] != "SELECT 'b'" {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    drop(lock);
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    let file = w.read(&format!("{Q}/orders.sql")).unwrap();
    assert!(file.ends_with("SELECT 'b'\n"), "{file}");
    // Whichever publish ran first wrote the later commit; the other had
    // nothing left to write.
    let written = [a.projection, b.projection]
        .into_iter()
        .flatten()
        .filter(|p| p.status == PublishStatus::Written)
        .count();
    assert_eq!(written, 1);
    assert!(replay_invariants_hold(&w).await);
}

/// The base and file id of every linked query agree with its file.
async fn replay_invariants_hold(w: &World) -> bool {
    for r in dump(w.ws.storage(), "saved_queries", "rowid").await {
        let Some(path) = r["shared_path"].as_str() else {
            continue;
        };
        let Some(text) = w.read(&format!("/repos/a/{path}")) else {
            continue;
        };
        let (hash, id) = seaquel_core::domain::shared::file_hash(path, &text).unwrap();
        if r["shared_base"].as_str() != Some(hash.as_str())
            || r["shared_file_id"].as_str() != id.as_deref()
        {
            return false;
        }
    }
    true
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_publish_leaves_the_row_and_the_next_sync_writes_it() {
    let w = World::new().await;
    team(&w).await;
    let target = w.abs(&format!("{Q}/orders.sql"));
    w.hook.failing.lock().unwrap().insert(target.clone());
    let made =
        w.ws.create_saved_query(
            &w.core,
            &origin(),
            serde_json::from_value::<SavedQueryDraft>(
                json!({"projectId": "p1", "name": "Orders", "query": "SELECT 1", "shared": true}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let p = made.projection.clone().unwrap();
    assert_eq!(
        (p.status, p.code.as_deref()),
        (PublishStatus::Failed, Some("FILE_ERROR"))
    );
    assert!(!target.exists());
    // The row is stored, with its path and no base (a first share's
    // `on_failure`).
    let q = row(&w, &made.value.id).await;
    assert_eq!(q["shared"], 1);
    assert_eq!(
        q["shared_path"],
        ".seaquel/projects/team/queries/orders.sql"
    );
    assert_eq!(q["shared_base"], Value::Null);
    // The next sync writes it and records the base.
    w.hook.failing.lock().unwrap().clear();
    let report = sync(&w).await;
    assert_eq!(report.files_written, 1);
    let file = std::fs::read_to_string(&target).unwrap();
    assert!(file.contains("name: Orders\n") && file.ends_with("SELECT 1\n"));
    assert!(row(&w, &made.value.id).await["shared_base"].is_string());
    assert!(replay_invariants_hold(&w).await);
}

/// Review (replay gap): a local change the publish never wrote (here an
/// older release's edit, straight to the row: R ≠ B, F = B) is written by
/// the next sync, which then records the new base.
#[tokio::test(flavor = "multi_thread")]
async fn a_sync_writes_a_local_change() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    let base = row(&w, "q1").await["shared_base"].clone();
    sqlx::query("UPDATE saved_queries SET query = 'SELECT 42' WHERE id = 'q1'")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    let report = sync(&w).await;
    assert_eq!(report.files_written, 1, "{report:?}");
    let file = w.read(&format!("{Q}/orders.sql")).unwrap();
    assert!(file.ends_with("SELECT 42\n"), "{file}");
    let q1 = row(&w, "q1").await;
    assert_ne!(q1["shared_base"], base);
    assert_eq!(q1["query"], "SELECT 42");
    assert!(replay_invariants_hold(&w).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publish_over_a_teammates_change_syncs_instead() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    // A teammate's change lands on disk before any sync.
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 'theirs'\n",
    );
    let out =
        w.ws.update_saved_query(
            &w.core,
            &origin(),
            "q1",
            serde_json::from_value(json!({"query": "SELECT 'mine'"})).unwrap(),
        )
        .await
        .unwrap();
    let p = out.projection.unwrap();
    assert_eq!(p.code.as_deref(), Some("FILE_CHANGED"));
    assert!(w
        .read(&format!("{Q}/orders.sql"))
        .unwrap()
        .contains("SELECT 'theirs'"));
    assert_eq!(row(&w, "q1").await["query"], "SELECT 'theirs'");
    let versions = dump(w.ws.storage(), "query_versions", "rowid").await;
    assert!(versions.iter().any(|v| v["snapshot"] == "SELECT 'mine'"));
}

/// Review M3: a rename whose new file is written while the old one turns
/// out changed by a teammate (the delete is stale) takes the new file back,
/// so no two files share one id, and syncs: the teammate's file wins.
#[tokio::test(flavor = "multi_thread")]
async fn a_rename_over_a_teammates_change_leaves_one_file() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    // Core rewrites it once, so the file carries an id.
    w.ws.update_saved_query(
        &w.core,
        &origin(),
        "q1",
        serde_json::from_value(json!({"query": "SELECT 2"})).unwrap(),
    )
    .await
    .unwrap();
    let ours = w.read(&format!("{Q}/orders.sql")).unwrap();
    let theirs = ours.replace("SELECT 2", "SELECT 'theirs'");
    w.put(&format!("{Q}/orders.sql"), &theirs);
    let out =
        w.ws.update_saved_query(
            &w.core,
            &origin(),
            "q1",
            serde_json::from_value(json!({"name": "All orders"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        out.projection.unwrap().code.as_deref(),
        Some("FILE_CHANGED")
    );
    let files: Vec<String> = std::fs::read_dir(w.abs(Q))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, ["orders.sql"], "two files share one id");
    assert_eq!(w.read(&format!("{Q}/orders.sql")).unwrap(), theirs);
    let q1 = row(&w, "q1").await;
    assert_eq!(q1["query"], "SELECT 'theirs'");
    assert_eq!(
        q1["shared_path"],
        ".seaquel/projects/team/queries/orders.sql"
    );
    assert!(replay_invariants_hold(&w).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn unshare_restores_the_file_when_the_row_write_fails() {
    let w = World::new().await;
    let text = "---\nname: Orders\n---\nSELECT 1\n";
    w.put(&format!("{Q}/orders.sql"), text);
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[
            query_row("q1", "Orders", "SELECT 1", true),
            query_row("q2", "Totals", "SELECT 2", false),
        ],
    )
    .await;
    sync(&w).await;
    // Unshare and rename onto a taken name: the file goes first, then the
    // row write fails (`NAME_TAKEN`) and the file comes back.
    let err =
        w.ws.update_saved_query(
            &w.core,
            &origin(),
            "q1",
            serde_json::from_value(json!({"shared": false, "name": "Totals"})).unwrap(),
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "NAME_TAKEN");
    assert_eq!(w.read(&format!("{Q}/orders.sql")).as_deref(), Some(text));
    assert_eq!(row(&w, "q1").await["shared"], 1);
    // A removal whose row write succeeds deletes it for good.
    let out =
        w.ws.update_saved_query(
            &w.core,
            &origin(),
            "q1",
            serde_json::from_value(json!({"shared": false})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(out.projection.unwrap().status, PublishStatus::Deleted);
    assert_eq!(w.read(&format!("{Q}/orders.sql")), None);
    let q1 = row(&w, "q1").await;
    assert_eq!(
        (q1["shared"].clone(), q1["shared_path"].clone()),
        (json!(0), Value::Null)
    );
}

/// Review I2 (decision): a shared row whose file can't be deleted isn't
/// removed or unshared, so the next sync can't bring it back as a new row.
#[tokio::test(flavor = "multi_thread")]
async fn a_removal_whose_file_cant_be_deleted_is_refused() {
    let w = World::new().await;
    let text = "---\nname: Orders\n---\nSELECT 1\n";
    w.put(&format!("{Q}/orders.sql"), text);
    w.put(
        &format!("{Q}/totals.sql"),
        "---\nname: Totals\n---\nSELECT 2\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[
            query_row("q1", "Orders", "SELECT 1", true),
            query_row("q2", "Totals", "SELECT 2", true),
        ],
    )
    .await;
    sync(&w).await;
    let file = w.abs(&format!("{Q}/orders.sql"));
    w.hook.failing.lock().unwrap().insert(file.clone());
    let err =
        w.ws.remove_saved_query(&w.core, &origin(), "q1")
            .await
            .map(|_| ())
            .unwrap_err();
    assert_eq!(err.code, "FILE_ERROR");
    assert_eq!(row(&w, "q1").await["shared"], 1, "the row stays");
    let err =
        w.ws.update_saved_query(
            &w.core,
            &origin(),
            "q1",
            serde_json::from_value(json!({"shared": false})).unwrap(),
        )
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(err.code, "FILE_ERROR");
    assert_eq!(row(&w, "q1").await["shared"], 1);
    assert_eq!(w.read(&format!("{Q}/orders.sql")).as_deref(), Some(text));
    w.hook.failing.lock().unwrap().clear();
    // The project's link can't be read (a directory Decision 32 refuses):
    // a row with a link isn't removed either.
    sqlx::query("UPDATE projects SET shared_dir = '.hidden' WHERE id = 'p1'")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    let err =
        w.ws.remove_saved_query(&w.core, &origin(), "q2")
            .await
            .map(|_| ())
            .unwrap_err();
    assert_eq!(err.code, "FILE_ERROR");
    assert!(!row(&w, "q2").await.is_null());
    // Nothing comes back at the next sync once the link is readable.
    sqlx::query("UPDATE projects SET shared_dir = 'team' WHERE id = 'p1'")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    let report = sync(&w).await;
    assert_eq!(report.rows_changed, 0);
    assert_eq!(
        dump(w.ws.storage(), "saved_queries", "rowid").await.len(),
        2
    );
}

/// Review I3 (decision): a repo a project still links to can't be
/// forgotten, and a project whose repo row is gone gets it back under the
/// id its connections' template links carry, so no template is imported
/// twice.
#[tokio::test(flavor = "multi_thread")]
async fn a_repo_in_use_stays_and_a_lost_one_keeps_its_id() {
    let w = World::new().await;
    w.put(
        &format!("{C}/warehouse.yaml"),
        "name: Warehouse\ntype: postgres\nhost: db.internal\nport: 5432\ndatabaseName: app\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "connections",
        &[connection_row(
            "c1",
            "Warehouse",
            Some("repo-a:.seaquel/projects/team/connections/warehouse.yaml"),
        )],
    )
    .await;
    sync(&w).await;
    let err =
        w.ws.shared_repo_remove(&w.core, &origin(), "repo-a")
            .await
            .map(|_| ())
            .unwrap_err();
    assert_eq!(err.code, "REPO_IN_USE");
    assert_eq!(dump(w.ws.storage(), "shared_repos", "rowid").await.len(), 1);

    // An older release forgot the repo row: the next sync registers it
    // again as `repo-a`, and the template keeps its one connection.
    sqlx::query("DELETE FROM shared_repos")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    let report = sync(&w).await;
    assert_eq!(report.rows_changed, 0, "{report:?}");
    let repos = dump(w.ws.storage(), "shared_repos", "rowid").await;
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0]["id"], "repo-a");
    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    assert_eq!(conns.len(), 1, "the template was imported again");

    // Unlinked, it can go.
    w.ws.shared_unlink_project(&w.core, &origin(), "p1", true)
        .await
        .unwrap();
    w.ws.shared_repo_register(&w.core, &origin(), "/elsewhere", None, None)
        .await
        .unwrap();
    let id = dump(w.ws.storage(), "shared_repos", "rowid").await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    w.ws.shared_repo_remove(&w.core, &origin(), &id)
        .await
        .unwrap();
}

/// Re-review R8: a repo path is compared in its canonical form, so a
/// project stored with a trailing slash (or through a symlink) still finds
/// its repo row: no second registration, its repo's sync includes it, and
/// the repo can't be removed while it links to it.
#[tokio::test(flavor = "multi_thread")]
async fn repo_paths_compare_in_canonical_form() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    let repo = team(&w).await;
    // The project's row has a trailing slash; a second project reaches the
    // same repo through a symlink.
    sqlx::query("UPDATE projects SET git_repo_path = ?1 WHERE id = 'p1'")
        .bind(format!("{repo}/"))
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    let alias = w.abs("/alias");
    symlink(&repo, &alias).unwrap();
    insert_rows(
        w.ws.storage(),
        "projects",
        &[json!({"id": "p3", "name": "Team", "description": null,
            "git_repo_path": alias.to_string_lossy(), "created_at": T0, "updated_at": T0})],
    )
    .await;
    sync(&w).await;
    assert_eq!(
        dump(w.ws.storage(), "shared_repos", "rowid").await.len(),
        1,
        "the repo was registered twice"
    );
    let report =
        w.ws.shared_sync(&w.core, &origin(), SyncTarget::Repo("repo-a".into()))
            .await
            .unwrap()
            .value;
    assert!(report.failures.is_empty(), "{report:?}");
    let q = dump(w.ws.storage(), "saved_queries", "rowid").await;
    let synced: std::collections::BTreeSet<&str> = q
        .iter()
        .map(|r| r["project_id"].as_str().unwrap())
        .collect();
    assert_eq!(
        synced,
        ["p1", "p3"].into(),
        "the repo's sync missed a project"
    );
    let err =
        w.ws.shared_repo_remove(&w.core, &origin(), "repo-a")
            .await
            .map(|_| ())
            .unwrap_err();
    assert_eq!(err.code, "REPO_IN_USE");
    // Unlinking one keeps the repo: the other still uses it.
    let out =
        w.ws.shared_unlink_project(&w.core, &origin(), "p1", true)
            .await
            .unwrap()
            .value;
    assert!(!out.repo_removed);
    let preview =
        w.ws.shared_scan(&w.core, &format!("{repo}/"))
            .await
            .unwrap();
    assert_eq!(preview.projects[0].linked_project_ids, ["p3"]);
}

/// Probe fix 8: a symlinked project directory isn't offered for import,
/// and the scan's answer names it as skipped.
#[tokio::test(flavor = "multi_thread")]
async fn a_symlinked_project_dir_is_named_as_skipped() {
    let w = World::new().await;
    let path = team(&w).await;
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("project.yaml"), "name: Elsewhere\n").unwrap();
    symlink(
        outside.path(),
        w.abs("/repos/a/.seaquel/projects/elsewhere"),
    )
    .unwrap();
    let preview = w.ws.shared_scan(&w.core, &path).await.unwrap();
    let dirs: Vec<&str> = preview.projects.iter().map(|p| p.dir.as_str()).collect();
    assert_eq!(dirs, ["team"]);
    assert_eq!(preview.skipped_dirs.len(), 1, "{preview:?}");
    assert_eq!(preview.skipped_dirs[0].dir, "elsewhere");
    assert_eq!(preview.skipped_dirs[0].why, SkipReason::Symlink);
}

/// Probe fix 4: a large sync plans outside the storage's write lock, so an
/// unrelated library write isn't held for the sync's whole duration.
#[tokio::test(flavor = "multi_thread")]
async fn an_unrelated_write_isnt_held_by_a_large_sync() {
    let w = Arc::new(World::new().await);
    team(&w).await;
    for i in 0..6000 {
        w.put(
            &format!("{Q}/q{i:05}.sql"),
            &format!("---\nname: Q{i}\ndescription: query number {i}\n---\nSELECT {i}\n"),
        );
    }
    assert_eq!(sync(&w).await.rows_changed, 6000);
    // A teammate changes 50 of them: the next sync reads and plans over
    // every row (the probe's `syncRepo` case) and writes few.
    for i in 0..50 {
        w.put(
            &format!("{Q}/q{i:05}.sql"),
            &format!("---\nname: Q{i}\ndescription: query number {i}\n---\nSELECT {i} + 1\n"),
        );
    }
    let started = std::time::Instant::now();
    let syncing = {
        let w = Arc::clone(&w);
        tokio::spawn(async move { sync(&w).await })
    };
    // Give the sync time to scan and start planning.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let mut worst = Duration::ZERO;
    let mut n = 0;
    while !syncing.is_finished() {
        let t = std::time::Instant::now();
        w.ws.set_setting(
            &w.core,
            &origin(),
            "query_version_limit",
            Some(format!("{}", 50 + n)),
        )
        .await
        .unwrap();
        worst = worst.max(t.elapsed());
        n += 1;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let report = syncing.await.unwrap();
    let total = started.elapsed();
    assert_eq!(report.rows_changed, 50);
    eprintln!("sync {total:?}, worst unrelated write {worst:?} over {n} writes");
    assert!(n > 0, "the sync finished before any write was tried");
    assert!(
        worst * 3 < total,
        "a write waited {worst:?} during a {total:?} sync"
    );
}

/// Probe fix 4: an edit that lands between a sync's plan and its write is
/// never overwritten by the stale plan. The sync plans again and sees it:
/// the file still wins (Q20), but as a named conflict with the edit in the
/// history, not as a silent update.
#[tokio::test(flavor = "multi_thread")]
async fn a_racing_edit_is_never_overwritten_by_a_stale_plan() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let go = Arc::new(tokio::sync::Notify::new());
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let hook: seaquel_core::SyncPlanHook = {
        let (gate, go, armed) = (gate.clone(), go.clone(), armed.clone());
        Arc::new(move || {
            let (gate, go, armed) = (gate.clone(), go.clone(), armed.clone());
            Box::pin(async move {
                if armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    gate.notify_one();
                    go.notified().await;
                }
            })
        })
    };
    let w = Arc::new(World::with_plan_hook(hook).await);
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    // A teammate's change: the next sync plans to update the row from the
    // file (R = B, F ≠ B).
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 'theirs'\n",
    );
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let syncing = {
        let w = Arc::clone(&w);
        tokio::spawn(async move { sync(&w).await })
    };
    gate.notified().await;
    // The user's edit lands after the plan, before its write. Its own
    // publish waits for the repo lock the sync holds, so it runs detached.
    let editing = {
        let w = Arc::clone(&w);
        tokio::spawn(async move {
            w.ws.update_saved_query(
                &w.core,
                &origin(),
                "q1",
                serde_json::from_value(json!({"query": "SELECT 'mine'"})).unwrap(),
            )
            .await
            .unwrap()
        })
    };
    while row(&w, "q1").await["query"] != "SELECT 'mine'" {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    go.notify_one();
    let report = syncing.await.unwrap();
    editing.await.unwrap();
    let conflict = serde_json::to_value(&report.notices).unwrap();
    assert!(
        conflict.to_string().contains("\"conflict\""),
        "the stale plan overwrote the edit silently: {conflict}"
    );
    let versions = dump(w.ws.storage(), "query_versions", "rowid").await;
    assert!(
        versions.iter().any(|v| v["snapshot"] == "SELECT 'mine'"),
        "the edit was lost: {versions:?}"
    );
}

// ── Decision 38: the repo lock and the storage lock ──

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_waits_for_a_publish() {
    let w = Arc::new(World::new().await);
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    let repo = team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    commit_all(Path::new(&repo), "synced");
    git(Path::new(&repo), &["push", "-q", "origin", "main"]);
    let target = w.abs(&format!("{Q}/orders.sql"));
    w.hook.gate(&target);
    let publish = {
        let w = Arc::clone(&w);
        tokio::spawn(async move {
            w.ws.update_saved_query(
                &w.core,
                &origin(),
                "q1",
                serde_json::from_value(json!({"query": "SELECT 2"})).unwrap(),
            )
            .await
            .unwrap()
        })
    };
    let hook = Arc::clone(&w.hook);
    tokio::task::spawn_blocking(move || hook.wait_held())
        .await
        .unwrap();
    let pull = {
        let w = Arc::clone(&w);
        let repo = repo.clone();
        tokio::spawn(async move {
            w.ws.shared_git_pull(&w.core, &origin(), &w.git_client(), &repo, None)
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let ran_early = pull.is_finished();
    w.hook.release();
    assert!(!ran_early, "the pull ran while a publish held the repo");
    publish.await.unwrap();
    pull.await.unwrap().unwrap();
}

/// Review M1: the repo lock is keyed by the canonical path, so a symlink
/// to the repo (and on macOS another letter case) shares its lock.
#[tokio::test(flavor = "multi_thread")]
async fn two_spellings_of_a_repo_share_its_lock() {
    let w = Arc::new(World::new().await);
    let repo = w.abs("/repos/Team");
    std::fs::create_dir_all(&repo).unwrap();
    let alias = w.abs("/repos/alias");
    symlink(&repo, &alias).unwrap();
    let mut spellings = vec![alias];
    if cfg!(target_os = "macos") {
        spellings.push(w.abs("/repos/team"));
    }
    for other in spellings {
        let held = w.core.repo_lock(&repo).await;
        let waiter = {
            let w = Arc::clone(&w);
            tokio::spawn(async move {
                let _second = w.core.repo_lock(&other).await;
            })
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        let early = waiter.is_finished();
        drop(held);
        waiter.await.unwrap();
        assert!(!early, "a second spelling took the lock while it was held");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_file_io_inside_a_write_tx() {
    let w = Arc::new(World::new().await);
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    // A publish's file write blocks: another storage write still commits.
    let target = w.abs(&format!("{Q}/orders.sql"));
    w.hook.gate(&target);
    let publish = {
        let w = Arc::clone(&w);
        tokio::spawn(async move {
            w.ws.update_saved_query(
                &w.core,
                &origin(),
                "q1",
                serde_json::from_value(json!({"query": "SELECT 2"})).unwrap(),
            )
            .await
            .unwrap()
        })
    };
    let hook = Arc::clone(&w.hook);
    tokio::task::spawn_blocking(move || hook.wait_held())
        .await
        .unwrap();
    let other = tokio::time::timeout(
        Duration::from_secs(10),
        w.ws.create_saved_query(
            &w.core,
            &origin(),
            serde_json::from_value::<SavedQueryDraft>(
                json!({"projectId": "p2", "name": "Elsewhere", "query": "SELECT 9"}),
            )
            .unwrap(),
        ),
    )
    .await;
    w.hook.release();
    assert!(other.is_ok(), "a storage write waited for a file write");
    other.unwrap().unwrap();
    publish.await.unwrap();

    // The same during a sync's file write (a row whose share's write
    // failed, written by the sync).
    let pending = w.abs(&format!("{Q}/totals.sql"));
    w.hook.failing.lock().unwrap().insert(pending.clone());
    w.ws.create_saved_query(
        &w.core,
        &origin(),
        serde_json::from_value::<SavedQueryDraft>(
            json!({"projectId": "p1", "name": "Totals", "query": "SELECT 3", "shared": true}),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    w.hook.failing.lock().unwrap().clear();
    w.hook.gate(&pending);
    let syncing = {
        let w = Arc::clone(&w);
        tokio::spawn(async move { sync(&w).await })
    };
    let hook = Arc::clone(&w.hook);
    tokio::task::spawn_blocking(move || hook.wait_held())
        .await
        .unwrap();
    let other = tokio::time::timeout(
        Duration::from_secs(10),
        w.ws.set_setting(&w.core, &origin(), "query_version_limit", Some("50".into())),
    )
    .await;
    w.hook.release();
    assert!(
        other.is_ok(),
        "a storage write waited for a sync's file write"
    );
    assert_eq!(syncing.await.unwrap().files_written, 1);
}

// ── Decision 40, Q25 ──

#[tokio::test(flavor = "multi_thread")]
async fn link_exports_and_links_templates_without_duplicates() {
    let w = World::new().await;
    let repo = w.abs("/repos/b");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    insert_rows(
        w.ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Team", "description": null, "git_repo_path": null,
            "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    insert_rows(
        w.ws.storage(),
        "connections",
        &[
            connection_row("c1", "Warehouse", None),
            connection_row("c2", "Billing", None),
        ],
    )
    .await;
    let path = repo.to_string_lossy().into_owned();
    let report =
        w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &["c1".to_string()])
            .await
            .unwrap()
            .value;
    assert!(report.notices.is_empty(), "{:?}", report.notices);
    assert_eq!(
        w.read("/repos/b/.seaquel/projects/team/project.yaml")
            .as_deref(),
        Some("name: Team\n")
    );
    let tpl = w
        .read("/repos/b/.seaquel/projects/team/connections/warehouse.yaml")
        .unwrap();
    assert!(
        tpl.contains("host: db.internal") && !tpl.contains("alice"),
        "{tpl}"
    );
    assert!(w
        .read("/repos/b/.seaquel/projects/team/connections/billing.yaml")
        .is_none());
    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    assert_eq!(conns.len(), 2, "no \"Warehouse (2)\" (bug 6)");
    let c1 = conns.iter().find(|c| c["id"] == "c1").unwrap();
    let repo_id = dump(w.ws.storage(), "shared_repos", "rowid").await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        c1["shared_connection_id"],
        format!("{repo_id}:.seaquel/projects/team/connections/warehouse.yaml")
    );
    assert!(c1["shared_base"].is_string());
    let c2 = conns.iter().find(|c| c["id"] == "c2").unwrap();
    assert_eq!(c2["shared_connection_id"], Value::Null);
    // Syncing again changes nothing.
    let again = sync(&w).await;
    assert_eq!(again.rows_changed + again.files_written, 0);
    assert_eq!(dump(w.ws.storage(), "connections", "rowid").await.len(), 2);
    let p = dump(w.ws.storage(), "projects", "rowid").await;
    assert_eq!(p[0]["shared_dir"], "team");
    // A `share` id that isn't the project's is refused.
    let err =
        w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &["nope".to_string()])
            .await
            .unwrap_err();
    assert_eq!(err.code, "INVALID_ARGUMENT");
}

/// Review I4 (decision): a linked project can't be linked to another
/// repo before it's unlinked; linking it to its own repo again only
/// shares the connections ticked now.
#[tokio::test(flavor = "multi_thread")]
async fn relinking_is_refused_elsewhere_and_shares_only_on_the_same_repo() {
    let w = World::new().await;
    let repo = team(&w).await;
    insert_rows(
        w.ws.storage(),
        "connections",
        &[connection_row("c1", "Warehouse", None)],
    )
    .await;
    sync(&w).await;
    let other = w.abs("/repos/b");
    std::fs::create_dir_all(&other).unwrap();
    git(&other, &["init", "-q"]);
    let before = dump(w.ws.storage(), "projects", "rowid").await;
    let err =
        w.ws.shared_link_project(&w.core, &origin(), "p1", &other.to_string_lossy(), &[])
            .await
            .map(|_| ())
            .unwrap_err();
    assert_eq!(err.code, "PROJECT_ALREADY_LINKED");
    assert_eq!(dump(w.ws.storage(), "projects", "rowid").await, before);
    assert!(!other.join(".seaquel").exists());
    // The same repo again: nothing changes but the ticked connection.
    w.ws.shared_link_project(&w.core, &origin(), "p1", &repo, &["c1".to_string()])
        .await
        .unwrap();
    let p = dump(w.ws.storage(), "projects", "rowid").await;
    assert_eq!(p[0]["updated_at"], before[0]["updated_at"]);
    assert_eq!(p[0]["shared_dir"], before[0]["shared_dir"]);
    assert_eq!(dump(w.ws.storage(), "shared_repos", "rowid").await.len(), 1);
    assert!(w.read(&format!("{C}/warehouse.yaml")).is_some());
}

/// Review M5: a repo's sync goes on past a project that fails, and says
/// which; an import of several projects keeps the ones that worked and
/// takes back the one whose sync failed, with whatever it stored.
#[tokio::test(flavor = "multi_thread")]
async fn multi_project_calls_go_on_past_a_failure() {
    let limits = seaquel_core::LibraryLimits {
        max_saved_queries: Some(2),
        ..Default::default()
    };
    let w = World::with_limits(limits).await;
    for (dir, name, n) in [("ops", "Ops", 1), ("big", "Big", 3)] {
        w.put(
            &format!("/repos/a/.seaquel/projects/{dir}/project.yaml"),
            &format!("name: {name}\n"),
        );
        for i in 0..n {
            w.put(
                &format!("/repos/a/.seaquel/projects/{dir}/queries/q{i}.sql"),
                &format!("---\nname: Q{i}\n---\nSELECT {i}\n"),
            );
        }
    }
    let repo = w.git_repo("a").to_string_lossy().into_owned();
    let out =
        w.ws.shared_import_projects(
            &w.core,
            &origin(),
            &repo,
            &["big".to_string(), "ops".to_string()],
        )
        .await
        .unwrap()
        .value;
    assert_eq!(out.project_ids.len(), 1, "{out:?}");
    assert_eq!(out.failures.len(), 1);
    assert_eq!(out.failures[0].dir.as_deref(), Some("big"));
    assert_eq!(out.failures[0].code, "INVALID_ARGUMENT");
    let projects = dump(w.ws.storage(), "projects", "rowid").await;
    assert_eq!(projects.len(), 1, "the failed import left a project behind");
    assert_eq!(projects[0]["name"], "Ops");
    assert_eq!(
        dump(w.ws.storage(), "saved_queries", "rowid").await.len(),
        1
    );

    // Linked by hand to `big` too: the repo's sync syncs `ops` and names
    // `big`'s failure.
    insert_rows(
        w.ws.storage(),
        "projects",
        &[
            json!({"id": "pb", "name": "Big", "description": null, "git_repo_path": repo,
            "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    w.put(
        "/repos/a/.seaquel/projects/ops/queries/new.sql",
        "---\nname: New\n---\nSELECT 9\n",
    );
    let repo_id = dump(w.ws.storage(), "shared_repos", "rowid").await[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let report =
        w.ws.shared_sync(&w.core, &origin(), SyncTarget::Repo(repo_id))
            .await
            .unwrap()
            .value;
    assert_eq!(report.failures.len(), 1, "{report:?}");
    assert_eq!(report.failures[0].project_id.as_deref(), Some("pb"));
    assert_eq!(
        dump(w.ws.storage(), "saved_queries", "rowid").await.len(),
        2
    );
}

/// Re-review R3: an imported project is announced only once its sync
/// succeeded; one whose import is taken back is never announced, so no
/// other window learns of it and writes into a project about to go.
#[tokio::test(flavor = "multi_thread")]
async fn an_import_announces_its_project_only_after_its_sync() {
    let limits = seaquel_core::LibraryLimits {
        max_saved_queries: Some(2),
        ..Default::default()
    };
    let w = World::with_limits(limits).await;
    for (dir, name, n) in [("ops", "Ops", 1), ("big", "Big", 3)] {
        w.put(
            &format!("/repos/a/.seaquel/projects/{dir}/project.yaml"),
            &format!("name: {name}\n"),
        );
        for i in 0..n {
            w.put(
                &format!("/repos/a/.seaquel/projects/{dir}/queries/q{i}.sql"),
                &format!("---\nname: Q{i}\n---\nSELECT {i}\n"),
            );
        }
    }
    let repo = w.git_repo("a").to_string_lossy().into_owned();
    let mut events = w.ws.events();
    let out =
        w.ws.shared_import_projects(
            &w.core,
            &origin(),
            &repo,
            &["big".to_string(), "ops".to_string()],
        )
        .await
        .unwrap()
        .value;
    assert_eq!(out.project_ids.len(), 1);
    let ops = out.project_ids[0].clone();
    let got = drain(&mut events).await;
    let project_events: Vec<&StorageChange> = got
        .iter()
        .filter(|c| c.kind == StoredKind::Project)
        .collect();
    for c in &project_events {
        assert_eq!(
            c.ids.as_deref(),
            Some(&[ops.clone()][..]),
            "a project other than the imported one was announced: {got:?}"
        );
    }
    let announced = got
        .iter()
        .position(|c| c.kind == StoredKind::Project)
        .expect("the imported project was never announced");
    let last_row = got
        .iter()
        .rposition(|c| c.kind == StoredKind::SavedQuery && c.scope.as_deref() == Some(&ops))
        .expect("the imported project's rows");
    assert!(announced > last_row, "announced before its sync: {got:?}");
}

/// Re-review R5: the project's repo is read again once the repo lock is
/// held; a link that waited while the project was moved elsewhere is
/// refused and writes nothing into the old repo.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_rechecks_the_project_under_the_lock() {
    let w = Arc::new(World::new().await);
    let repo = team(&w).await;
    insert_rows(
        w.ws.storage(),
        "connections",
        &[connection_row("c1", "Warehouse", None)],
    )
    .await;
    sync(&w).await;
    let lock = w.core.repo_lock(Path::new(&repo)).await;
    let linking = {
        let w = Arc::clone(&w);
        let repo = repo.clone();
        tokio::spawn(async move {
            w.ws.shared_link_project(&w.core, &origin(), "p1", &repo, &["c1".to_string()])
                .await
                .map(|_| ())
        })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;
    sqlx::query("UPDATE projects SET git_repo_path = '/elsewhere' WHERE id = 'p1'")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    drop(lock);
    let err = linking.await.unwrap().unwrap_err();
    assert_eq!(err.code, "PROJECT_ALREADY_LINKED");
    assert!(w.read(&format!("{C}/warehouse.yaml")).is_none());
}

/// Probe fix 1: a relink adopts the user's own templates already in the
/// directory (by file id, then by name) instead of writing `<name>-2.yaml`,
/// so no file is added, nothing is `unpaired`, and a second install
/// imports each connection once.
#[tokio::test(flavor = "multi_thread")]
async fn a_relink_adopts_the_users_own_templates() {
    let w = World::new().await;
    let repo = w.abs("/repos/b");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    insert_rows(
        w.ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Team", "description": null, "git_repo_path": null,
            "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    insert_rows(
        w.ws.storage(),
        "connections",
        &[
            connection_row("c1", "Warehouse", None),
            connection_row("c2", "Billing", None),
        ],
    )
    .await;
    let path = repo.to_string_lossy().into_owned();
    let share = ["c1".to_string(), "c2".to_string()];
    w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &share)
        .await
        .unwrap();
    let tdir = repo.join(".seaquel/projects/team/connections");
    let files = |d: &Path| {
        let mut f: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        f.sort();
        f
    };
    let before = files(&tdir);
    assert_eq!(before, ["billing.yaml", "warehouse.yaml"]);
    // An older release rewrote one template and dropped its id: the name
    // still adopts it.
    let billing = tdir.join("billing.yaml");
    let text = std::fs::read_to_string(&billing).unwrap();
    let without_id: String = text
        .lines()
        .filter(|l| !l.starts_with("id: "))
        .map(|l| format!("{l}\n"))
        .collect();
    std::fs::write(&billing, without_id).unwrap();
    w.ws.shared_unlink_project(&w.core, &origin(), "p1", false)
        .await
        .unwrap();
    let report =
        w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &share)
            .await
            .unwrap()
            .value;
    assert_eq!(files(&tdir), before, "the relink wrote new templates");
    let notices = serde_json::to_value(&report.notices).unwrap();
    assert!(!notices.to_string().contains("unpaired"), "{notices}");
    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    assert_eq!(conns.len(), 2, "{conns:?}");
    for c in &conns {
        let link = c["shared_connection_id"].as_str().unwrap();
        assert!(
            link.ends_with(".yaml") && !link.contains("-2.yaml"),
            "{link}"
        );
        assert_eq!(c["shared_origin"], "exported");
    }

    // A second install imports the directory: one connection per template.
    let other = tempfile::tempdir().unwrap();
    let ws2 = w
        .core
        .open_workspace(WorkspaceSpec::new(other.path()))
        .await
        .unwrap();
    let out = ws2
        .shared_import_projects(&w.core, &origin(), &path, &["team".to_string()])
        .await
        .unwrap()
        .value;
    assert_eq!(out.project_ids.len(), 1);
    let names: Vec<String> = dump(ws2.storage(), "connections", "rowid")
        .await
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(sorted, ["Billing", "Warehouse"], "{names:?}");
}

/// Probe fix 8: importing a directory a local project already links is a
/// per-directory failure (`PROJECT_ALREADY_LINKED`), nothing stored; the
/// other directories asked for are still imported.
#[tokio::test(flavor = "multi_thread")]
async fn importing_an_already_linked_directory_is_refused() {
    let w = World::new().await;
    let path = team(&w).await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    w.put("/repos/a/.seaquel/projects/ops/project.yaml", "name: Ops\n");
    sync(&w).await;
    let projects_before = dump(w.ws.storage(), "projects", "rowid").await.len();
    let queries_before = dump(w.ws.storage(), "saved_queries", "rowid").await;

    // The path as typed with a trailing slash names the same repo.
    let out =
        w.ws.shared_import_projects(
            &w.core,
            &origin(),
            &format!("{path}/"),
            &["team".into(), "ops".into()],
        )
        .await
        .unwrap()
        .value;
    assert_eq!(out.project_ids.len(), 1, "{out:?}");
    assert_eq!(out.failures.len(), 1, "{out:?}");
    assert_eq!(out.failures[0].dir.as_deref(), Some("team"));
    assert_eq!(out.failures[0].code, "PROJECT_ALREADY_LINKED");
    assert_eq!(
        dump(w.ws.storage(), "projects", "rowid").await.len(),
        projects_before + 1
    );
    assert_eq!(
        dump(w.ws.storage(), "saved_queries", "rowid").await,
        queries_before
    );
}

/// Probe-fix review A1: a ticked connection adopted by name from a
/// teammate's template with other values takes no base, so Q27 applies:
/// the template wins, the notice lists the replaced values, and nothing of
/// the user's is pushed into the file.
#[tokio::test(flavor = "multi_thread")]
async fn adopting_a_teammates_template_by_name_pushes_nothing() {
    let w = World::new().await;
    w.put(
        "/repos/b/.seaquel/projects/team/project.yaml",
        "name: Team\n",
    );
    let template =
        "name: Billing\ntype: postgres\nhost: teammate.db\nport: 5432\ndatabaseName: app\n";
    w.put(
        "/repos/b/.seaquel/projects/team/connections/billing.yaml",
        template,
    );
    let repo = w.abs("/repos/b");
    git(&repo, &["init", "-q"]);
    insert_rows(
        w.ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Team", "description": null, "git_repo_path": null,
            "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    insert_rows(
        w.ws.storage(),
        "connections",
        &[connection_row("c2", "Billing", None)],
    )
    .await;
    let path = repo.to_string_lossy().into_owned();
    let report =
        w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &["c2".to_string()])
            .await
            .unwrap()
            .value;
    let tdir = repo.join(".seaquel/projects/team/connections");
    let mut files: Vec<String> = std::fs::read_dir(&tdir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["billing.yaml"]);
    assert_eq!(
        std::fs::read_to_string(tdir.join("billing.yaml")).unwrap(),
        template
    );
    let notices = serde_json::to_value(&report.notices).unwrap();
    let conflict = notices
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["type"] == "conflict" && n["id"] == "c2")
        .unwrap_or_else(|| panic!("no conflict notice: {notices}"));
    assert!(
        conflict["replaced"].to_string().contains("db.internal"),
        "{conflict}"
    );
    let c = &dump(w.ws.storage(), "connections", "rowid").await[0];
    assert_eq!(c["host"], "teammate.db");
}

/// A1 on the kept-file-id path: a teammate's edit to the template made
/// after the unlink isn't discarded by the relink.
#[tokio::test(flavor = "multi_thread")]
async fn a_relink_keeps_a_teammates_edit_made_after_the_unlink() {
    let w = World::new().await;
    let repo = w.abs("/repos/b");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    insert_rows(
        w.ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Team", "description": null, "git_repo_path": null,
            "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    insert_rows(
        w.ws.storage(),
        "connections",
        &[connection_row("c2", "Billing", None)],
    )
    .await;
    let path = repo.to_string_lossy().into_owned();
    let share = ["c2".to_string()];
    w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &share)
        .await
        .unwrap();
    w.ws.shared_unlink_project(&w.core, &origin(), "p1", false)
        .await
        .unwrap();
    let file = repo.join(".seaquel/projects/team/connections/billing.yaml");
    let edited = std::fs::read_to_string(&file)
        .unwrap()
        .replace("db.internal", "moved.db");
    std::fs::write(&file, &edited).unwrap();
    let report =
        w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &share)
            .await
            .unwrap()
            .value;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
    let c = &dump(w.ws.storage(), "connections", "rowid").await[0];
    assert_eq!(c["host"], "moved.db", "{report:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn import_projects_keeps_the_directory_under_a_taken_name() {
    let w = World::new().await;
    w.put(
        "/repos/a/.seaquel/projects/main/project.yaml",
        "name: Main\n",
    );
    w.put(
        "/repos/a/.seaquel/projects/main/queries/orders.sql",
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    let repo = w.git_repo("a");
    insert_rows(
        w.ws.storage(),
        "projects",
        &[
            json!({"id": "p1", "name": "Main", "description": null, "git_repo_path": null,
            "created_at": T0, "updated_at": T0}),
        ],
    )
    .await;
    let ids =
        w.ws.shared_import_projects(
            &w.core,
            &origin(),
            &repo.to_string_lossy(),
            &["main".into()],
        )
        .await
        .unwrap()
        .value
        .project_ids;
    assert_eq!(ids.len(), 1);
    let p = dump(w.ws.storage(), "projects", "rowid").await;
    let imported = p.iter().find(|r| r["id"] == ids[0].as_str()).unwrap();
    assert_eq!(imported["name"], "Main (2)");
    assert_eq!(imported["shared_dir"], "main", "not main-2 (bug 8)");
    let q = dump(w.ws.storage(), "saved_queries", "rowid").await;
    assert_eq!(q.len(), 1);
    assert_eq!(q[0]["project_id"], ids[0].as_str());
    // An import writes no file.
    assert_eq!(
        git(&repo, &["status", "--porcelain"]).trim(),
        "",
        "the import wrote a file"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rename_keeps_the_directory() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    // Never synced: the directory is the slug of the name until stored.
    let out =
        w.ws.update_project(
            &w.core,
            &origin(),
            "p1",
            serde_json::from_value(json!({"name": "Team Renamed"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(out.projection.unwrap().status, PublishStatus::Written);
    let p = dump(w.ws.storage(), "projects", "rowid").await;
    assert_eq!(p[0]["shared_dir"], "team");
    assert_eq!(
        w.read("/repos/a/.seaquel/projects/team/project.yaml")
            .as_deref(),
        Some("name: Team Renamed\n")
    );
    assert!(!w.abs("/repos/a/.seaquel/projects/team-renamed").exists());
    // The query still pairs in the same directory.
    let report = sync(&w).await;
    assert!(report.notices.is_empty(), "{:?}", report.notices);
    assert_eq!(row(&w, "q1").await["shared"], 1);
}

// ── Q23 ──

#[tokio::test(flavor = "multi_thread")]
async fn template_changes_reach_the_connection_and_keep_local_fields() {
    let w = World::new().await;
    w.put(
        &format!("{C}/warehouse.yaml"),
        "name: Warehouse\ntype: postgres\nhost: db.internal\nport: 5432\ndatabaseName: app\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "connections",
        &[connection_row(
            "c1",
            "Warehouse",
            Some("repo-a:.seaquel/projects/team/connections/warehouse.yaml"),
        )],
    )
    .await;
    sqlx::query("UPDATE connections SET save_password = 1, ai_share_schema = 1 WHERE id = 'c1'")
        .execute(w.ws.storage().pool())
        .await
        .unwrap();
    w.store.put("db:c1", "s3cret");
    sync(&w).await;
    // A teammate moves the database.
    w.put(
        &format!("{C}/warehouse.yaml"),
        "name: Warehouse\ntype: postgres\nhost: db2.internal\nport: 6432\ndatabaseName: app2\n\
         username: mallory\npassword: hunter2\n",
    );
    let report = sync(&w).await;
    assert!(report.notices.is_empty(), "{:?}", report.notices);
    let c = &dump(w.ws.storage(), "connections", "rowid").await[0];
    assert_eq!(
        (
            c["host"].clone(),
            c["port"].clone(),
            c["database_name"].clone()
        ),
        (json!("db2.internal"), json!(6432), json!("app2"))
    );
    assert_eq!(c["username"], "alice", "the user name stays local");
    assert_eq!(c["save_password"], 1);
    assert_eq!(c["ai_share_schema"], 1);
    assert_eq!(w.store.entries()["db:c1"], "s3cret");
    // A local edit goes out to the template; a credential never does.
    let out =
        w.ws.update_connection(
            &w.core,
            &origin(),
            "c1",
            serde_json::from_value::<ConnectionPatch>(json!({"host": "db3.internal"})).unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    assert_eq!(out.projection.unwrap().status, PublishStatus::Written);
    let tpl = w.read(&format!("{C}/warehouse.yaml")).unwrap();
    assert!(tpl.contains("host: db3.internal"), "{tpl}");
    assert!(!tpl.contains("alice") && !tpl.contains("mallory") && !tpl.contains("hunter2"));
}

// ── Decision 31 ──

#[tokio::test(flavor = "multi_thread")]
async fn local_files_denied_refuses_every_call_and_publishes_nothing() {
    let w = World::new().await;
    let repo = team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    // A desktop-like Core without `LocalFiles`, on the same file.
    let core = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build();
    let ws = core
        .open_workspace(WorkspaceSpec::new(&w.data))
        .await
        .unwrap();
    let o = origin();
    let client = w.git_client();
    let codes = vec![
        ws.shared_sync(&core, &o, SyncTarget::Project("p1".into()))
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_sync(&core, &o, SyncTarget::Repo("repo-a".into()))
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_link_project(&core, &o, "p2", &repo, &[])
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_unlink_project(&core, &o, "p1", true)
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_scan(&core, &repo)
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_import_projects(&core, &o, &repo, &[])
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_repos(&core).await.map(|_| ()).unwrap_err().code,
        ws.shared_repo_register(&core, &o, &repo, None, None)
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_repo_update(&core, &o, "repo-a", Default::default())
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_repo_remove(&core, &o, "repo-a")
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_git_pull(&core, &o, &client, &repo, None)
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.shared_git_commit(&core, &client, &repo, "m")
            .await
            .map(|_| ())
            .unwrap_err()
            .code,
        ws.import_candidates(
            &core,
            seaquel_core::domain::imports::ImportSource::Dbeaver,
            "p1",
            None,
        )
        .await
        .map(|_| ())
        .unwrap_err()
        .code,
        ws.import_create(
            &core,
            &o,
            seaquel_core::domain::imports::ImportSource::Dbeaver,
            "p1",
            &[],
            None,
        )
        .await
        .map(|_| ())
        .unwrap_err()
        .code,
    ];
    assert!(codes.iter().all(|c| c == "NOT_SUPPORTED"), "{codes:?}");
    // A library write in a linked project publishes nothing.
    let out = ws
        .update_saved_query(
            &core,
            &o,
            "q1",
            serde_json::from_value(json!({"query": "SELECT 2"})).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(out.projection, None);
    assert!(
        std::fs::read_dir(w.abs(Q)).is_err() || std::fs::read_dir(w.abs(Q)).unwrap().count() == 0
    );
    assert_eq!(git(Path::new(&repo), &["status", "--porcelain"]).trim(), "");
}

// ── Events (Decision 44) ──

#[tokio::test(flavor = "multi_thread")]
async fn a_sync_emits_one_event_per_kind_and_scope() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    w.put(
        &format!("{Q}/totals.sql"),
        "---\nname: Totals\n---\nSELECT 2\n",
    );
    w.put(
        "/repos/a/.seaquel/projects/team/dashboards/sales.json",
        "{\n  \"name\": \"Sales\",\n  \"widgets\": []\n}\n",
    );
    w.put(
        &format!("{C}/warehouse.yaml"),
        "name: Warehouse\ntype: postgres\nhost: h\nport: 5432\ndatabaseName: d\n",
    );
    team(&w).await;
    let mut events = w.ws.events();
    let report = sync(&w).await;
    assert_eq!(report.rows_changed, 4);
    let got = drain(&mut events).await;
    let mut kinds: Vec<(StoredKind, Option<String>)> =
        got.iter().map(|c| (c.kind, c.scope.clone())).collect();
    kinds.sort_by_key(|(k, s)| (k.as_str(), s.clone()));
    assert_eq!(
        kinds,
        [
            (StoredKind::Connection, Some("p1".to_string())),
            (StoredKind::Dashboard, Some("p1".to_string())),
            (StoredKind::Project, None),
            (StoredKind::SavedQuery, Some("p1".to_string())),
        ]
    );
    let q = got
        .iter()
        .find(|c| c.kind == StoredKind::SavedQuery)
        .unwrap();
    assert_eq!(q.ids.as_ref().map(Vec::len), Some(2));
    assert!(got.iter().all(|c| c.origin.as_deref() == Some("main")));
}

/// Review I1: the rows a sync committed are announced at once, before its
/// file writes, so a file task that fails after the commit loses no event,
/// and the published sequence isn't held while files are written.
#[tokio::test(flavor = "multi_thread")]
async fn a_syncs_row_events_survive_a_failed_file_task() {
    let w = Arc::new(World::new().await);
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    // A shared query whose first write failed: the sync writes its file.
    let pending = w.abs(&format!("{Q}/totals.sql"));
    w.hook.failing.lock().unwrap().insert(pending.clone());
    w.ws.create_saved_query(
        &w.core,
        &origin(),
        serde_json::from_value::<SavedQueryDraft>(
            json!({"projectId": "p1", "name": "Totals", "query": "SELECT 2", "shared": true}),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    w.hook.failing.lock().unwrap().clear();

    // Held write: the row the sync made (orders.sql) is already announced
    // and published.
    w.hook.gate(&pending);
    let mut events = w.ws.events();
    let before = w.ws.change_seq().n;
    let syncing = {
        let w = Arc::clone(&w);
        tokio::spawn(async move { sync(&w).await })
    };
    let hook = Arc::clone(&w.hook);
    tokio::task::spawn_blocking(move || hook.wait_held())
        .await
        .unwrap();
    let published = w.ws.change_seq().n;
    let early = drain(&mut events).await;
    w.hook.release();
    syncing.await.unwrap();
    assert!(
        published > before,
        "the sync's rows weren't published before its files"
    );
    assert!(
        early.iter().any(|c| c.kind == StoredKind::SavedQuery),
        "{early:?}"
    );

    // A file task that fails outright after the commit: the row events
    // were sent all the same.
    w.put(
        &format!("{Q}/latency.sql"),
        "---\nname: Latency\n---\nSELECT 3\n",
    );
    let blocked = w.abs(&format!("{Q}/reports.sql"));
    w.hook.failing.lock().unwrap().insert(blocked.clone());
    w.ws.create_saved_query(
        &w.core,
        &origin(),
        serde_json::from_value::<SavedQueryDraft>(
            json!({"projectId": "p1", "name": "Reports", "query": "SELECT 4", "shared": true}),
        )
        .unwrap(),
    )
    .await
    .unwrap();
    w.hook.failing.lock().unwrap().clear();
    w.hook.panicking.lock().unwrap().insert(blocked);
    let mut events = w.ws.events();
    let failed =
        w.ws.shared_sync(&w.core, &origin(), SyncTarget::Project("p1".into()))
            .await;
    assert!(failed.is_err());
    let got = drain(&mut events).await;
    let q = got.iter().find(|c| c.kind == StoredKind::SavedQuery);
    assert!(q.is_some(), "the committed row's event was lost: {got:?}");
    let latency = dump(w.ws.storage(), "saved_queries", "rowid")
        .await
        .into_iter()
        .find(|r| r["name"] == "Latency")
        .unwrap();
    assert!(q
        .unwrap()
        .ids
        .as_ref()
        .unwrap()
        .contains(&latency["id"].as_str().unwrap().to_string()));
}

/// Re-review R2: a link the sync stores after its file write (a query
/// whose share's write failed, now written) is announced for that row, even
/// when the row's kind already went out with the first transaction.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_link_is_announced_for_its_row() {
    let w = World::new().await;
    team(&w).await;
    let pending = w.abs(&format!("{Q}/totals.sql"));
    w.hook.failing.lock().unwrap().insert(pending.clone());
    let made =
        w.ws.create_saved_query(
            &w.core,
            &origin(),
            serde_json::from_value::<SavedQueryDraft>(
                json!({"projectId": "p1", "name": "Totals", "query": "SELECT 2", "shared": true}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    w.hook.failing.lock().unwrap().clear();
    // A teammate's new file makes the first transaction announce
    // `savedQuery` too.
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    let mut events = w.ws.events();
    let report = sync(&w).await;
    assert_eq!(report.files_written, 1);
    let got = drain(&mut events).await;
    let saved: Vec<&StorageChange> = got
        .iter()
        .filter(|c| c.kind == StoredKind::SavedQuery)
        .collect();
    assert!(
        saved.iter().any(|c| c
            .ids
            .as_ref()
            .is_some_and(|ids| ids.contains(&made.value.id))),
        "the late link's row wasn't announced: {got:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_no_op_sync_emits_nothing() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    let mut events = w.ws.events();
    let report = sync(&w).await;
    assert_eq!(report.rows_changed + report.files_written, 0);
    assert_eq!(drain(&mut events).await, vec![]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_publish_emits_shared_repo() {
    let w = World::new().await;
    w.put(
        &format!("{Q}/orders.sql"),
        "---\nname: Orders\n---\nSELECT 1\n",
    );
    team(&w).await;
    insert_rows(
        w.ws.storage(),
        "saved_queries",
        &[query_row("q1", "Orders", "SELECT 1", true)],
    )
    .await;
    sync(&w).await;
    let mut events = w.ws.events();
    w.ws.update_saved_query(
        &w.core,
        &origin(),
        "q1",
        serde_json::from_value(json!({"query": "SELECT 2"})).unwrap(),
    )
    .await
    .unwrap();
    let got = drain(&mut events).await;
    let kinds: Vec<StoredKind> = got.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [StoredKind::SavedQuery, StoredKind::SharedRepo]);
    assert_eq!(got[1].ids, Some(vec!["repo-a".to_string()]));
    // A viewport-like change that writes nothing emits only its row's.
    let mut events = w.ws.events();
    w.ws.update_saved_query(
        &w.core,
        &origin(),
        "q1",
        serde_json::from_value::<SavedQueryPatch>(json!({"starred": true})).unwrap(),
    )
    .await
    .unwrap();
    let kinds: Vec<StoredKind> = drain(&mut events).await.iter().map(|c| c.kind).collect();
    assert_eq!(kinds, [StoredKind::SavedQuery]);
}

// ── Logs (Decision 50) ──

#[tokio::test(flavor = "multi_thread")]
async fn no_paths_names_hosts_or_contents_in_logs() {
    capture_logs();
    let w = World::new().await;
    w.put(
        &format!("{Q}/canary-file.sql"),
        "---\nname: CanaryQueryName\n---\nSELECT 'CanaryText'\n",
    );
    w.put(
        &format!("{C}/canary-conn.yaml"),
        "name: CanaryConn\ntype: postgres\nhost: canary-host.internal\nport: 5432\ndatabaseName: canarydb\n",
    );
    std::fs::create_dir_all(w.abs("/outside")).unwrap();
    symlink(w.abs("/outside"), w.abs(&format!("{Q}/canary-link.sql"))).unwrap();
    let repo = team(&w).await;
    sync(&w).await;
    w.hook
        .failing
        .lock()
        .unwrap()
        .insert(w.abs(&format!("{Q}/canary-fail.sql")));
    let _ =
        w.ws.create_saved_query(
            &w.core,
            &origin(),
            serde_json::from_value::<SavedQueryDraft>(json!({"projectId": "p1",
                "name": "Canary fail", "query": "SELECT 'CanaryText2'", "shared": true}))
            .unwrap(),
        )
        .await
        .unwrap();
    let _ = w.ws.shared_scan(&w.core, &repo).await.unwrap();
    let _ =
        w.ws.shared_unlink_project(&w.core, &origin(), "p1", true)
            .await;
    let log = logged();
    for canary in [
        "canary",
        "Canary",
        "CanaryQueryName",
        "canary-host",
        "SELECT",
        repo.as_str(),
        &w.root.to_string_lossy(),
        ".seaquel",
        "team",
    ] {
        assert!(
            !log.contains(canary),
            "{canary:?} reached a log line:\n{log}"
        );
    }
    assert!(log.contains("shared.sync"), "the sync logged nothing");
}

/// Task 6 review: `lastSyncAt` is best effort. A pull or push that worked
/// answers its result even when the repo row can't be written (here, the
/// CLI's read-only storage); the failure is logged by code, no path.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_or_pull_succeeds_when_last_sync_cant_be_written() {
    let w = World::new().await;
    let path = team(&w).await;
    let repo = w.abs("/repos/a");
    std::fs::write(repo.join("notes.txt"), "mine\n").unwrap();
    commit_all(&repo, "mine");
    w.ws.close().await;
    let ro = w
        .core
        .open_workspace(
            WorkspaceSpec::new(&w.data)
                .with_storage_options(seaquel_core::storage::StorageOptions {
                    read_only: true,
                    ..Default::default()
                })
                .with_secrets(w.store.clone()),
        )
        .await
        .unwrap();
    capture_logs();
    let pushed = ro
        .shared_git_push(&w.core, &origin(), &w.git_client(), &path, None)
        .await;
    let pulled = ro
        .shared_git_pull(&w.core, &origin(), &w.git_client(), &path, None)
        .await;
    let pushed = pushed.expect("the push worked; only lastSyncAt failed");
    assert!(pushed.success, "{pushed:?}");
    let pulled = pulled.expect("the pull worked; only lastSyncAt failed");
    assert!(pulled.success, "{pulled:?}");
    let records = logged();
    assert!(
        records
            .lines()
            .any(|r| r.contains("code=STORAGE_READ_ONLY") && r.contains("lastSync")),
        "{records}"
    );
    assert!(!records.contains(&path), "{records}");
}

// ── Q31: unlink keeps the user's own connections ──

/// `p1` linked to `/repos/a` with "Warehouse" (`c1`, the user's own)
/// exported at link time, and "Billing" a teammate's template the sync
/// imported. Each has a password in the keychain. Returns the imported
/// connection's id.
async fn linked_with_own_and_imported(w: &World) -> String {
    let path = team(w).await;
    w.put(&format!("{C}/billing.yaml"), "name: Billing\ntype: postgres\nhost: billing.internal\nport: 5432\ndatabaseName: billing\n");
    insert_rows(
        w.ws.storage(),
        "connections",
        &[connection_row("c1", "Warehouse", None)],
    )
    .await;
    // The same path again only shares the ticked connections (review I4).
    w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &["c1".to_string()])
        .await
        .unwrap();
    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    assert_eq!(conns.len(), 2, "{conns:?}");
    let imported = conns.iter().find(|c| c["name"] == "Billing").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let c1 = conns.iter().find(|c| c["id"] == "c1").unwrap();
    assert_eq!(c1["shared_origin"], "exported", "{c1:?}");
    let imp = conns.iter().find(|c| c["id"] == imported.as_str()).unwrap();
    assert_eq!(imp["shared_origin"], "imported", "{imp:?}");
    w.store.put("db:c1", "own-password");
    w.store.put(&format!("db:{imported}"), "imported-password");
    imported
}

#[tokio::test(flavor = "multi_thread")]
async fn unlink_keeps_own_connections_and_secrets_and_removes_imported_only_when_confirmed() {
    let w = World::new().await;
    let imported = linked_with_own_and_imported(&w).await;

    let report =
        w.ws.shared_unlink_project(&w.core, &origin(), "p1", true)
            .await
            .unwrap()
            .value;

    assert_eq!(report.kept_connection_ids, vec!["c1".to_string()]);
    assert_eq!(report.removed_connection_ids, vec![imported.clone()]);
    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    assert_eq!(conns.len(), 1, "{conns:?}");
    let c1 = &conns[0];
    assert_eq!(c1["id"], "c1");
    assert_eq!(c1["shared_connection_id"], Value::Null);
    assert_eq!(c1["shared_origin"], Value::Null);
    assert_eq!(c1["is_local_only"], 1);
    let secrets = w.store.entries();
    assert_eq!(
        secrets.get("db:c1").map(String::as_str),
        Some("own-password")
    );
    assert!(
        !secrets.contains_key(&format!("db:{imported}")),
        "{secrets:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unlink_without_confirming_keeps_every_connection_unlinked() {
    let w = World::new().await;
    let imported = linked_with_own_and_imported(&w).await;

    let report =
        w.ws.shared_unlink_project(&w.core, &origin(), "p1", false)
            .await
            .unwrap()
            .value;

    assert!(report.removed_connection_ids.is_empty());
    assert_eq!(
        report.kept_connection_ids,
        vec!["c1".to_string(), imported.clone()]
    );
    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    assert_eq!(conns.len(), 2);
    for c in &conns {
        assert_eq!(c["shared_connection_id"], Value::Null, "{c:?}");
        assert_eq!(c["is_local_only"], 1, "{c:?}");
    }
    assert_eq!(w.store.entries().len(), 2);
}

/// A relink (unlink, then link) that fails after the unlink loses none of
/// the user's connections.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_relink_after_an_unlink_keeps_the_users_connections() {
    let w = World::new().await;
    linked_with_own_and_imported(&w).await;
    w.ws.shared_unlink_project(&w.core, &origin(), "p1", false)
        .await
        .unwrap();

    let missing = w.abs("/repos/missing").to_string_lossy().into_owned();
    let err =
        w.ws.shared_link_project(&w.core, &origin(), "p1", &missing, &["c1".to_string()])
            .await
            .unwrap_err();
    assert_eq!(err.code, "FILE_ERROR");

    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    assert!(conns.iter().any(|c| c["id"] == "c1"), "{conns:?}");
    assert_eq!(
        w.store.entries().get("db:c1").map(String::as_str),
        Some("own-password")
    );
}

// ── Re-review I1: a ticked connection is shared even when it's local-only ──

#[tokio::test(flavor = "multi_thread")]
async fn linking_shares_a_ticked_local_only_connection() {
    let w = World::new().await;
    let path = team(&w).await;
    let mut row = connection_row("c1", "Warehouse", None);
    // As the wizard makes it.
    row["is_local_only"] = json!(1);
    insert_rows(w.ws.storage(), "connections", &[row]).await;

    w.ws.shared_link_project(&w.core, &origin(), "p1", &path, &["c1".to_string()])
        .await
        .unwrap();

    assert!(
        w.read(&format!("{C}/warehouse.yaml")).is_some(),
        "the template is written"
    );
    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    let c1 = conns.iter().find(|c| c["id"] == "c1").unwrap();
    assert_eq!(c1["is_local_only"], 0, "{c1:?}");
    assert!(c1["shared_connection_id"].is_string(), "{c1:?}");
    assert_eq!(c1["shared_origin"], "exported", "{c1:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relink_to_another_repo_exports_a_kept_connection() {
    let w = World::new().await;
    linked_with_own_and_imported(&w).await;
    w.ws.shared_unlink_project(&w.core, &origin(), "p1", false)
        .await
        .unwrap();
    let other = w.git_repo("b");
    let other = other.to_string_lossy().into_owned();

    w.ws.shared_link_project(&w.core, &origin(), "p1", &other, &["c1".to_string()])
        .await
        .unwrap();

    assert!(
        w.read("/repos/b/.seaquel/projects/team/connections/warehouse.yaml")
            .is_some(),
        "c1 is exported to the new repo"
    );
    let conns = dump(w.ws.storage(), "connections", "rowid").await;
    let c1 = conns.iter().find(|c| c["id"] == "c1").unwrap();
    assert_eq!(c1["is_local_only"], 0, "{c1:?}");
    assert_eq!(c1["shared_origin"], "exported", "{c1:?}");
}

/// Re-review minor 1: the unlink dialog lists exactly what an unlink would
/// remove (Core answers it), not every linked connection the page holds.
#[tokio::test(flavor = "multi_thread")]
async fn unlink_preview_lists_what_unlink_would_remove() {
    let w = World::new().await;
    let imported = linked_with_own_and_imported(&w).await;
    // Linked to another repo's template: not this project's to remove.
    insert_rows(
        w.ws.storage(),
        "connections",
        &[connection_row(
            "c9",
            "Elsewhere",
            Some("repo-z:.seaquel/projects/team/connections/x.yaml"),
        )],
    )
    .await;

    let preview = w.ws.shared_unlink_preview(&w.core, "p1").await.unwrap();
    assert_eq!(preview.imported_connection_ids, vec![imported.clone()]);

    let report =
        w.ws.shared_unlink_project(&w.core, &origin(), "p1", true)
            .await
            .unwrap()
            .value;
    assert_eq!(
        report.removed_connection_ids,
        preview.imported_connection_ids
    );
    let err = w.ws.shared_unlink_preview(&w.core, "p1").await.unwrap_err();
    assert_eq!(err.code, "PROJECT_NOT_LINKED");
}
