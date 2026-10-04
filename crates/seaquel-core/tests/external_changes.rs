//! External changes (phase 7a Decisions 3 and 6): a workspace with
//! `with_external_changes` polls its storage's `external_version` on the
//! executor and turns a change committed by any other connection into one
//! `StorageChanged { kind: External }`, after taking a change-sequence
//! number; a second-process workspace opens a current file writable and
//! does no maintenance writes.
#![cfg(all(feature = "storage", feature = "secrets", feature = "workspace"))]
// Native-only tests: tasks on the test runtime and the wall clock.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use common::{core, dump, insert_rows, TestStore};
use futures::StreamExt;
use seaquel_core::domain::library::ProjectDraft;
use seaquel_core::storage::{app_state, SchemaPolicy, Storage, StorageOptions};
use seaquel_core::{
    StorageChange, StoredKind, WorkspaceEvent, WorkspaceSpec, WriteOrigin,
    STRING_SECRETS_UPGRADED_KEY,
};
use seaquel_runtime::BoxStream;
use serde_json::json;

const POLL: Duration = Duration::from_millis(20);

/// The storage events that arrive within `wait`.
async fn drain_for(
    events: &mut BoxStream<'static, WorkspaceEvent>,
    wait: Duration,
) -> Vec<StorageChange> {
    let mut out = Vec::new();
    let deadline = tokio::time::Instant::now() + wait;
    while let Ok(Some(e)) = tokio::time::timeout_at(deadline, events.next()).await {
        if let WorkspaceEvent::StorageChanged(c) = e {
            out.push(c);
        }
    }
    out
}

fn draft_project(name: &str) -> ProjectDraft {
    serde_json::from_value(json!({ "name": name })).unwrap()
}

/// A write by another connection to the file: what the other process does.
async fn write_elsewhere(path: &Path, key: &str) {
    let other = Storage::open(path, second_process_options()).await.unwrap();
    app_state::set(&other, key, Some("v")).await.unwrap();
    other.close().await;
}

/// An open that writes nothing itself, so the only change is the write.
fn second_process_options() -> StorageOptions {
    StorageOptions {
        schema: SchemaPolicy::RequireCurrent,
        ..StorageOptions::default()
    }
}

fn file(dir: &Path) -> PathBuf {
    dir.join(seaquel_core::DESKTOP_STORAGE_FILE)
}

#[tokio::test(flavor = "multi_thread")]
async fn another_connections_write_is_one_external_event_with_a_newer_seq() {
    let dir = tempfile::tempdir().unwrap();
    let core = core();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_external_changes(POLL))
        .await
        .unwrap();
    let mut events = ws.events();
    ws.create_project(
        &core,
        &WriteOrigin::new(Some("win-1")),
        draft_project("Main"),
    )
    .await
    .unwrap();
    let own = drain_for(&mut events, POLL * 6).await;
    // A's own write: its own event and nothing else.
    assert_eq!(own.len(), 1, "{own:?}");
    assert_eq!(own[0].kind, StoredKind::Project);

    write_elsewhere(&file(dir.path()), "elsewhere").await;
    let seen = drain_for(&mut events, POLL * 10).await;
    assert_eq!(seen.len(), 1, "{seen:?}");
    let ext = &seen[0];
    assert_eq!(ext.kind, StoredKind::External);
    assert_eq!((ext.scope.as_deref(), ext.ids.as_deref()), (None, None));
    assert_eq!(ext.origin, None);
    assert_eq!(ext.seq.epoch, own[0].seq.epoch);
    assert!(ext.seq.n > own[0].seq.n, "{ext:?} after {:?}", own[0]);
    // The number is complete: a read after the event is at least as new.
    assert!(ws.change_seq().n >= ext.seq.n);

    // Several commits between two polls are one event.
    let path = file(dir.path());
    let other = Storage::open(&path, second_process_options())
        .await
        .unwrap();
    for i in 0..5 {
        app_state::set(&other, &format!("burst-{i}"), Some("v"))
            .await
            .unwrap();
    }
    other.close().await;
    let burst = drain_for(&mut events, POLL * 10).await;
    assert!(
        (1..=2).contains(&burst.len()),
        "a burst is one or two events: {burst:?}"
    );
    assert!(burst.iter().all(|c| c.kind == StoredKind::External));
    ws.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn polling_is_off_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let core = core();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();
    let mut events = ws.events();
    write_elsewhere(&file(dir.path()), "elsewhere").await;
    let seen = drain_for(&mut events, POLL * 10).await;
    assert!(seen.is_empty(), "{seen:?}");
    ws.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_event_after_close_or_close_all() {
    for close_all in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let core = core();
        let ws = core
            .open_workspace(WorkspaceSpec::new(dir.path()).with_external_changes(POLL))
            .await
            .unwrap();
        let mut events = ws.events();
        if close_all {
            ws.close_all(&core).await;
        } else {
            ws.close().await;
        }
        write_elsewhere(&file(dir.path()), "after-close").await;
        let seen = drain_for(&mut events, POLL * 10).await;
        assert!(seen.is_empty(), "close_all {close_all}: {seen:?}");
        assert!(!ws.polls_external_changes(), "close_all {close_all}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_poll_task_holds_no_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let core = core();
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_external_changes(POLL))
        .await
        .unwrap();
    assert!(ws.polls_external_changes());
    let weak = Arc::downgrade(&ws);
    drop(ws);
    tokio::time::sleep(POLL * 3).await;
    assert!(weak.upgrade().is_none());
}

