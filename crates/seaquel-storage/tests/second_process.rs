//! `SchemaPolicy::RequireCurrent`: a second process
//! beside the app (the TUI) opens the file writable, but only when there is
//! no schema work left, and then never creates, migrates or re-journals it.

#![cfg(not(target_arch = "wasm32"))]

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use common::*;
use seaquel_storage::{
    app_state, SchemaPolicy, Storage, StorageError, StorageOptions, DATA_STEPS_TABLE,
    STORAGE_NEEDS_UPGRADE, STORAGE_NOT_FOUND,
};

fn second_process() -> StorageOptions {
    StorageOptions {
        schema: SchemaPolicy::RequireCurrent,
        ..StorageOptions::default()
    }
}

async fn open_second(path: &Path) -> Result<Storage, StorageError> {
    Storage::open(path, second_process()).await
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The file's bytes and the directory's entries (no `-wal` or `-shm`
/// either), to show a refused open changed nothing.
fn disk_state(path: &Path) -> (Vec<u8>, Vec<String>) {
    (
        std::fs::read(path).unwrap(),
        entries(path.parent().unwrap()),
    )
}

/// A file the app has opened and closed: current, in WAL mode, with no
/// `-wal` or `-shm` left.
async fn current_file(dir: &Path) -> PathBuf {
    let path = dir.join("seaquel.db");
    let storage = Storage::open(
        &path,
        StorageOptions {
            max_connections: 1,
            ..StorageOptions::default()
        },
    )
    .await
    .unwrap();
    storage.close().await;
    for _ in 0..100 {
        if entries(dir) == ["seaquel.db"] {
            return path;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the app's -wal and -shm stayed: {:?}", entries(dir));
}

async fn test_migrator() -> sqlx::migrate::Migrator {
    sqlx::migrate::Migrator::new(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/test_migrations"),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_current_file_opens_writable_and_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    let st = open_second(&path).await.unwrap();
    assert!(!st.is_read_only());

    let mut tx = st.write().await.unwrap();
    app_state::set_in(&mut tx, "k", Some("from the tui"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    st.close().await;

    // The app reads what the second process wrote.
    let app = Storage::open(&path, StorageOptions::default())
        .await
        .unwrap();
    assert_eq!(
        app_state::get(&app, "k").await.unwrap().as_deref(),
        Some("from the tui")
    );
    app.close().await;
}

#[tokio::test]
async fn it_leaves_the_journal_mode_alone() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    // A current file whose journal mode isn't WAL: the second process must
    // not switch it (only the app sets the journal mode).
    Storage::open(&path, StorageOptions::default())
        .await
        .unwrap()
        .close()
        .await;
    exec_file(&path, "PRAGMA journal_mode = DELETE").await;

    let st = open_second(&path).await.unwrap();
    let mut tx = st.write().await.unwrap();
    app_state::set_in(&mut tx, "k", Some("v")).await.unwrap();
    tx.commit().await.unwrap();
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(st.pool())
        .await
        .unwrap();
    assert_eq!(mode, "delete");
    st.close().await;
}

#[tokio::test]
async fn every_frozen_release_file_needs_the_app_first() {
    for fixture in [
        "schemas/v2026.4.5-beta.1.sql",
        "schemas/v2026.4.5.sql",
        "schemas/v2026.4.8.sql",
        "schemas/v2026.9.1.sql",
        "schemas/v2026.9.2.sql",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seaquel.db");
        load_fixture(&path, fixture).await;
        let before = disk_state(&path);

        let err = open_second(&path).await.unwrap_err();
        assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{fixture}: {err}");
        assert_eq!(disk_state(&path), before, "{fixture}");
    }
}

#[tokio::test]
async fn a_pending_migration_needs_the_app_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    let before = disk_state(&path);

    let err = Storage::open_with_migrator(&path, second_process(), test_migrator().await)
        .await
        .unwrap_err();
    assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
    assert!(err.to_string().contains("migration 9001"), "{err}");
    assert_eq!(disk_state(&path), before);
}

#[tokio::test]
async fn a_pending_data_step_needs_the_app_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = current_file(dir.path()).await;
    exec_file(&path, &format!("DELETE FROM {DATA_STEPS_TABLE}")).await;
    let before = disk_state(&path);

    let err = open_second(&path).await.unwrap_err();
    assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
    assert!(
        matches!(&err, StorageError::DataStepPending { .. }),
        "{err:?}"
    );
    assert_eq!(disk_state(&path), before);
}

#[tokio::test]
async fn a_missing_file_is_not_found_and_nothing_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("users/u1/seaquel.db");
    let err = open_second(&path).await.unwrap_err();
    assert_eq!(err.code(), STORAGE_NOT_FOUND, "{err}");
    assert!(entries(dir.path()).is_empty());

    let path = dir.path().join("seaquel.db");
    let err = open_second(&path).await.unwrap_err();
    assert_eq!(err.code(), STORAGE_NOT_FOUND, "{err}");
    assert!(entries(dir.path()).is_empty());
}

#[tokio::test]
async fn an_empty_file_needs_the_app_first() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("seaquel.db");
    std::fs::write(&path, b"").unwrap();
    let err = open_second(&path).await.unwrap_err();
    assert_eq!(err.code(), STORAGE_NEEDS_UPGRADE, "{err}");
    assert_eq!(disk_state(&path), (Vec::new(), vec!["seaquel.db".into()]));
}
