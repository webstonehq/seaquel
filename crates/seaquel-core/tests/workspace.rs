//! `Core::open_workspace`: each workspace owns its own storage and secret
//! store, and storage failures keep their codes.

use std::sync::Arc;

use seaquel_core::secrets::MemoryStore;
use seaquel_core::storage::{connections, projects};
use seaquel_core::{Core, WorkspaceSpec, DESKTOP_STORAGE_FILE};
use seaquel_types::storage::{PersistedConnection, PersistedProject};

fn core() -> Core {
    Core::builder().build()
}

fn project(id: &str) -> PersistedProject {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": format!("Project {id}"),
        "createdAt": "2026-01-02T03:04:05.000Z",
        "updatedAt": "2026-01-02T03:04:05.000Z",
        "customLabels": [],
    }))
    .unwrap()
}

fn connection(id: &str, project_id: &str) -> PersistedConnection {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "projectId": project_id,
        "name": format!("Connection {id}"),
        "type": "postgres",
        "host": "localhost",
        "port": 5432,
        "databaseName": "db",
        "username": "me",
        "labelIds": [],
    }))
    .unwrap()
}

#[tokio::test]
async fn opens_storage_in_the_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ws = core()
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap();

    assert_eq!(ws.data_dir(), dir.path());
    assert_eq!(ws.storage().path(), dir.path().join(DESKTOP_STORAGE_FILE));
    assert!(dir.path().join(DESKTOP_STORAGE_FILE).is_file());
    assert!(ws.secrets().is_none());

    projects::save(ws.storage(), &project("p1")).await.unwrap();
    connections::save(ws.storage(), &connection("c1", "p1"))
        .await
        .unwrap();
    let loaded = connections::load_all(ws.storage()).await.unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, "c1");
    ws.close().await;
}

#[tokio::test]
async fn the_storage_file_name_is_configurable() {
    let dir = tempfile::tempdir().unwrap();
    let ws = core()
        .open_workspace(WorkspaceSpec::new(dir.path()).with_storage_file("meta.db"))
        .await
        .unwrap();
    assert!(dir.path().join("meta.db").is_file());
    assert!(!dir.path().join(DESKTOP_STORAGE_FILE).exists());
    ws.close().await;
}

#[tokio::test]
async fn two_workspaces_are_independent() {
    let core = core();
    let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a_store = Arc::new(MemoryStore::new());
    let b_store = Arc::new(MemoryStore::new());
    let a = core
        .open_workspace(WorkspaceSpec::new(a_dir.path()).with_secrets(a_store))
        .await
        .unwrap();
    let b = core
        .open_workspace(WorkspaceSpec::new(b_dir.path()).with_secrets(b_store))
        .await
        .unwrap();

    projects::save(a.storage(), &project("pa")).await.unwrap();
    projects::save(b.storage(), &project("pb")).await.unwrap();
    let ids = |ps: Vec<PersistedProject>| ps.into_iter().map(|p| p.id).collect::<Vec<_>>();
    assert_eq!(ids(projects::load_all(a.storage()).await.unwrap()), ["pa"]);
    assert_eq!(ids(projects::load_all(b.storage()).await.unwrap()), ["pb"]);

    let a_secrets = a.secrets().unwrap();
    let b_secrets = b.secrets().unwrap();
    a_secrets.set("db:c1", "a-password").await.unwrap();
    assert_eq!(
        a_secrets.get("db:c1").await.unwrap().as_deref(),
        Some("a-password")
    );
    assert_eq!(b_secrets.get("db:c1").await.unwrap(), None);

    // Closing one leaves the other working.
    a.close().await;
    assert!(projects::load_all(a.storage()).await.is_err());
    assert_eq!(ids(projects::load_all(b.storage()).await.unwrap()), ["pb"]);
    b.close().await;
}

#[tokio::test]
async fn a_legacy_data_dir_fails_with_legacy_storage() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("projects.json"), "{}").unwrap();
    let err = core()
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap_err();
    assert_eq!(err.code, "LEGACY_STORAGE");
    assert!(err.message.contains("2026.4.5"), "{}", err.message);
    assert!(!dir.path().join(DESKTOP_STORAGE_FILE).exists());
}

#[tokio::test]
async fn a_file_that_isnt_sqlite_fails_with_storage_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(DESKTOP_STORAGE_FILE);
    std::fs::write(&file, b"not a database, just some text").unwrap();
    let err = core()
        .open_workspace(WorkspaceSpec::new(dir.path()))
        .await
        .unwrap_err();
    assert_eq!(err.code, "STORAGE_CORRUPT");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        b"not a database, just some text"
    );
}

#[test]
fn debug_hides_the_secret_store() {
    let spec = WorkspaceSpec::new("/tmp/x").with_secrets(Arc::new(MemoryStore::new()));
    let text = format!("{spec:?}");
    assert!(text.contains("<store>"), "{text}");
}