/// A file a pre-5a release left: a password in a stored connection string,
/// a row with no `name_key`, and the upgrade not yet run.
async fn seed_old_rows(dir: &Path) -> PathBuf {
    let path = file(dir);
    let st = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    insert_rows(
        &st,
        "projects",
        &[json!({"id": "p1", "name": "Main", "description": null,
                 "created_at": "2024-01-01T00:00:00.000Z",
                 "updated_at": "2024-01-01T00:00:00.000Z", "git_repo_path": null})],
    )
    .await;
    insert_rows(
        &st,
        "connections",
        &[json!({
            "id": "c1", "project_id": "p1", "name": "Old", "type": "postgres",
            "host": "db.example.com", "port": 5432, "database_name": "app",
            "username": "alice", "ssl_mode": null,
            "connection_string": "postgres://alice:canary-pw@db.example.com/app",
            "last_connected": "2024-01-01T00:00:00.000Z", "ssh_tunnel": null,
            "save_password": 0, "save_ssh_password": 0, "save_ssh_key_passphrase": 0,
            "is_local_only": 1, "shared_connection_id": null, "ai_share_schema": null,
            "ai_share_data": null, "active_ai_provider_id": null, "active_ai_model": null,
        })],
    )
    .await;
    st.close().await;
    path
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_process_runs_no_maintenance_writes() {
    let dir = tempfile::tempdir().unwrap();
    seed_old_rows(dir.path()).await;
    let store = TestStore::new();
    let core = core();
    let ws = core
        .open_workspace(
            WorkspaceSpec::new(dir.path())
                .with_secrets(store.clone())
                .second_process(),
        )
        .await
        .unwrap();
    // No secrets upgrade: the keychain wasn't touched and the string is
    // as it was.
    assert_eq!(store.sets.load(Ordering::SeqCst), 0);
    assert_eq!(store.gets.load(Ordering::SeqCst), 0);
    let rows = dump(ws.storage(), "connections", "id").await;
    assert_eq!(
        rows[0]["connection_string"],
        "postgres://alice:canary-pw@db.example.com/app"
    );
    assert_eq!(
        app_state::get(ws.storage(), STRING_SECRETS_UPGRADED_KEY)
            .await
            .unwrap(),
        None
    );
    // No refill: the row's `name_key` is still NULL.
    assert_eq!(rows[0]["name_key"], serde_json::Value::Null);
    // It writes when asked.
    ws.create_project(&core, &WriteOrigin::none(), draft_project("From the TUI"))
        .await
        .unwrap();
    ws.close().await;

    // The app's open then runs both, as before.
    let app = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_secrets(store.clone()))
        .await
        .unwrap();
    assert_eq!(store.entries()["db:c1"], "canary-pw");
    let rows = dump(app.storage(), "connections", "id").await;
    assert_ne!(rows[0]["name_key"], serde_json::Value::Null);
    app.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_process_needs_an_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let err = core()
        .open_workspace(WorkspaceSpec::new(dir.path()).second_process())
        .await
        .err()
        .unwrap();
    assert_eq!(err.code, "STORAGE_NOT_FOUND");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_app_hears_a_second_process_workspace() {
    // The TUI's workspace and the app's, on one file: each hears the other.
    let dir = tempfile::tempdir().unwrap();
    let core = core();
    let app = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_external_changes(POLL))
        .await
        .unwrap();
    let tui = core
        .open_workspace(
            WorkspaceSpec::new(dir.path())
                .second_process()
                .with_external_changes(POLL),
        )
        .await
        .unwrap();
    let mut app_events = app.events();
    let mut tui_events = tui.events();
    tui.create_project(
        &core,
        &WriteOrigin::new(Some("tui-0123abcd")),
        draft_project("T"),
    )
    .await
    .unwrap();
    let heard = drain_for(&mut app_events, POLL * 10).await;
    assert_eq!(
        heard.iter().map(|c| c.kind).collect::<Vec<_>>(),
        [StoredKind::External]
    );
    let own = drain_for(&mut tui_events, POLL * 6).await;
    assert_eq!(
        own.iter().map(|c| c.kind).collect::<Vec<_>>(),
        [StoredKind::Project]
    );

    app.create_project(&core, &WriteOrigin::new(Some("main")), draft_project("A"))
        .await
        .unwrap();
    let heard = drain_for(&mut tui_events, POLL * 10).await;
    assert_eq!(
        heard.iter().map(|c| c.kind).collect::<Vec<_>>(),
        [StoredKind::External]
    );
    app.close().await;
    tui.close().await;
}

/// Task 1 review: `second_process` holds whatever order the builders run
/// in; `with_storage_options` after it doesn't undo it.
#[tokio::test(flavor = "multi_thread")]
async fn second_process_holds_in_either_builder_order() {
    let dir = tempfile::tempdir().unwrap();
    let err = core()
        .open_workspace(
            WorkspaceSpec::new(dir.path())
                .second_process()
                .with_storage_options(StorageOptions::default()),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(err.code, "STORAGE_NOT_FOUND");
    assert!(!file(dir.path()).exists());
}
