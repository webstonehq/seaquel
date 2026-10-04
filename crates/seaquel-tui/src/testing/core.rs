//! A real Core in a temp data dir: a file the "app" made (opened the
//! normal way, so it's current), seeded through Core's library calls, then
//! opened by the TUI as a second process. Never the real data dir or
//! keychain.

use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use seaquel_core::domain::library::{
    ConnectionDraft, ProjectDraft, SavedQueryDraft, SecretChanges,
};
use seaquel_core::secrets::{MemoryStore, SecretStore};
use seaquel_core::{Core, Workspace, WorkspaceSpec, WriteOrigin};

/// An empty in-memory secret store.
pub fn memory_store() -> Arc<dyn SecretStore> {
    Arc::new(MemoryStore::new())
}

/// The "app": a Core like the desktop's, without the AI client.
pub fn app_core() -> Arc<Core> {
    Arc::new(seaquel_terminal::core_builder(seaquel_terminal::CoreOptions::default()).build())
}

/// A data dir with a current `seaquel.db`.
pub struct Seed {
    pub dir: tempfile::TempDir,
    /// The app's secret store, which the TUI opens too when a test wants it.
    pub store: Arc<dyn SecretStore>,
}

impl Seed {
    pub async fn new() -> Seed {
        let seed = Seed {
            dir: tempfile::tempdir().unwrap(),
            store: memory_store(),
        };
        seed.with(|_, _| async {}).await;
        seed
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Opens the file as the app does, runs `f`, and closes it.
    pub async fn with<F, Fut, T>(&self, f: F) -> T
    where
        F: FnOnce(Arc<Core>, Arc<Workspace>) -> Fut,
        Fut: Future<Output = T>,
    {
        let core = app_core();
        let ws = core
            .open_workspace(WorkspaceSpec::new(self.path()).with_secrets(self.store.clone()))
            .await
            .unwrap();
        let out = f(core.clone(), ws.clone()).await;
        ws.close_all(&core).await;
        ws.close().await;
        out
    }
}

fn app() -> WriteOrigin {
    WriteOrigin::new(Some("app-window"))
}

/// A new project's id.
pub async fn project(core: &Core, ws: &Workspace, name: &str) -> String {
    let draft: ProjectDraft = serde_json::from_value(serde_json::json!({ "name": name })).unwrap();
    ws.create_project(core, &app(), draft)
        .await
        .unwrap()
        .value
        .id
}

/// A new connection's id, from a `connectionCreate` draft.
pub async fn connection(core: &Core, ws: &Workspace, draft: serde_json::Value) -> String {
    connection_with(core, ws, draft, SecretChanges::default()).await
}

/// A new connection's id, with secrets saved beside it.
pub async fn connection_with(
    core: &Core,
    ws: &Workspace,
    mut draft: serde_json::Value,
    secrets: SecretChanges,
) -> String {
    let obj = draft.as_object_mut().unwrap();
    for (key, value) in [
        ("host", serde_json::json!("")),
        ("port", serde_json::json!(0)),
        ("databaseName", serde_json::json!("")),
        ("username", serde_json::json!("")),
        ("savePassword", serde_json::json!(false)),
        ("saveSshPassword", serde_json::json!(false)),
        ("saveSshKeyPassphrase", serde_json::json!(false)),
        ("labelIds", serde_json::json!([])),
    ] {
        obj.entry(key).or_insert(value);
    }
    let draft: ConnectionDraft = serde_json::from_value(draft).unwrap();
    ws.create_connection(core, &app(), draft, secrets)
        .await
        .unwrap()
        .value
        .id
}

/// A new saved query's id.
pub async fn saved_query(
    core: &Core,
    ws: &Workspace,
    project_id: &str,
    name: &str,
    sql: &str,
    folder: Option<&str>,
) -> String {
    let draft: SavedQueryDraft = serde_json::from_value(serde_json::json!({
        "projectId": project_id, "name": name, "query": sql, "folder": folder,
        "starred": false, "shared": false,
    }))
    .unwrap();
    ws.create_saved_query(core, &app(), draft)
        .await
        .unwrap()
        .value
        .id
}
