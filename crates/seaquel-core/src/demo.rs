//! The browser demo's own connection (phase 8 Decision 19).
//!
//! The demo has one saved connection with a fixed id, [`DEMO_CONNECTION_ID`]:
//! the page's DuckDB-WASM database. Its history, AI chats and labels refer
//! to it by that id, so Core stores it once and afterwards only marks it
//! connected. Behind `browser` (and compiled for this crate's tests): no
//! other interface can make the row.

use log::debug;
use seaquel_storage::{connections, projects};
use seaquel_types::storage::PersistedConnection;
use seaquel_workspace::library::{
    self as lib, ConnectionDraft, ConnectionPatch, SecretChanges, DEFAULT_PROJECT_ID,
};

use crate::changes::WriteOrigin;
use crate::library::{check_engine, insert_connection_in, now, update_connection_in};
use crate::{Core, CoreError, Seqd, StoredKind, Workspace};

/// The demo connection's fixed id.
pub const DEMO_CONNECTION_ID: &str = "demo-connection";

/// The row the demo stores the first time, as the TypeScript twin's
/// `addDemoConnection` did: the page's DuckDB-WASM database, labelled
/// `prod`, with no secrets and no connection string.
fn demo_draft(project_id: String) -> ConnectionDraft {
    ConnectionDraft {
        project_id,
        name: "Demo Database".to_string(),
        ty: "duckdb".to_string(),
        host: "browser".to_string(),
        port: 0.0,
        database_name: "demo".to_string(),
        username: String::new(),
        ssl_mode: None,
        connection_string: None,
        ssh_tunnel: None,
        save_password: false,
        save_ssh_password: false,
        save_ssh_key_passphrase: false,
        label_ids: vec!["prod".to_string()],
        is_local_only: None,
        shared_connection_id: None,
        ai_share_schema: None,
        ai_share_data: None,
        active_ai_provider_id: None,
        active_ai_model: None,
        connected: true,
        // Never refused for its name: a visitor's own "Demo Database" in
        // the same project gives this one "Demo Database (2)".
        rename_if_taken: true,
    }
}

impl Workspace {
    /// Stores the demo connection (`demo-connection`) the first time, and
    /// afterwards only sets its `lastConnected`, so the visitor's labels, AI
    /// flags and name stay. One write transaction with Core's checks and
    /// `name_key`, and one `connection` event (plus a `project` event when
    /// it had to make the default project first).
    ///
    /// A new row goes into the default project, else the first project,
    /// else a default project made for it. Errors: the storage codes.
    pub async fn ensure_demo_connection(
        &self,
        core: &Core,
        origin: &WriteOrigin,
    ) -> Result<Seqd<PersistedConnection>, CoreError> {
        debug!(activity = "library.demoConnection"; "Ensure the demo connection");
        check_engine(core, "duckdb")?;
        let limits = core.library_limits();
        let now = now(core)?;
        let mut tx = self.storage().write().await?;

        // Stored before: only `lastConnected` moves (a patch of nothing
        // but `connected`), so the visitor's edits stay.
        if connections::get(&mut tx, DEMO_CONNECTION_ID)
            .await?
            .is_some()
        {
            let ticket = self.take_seq();
            let patch = ConnectionPatch {
                connected: true,
                ..ConnectionPatch::default()
            };
            let (_, stored) = update_connection_in(
                &mut tx,
                DEMO_CONNECTION_ID,
                &patch,
                &now,
                &SecretChanges::default(),
            )
            .await?;
            tx.commit().await?;
            let seq = self.announce(
                ticket,
                StoredKind::Connection,
                Some(stored.project_id.clone()),
                Some(vec![DEMO_CONNECTION_ID.to_string()]),
                origin,
            );
            return Ok(Seqd {
                value: stored,
                seq,
                projection: None,
            });
        }

        // The default project, else the first one, else a default project
        // made now.
        let mut project_ticket = None;
        let project_id = if projects::get(&mut tx, DEFAULT_PROJECT_ID).await?.is_some() {
            DEFAULT_PROJECT_ID.to_string()
        } else if let Some(first) = projects::names(&mut tx).await?.into_iter().next() {
            first.id
        } else {
            project_ticket = Some(self.take_seq());
            projects::insert_if_missing(&mut tx, &lib::default_project(&now)).await?;
            DEFAULT_PROJECT_ID.to_string()
        };
        let ticket = self.take_seq();
        let draft = demo_draft(project_id);
        lib::check_connection_draft(&draft, &limits)?;
        let mut row = lib::connection_from_draft(DEMO_CONNECTION_ID.to_string(), &draft, &now);
        let stored =
            insert_connection_in(&mut tx, &mut row, draft.rename_if_taken, &limits).await?;
        tx.commit().await?;
        if let Some(project_ticket) = project_ticket {
            self.announce(
                project_ticket,
                StoredKind::Project,
                None,
                Some(vec![DEFAULT_PROJECT_ID.to_string()]),
                origin,
            );
        }
        let seq = self.announce(
            ticket,
            StoredKind::Connection,
            Some(stored.project_id.clone()),
            Some(vec![DEMO_CONNECTION_ID.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: stored,
            seq,
            projection: None,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods, clippy::disallowed_types)]

    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use futures::future::BoxFuture;
    use futures::{FutureExt, StreamExt};
    use seaquel_runtime::{Executor, TokioExecutor};
    use seaquel_workspace::library::{ConnectionPatch, ProjectDraft, SecretChanges};

    use super::*;
    use crate::{ConnectPolicy, StoredKind, WorkspaceEvent, WorkspaceSpec};

    /// Tokio, with a wall clock that moves one second per reading, so two
    /// connects get different `lastConnected` times.
    struct Ticks(AtomicU64);

    impl Executor for Ticks {
        fn spawn(&self, future: BoxFuture<'static, ()>) {
            TokioExecutor.spawn(future)
        }
        fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
            TokioExecutor.sleep(duration)
        }
        fn unix_time(&self) -> Duration {
            Duration::from_secs(1_790_000_000 + self.0.fetch_add(1, Ordering::SeqCst))
        }
        fn monotonic(&self) -> Duration {
            TokioExecutor.monotonic()
        }
    }

    fn core() -> Core {
        crate::with_default_plugins()
            .connect_policy(ConnectPolicy::Unrestricted)
            .executor(Arc::new(Ticks(AtomicU64::new(0))))
            .build()
    }

    async fn open(core: &Core, dir: &std::path::Path) -> Arc<Workspace> {
        core.open_workspace(WorkspaceSpec::new(dir)).await.unwrap()
    }

    fn demo_origin() -> WriteOrigin {
        WriteOrigin::new(Some("demo"))
    }

    /// The events emitted so far, without waiting.
    fn drain(rx: &mut seaquel_runtime::BoxStream<'static, WorkspaceEvent>) -> Vec<WorkspaceEvent> {
        let mut out = Vec::new();
        while let Some(Some(e)) = rx.next().now_or_never() {
            out.push(e);
        }
        out
    }

    fn json(row: &PersistedConnection) -> serde_json::Value {
        serde_json::to_value(row).unwrap()
    }

    fn changed(
        events: &[WorkspaceEvent],
    ) -> Vec<(StoredKind, Option<Vec<String>>, Option<String>)> {
        events
            .iter()
            .filter_map(|e| match e {
                WorkspaceEvent::StorageChanged(c) => {
                    Some((c.kind, c.ids.clone(), c.origin.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn ensure_demo_connection_creates_once_and_keeps_labels() {
        let dir = tempfile::tempdir().unwrap();
        let core = core();
        let ws = open(&core, dir.path()).await;
        let mut events = ws.events();

        // A new file: the default project is made, then the row.
        let first = ws
            .ensure_demo_connection(&core, &demo_origin())
            .await
            .unwrap();
        let row = &first.value;
        assert_eq!(row.id, DEMO_CONNECTION_ID);
        assert_eq!(row.project_id, "default-seaquel");
        assert_eq!(row.name, "Demo Database");
        assert_eq!(row.ty, "duckdb");
        assert_eq!(row.host, "browser");
        assert_eq!(row.port, 0.0);
        assert_eq!(row.database_name, "demo");
        assert_eq!(row.username, "");
        assert_eq!(row.label_ids, vec!["prod".to_string()]);
        assert!(!row.save_password);
        assert_eq!(row.connection_string, None);
        let first_connected = row.last_connected.clone().expect("connected now");
        assert_eq!(
            changed(&drain(&mut events)),
            vec![
                (
                    StoredKind::Project,
                    Some(vec!["default-seaquel".to_string()]),
                    Some("demo".to_string())
                ),
                (
                    StoredKind::Connection,
                    Some(vec![DEMO_CONNECTION_ID.to_string()]),
                    Some("demo".to_string())
                ),
            ]
        );
        assert_eq!(first.seq, ws.change_seq());

        // The visitor changes the labels and an AI flag.
        let patch: ConnectionPatch =
            serde_json::from_value(serde_json::json!({"labelIds": ["local"], "aiShareData": true}))
                .unwrap();
        ws.update_connection(
            &core,
            &demo_origin(),
            DEMO_CONNECTION_ID,
            patch,
            SecretChanges::default(),
        )
        .await
        .unwrap();
        drain(&mut events);

        // The next start keeps them and only marks the row connected.
        let again = ws
            .ensure_demo_connection(&core, &demo_origin())
            .await
            .unwrap();
        assert_eq!(again.value.label_ids, vec!["local".to_string()]);
        assert_eq!(again.value.ai_share_data, Some(true));
        assert_eq!(again.value.name, "Demo Database");
        let second_connected = again.value.last_connected.clone().unwrap();
        assert!(
            second_connected > first_connected,
            "{second_connected} {first_connected}"
        );
        assert_eq!(
            changed(&drain(&mut events)),
            vec![(
                StoredKind::Connection,
                Some(vec![DEMO_CONNECTION_ID.to_string()]),
                Some("demo".to_string())
            )]
        );

        // One row, as stored.
        let all = ws.list_connections().await.unwrap().value;
        assert_eq!(all.len(), 1);
        assert_eq!(json(&all[0]), json(&again.value));
        assert_eq!(ws.list_projects().await.unwrap().value.len(), 1);
    }

    #[tokio::test]
    async fn ensure_demo_connection_on_a_file_that_has_the_row() {
        // Today's demo file (S6): `demo-connection` in `default-seaquel`
        // with the `prod` label, stored by the TypeScript twin.
        let dir = tempfile::tempdir().unwrap();
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../seaquel-storage/tests/fixtures/sqljs/demo-2026-10-01.db"
            ),
            dir.path().join("seaquel.db"),
        )
        .unwrap();
        let core = core();
        let ws = open(&core, dir.path()).await;
        let before = ws
            .list_connections()
            .await
            .unwrap()
            .value
            .into_iter()
            .find(|c| c.id == DEMO_CONNECTION_ID)
            .expect("the fixture has the row");

        let after = ws
            .ensure_demo_connection(&core, &WriteOrigin::none())
            .await
            .unwrap()
            .value;
        assert_ne!(after.last_connected, before.last_connected);
        let mut expected = before.clone();
        expected.last_connected = after.last_connected.clone();
        assert_eq!(json(&after), json(&expected));
        assert_eq!(ws.list_connections().await.unwrap().value.len(), 1);
    }

    #[tokio::test]
    async fn two_concurrent_calls_store_one_row() {
        // The page could start twice at once (a reload racing a slow
        // start): the write turn orders them, so the second finds the row
        // the first stored. One row, no "Demo Database (2)", and each call
        // announces its own write once.
        let dir = tempfile::tempdir().unwrap();
        let core = core();
        let ws = open(&core, dir.path()).await;
        let mut events = ws.events();
        let origin = demo_origin();
        let (a, b) = tokio::join!(
            ws.ensure_demo_connection(&core, &origin),
            ws.ensure_demo_connection(&core, &origin)
        );
        let (a, b) = (a.unwrap().value, b.unwrap().value);
        assert_eq!(
            (a.id.as_str(), b.id.as_str()),
            (DEMO_CONNECTION_ID, DEMO_CONNECTION_ID)
        );
        assert_eq!(
            (a.name.as_str(), b.name.as_str()),
            ("Demo Database", "Demo Database")
        );
        let all = ws.list_connections().await.unwrap().value;
        assert_eq!(all.len(), 1);
        assert_eq!(ws.list_projects().await.unwrap().value.len(), 1);
        let kinds: Vec<StoredKind> = changed(&drain(&mut events))
            .into_iter()
            .map(|(kind, ids, _)| {
                if kind == StoredKind::Connection {
                    assert_eq!(ids, Some(vec![DEMO_CONNECTION_ID.to_string()]));
                }
                kind
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                StoredKind::Project,
                StoredKind::Connection,
                StoredKind::Connection
            ]
        );
    }

    #[tokio::test]
    async fn without_the_duckdb_engine_it_is_engine_not_available() {
        let dir = tempfile::tempdir().unwrap();
        let core = crate::with_plugins(|id| id != "duckdb")
            .connect_policy(ConnectPolicy::Unrestricted)
            .executor(Arc::new(Ticks(AtomicU64::new(0))))
            .build();
        let ws = open(&core, dir.path()).await;
        let mut events = ws.events();
        let err = ws
            .ensure_demo_connection(&core, &demo_origin())
            .await
            .unwrap_err();
        assert_eq!(err.code, "ENGINE_NOT_AVAILABLE");
        // Refused before anything was written.
        assert!(ws.list_connections().await.unwrap().value.is_empty());
        assert!(ws.list_projects().await.unwrap().value.is_empty());
        assert!(changed(&drain(&mut events)).is_empty());
    }

    #[tokio::test]
    async fn ensure_demo_connection_goes_to_the_first_project_without_a_default_one() {
        let dir = tempfile::tempdir().unwrap();
        let core = core();
        let ws = open(&core, dir.path()).await;
        let made = ws
            .create_project(
                &core,
                &WriteOrigin::none(),
                ProjectDraft {
                    name: "Mine".to_string(),
                    description: None,
                    rename_if_taken: false,
                },
            )
            .await
            .unwrap()
            .value;
        let row = ws
            .ensure_demo_connection(&core, &WriteOrigin::none())
            .await
            .unwrap()
            .value;
        assert_eq!(row.project_id, made.id);
        assert_eq!(ws.list_projects().await.unwrap().value.len(), 1);
    }
}
