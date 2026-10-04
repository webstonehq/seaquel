//! Task 3 against a real Core: a data dir the "app" made, opened by the TUI
//! as a second process, driven through `update` and the runner with scripted
//! keys (`testing::harness`). SQLite always; Postgres and SSH behind the
//! `SEAQUEL_TEST_*` variables, as the engine suites (CI sets them, and
//! `SEAQUEL_TEST_REQUIRE_ENGINES=1` turns a missing one into a failure).
//! Secrets live in `MemoryStore`s, known_hosts in a temp file.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::KeyCode;
use futures::StreamExt;
use seaquel_core::domain::library::SecretChanges;
use seaquel_core::secrets::{MemoryStore, SecretError, SecretOp, SecretStore};
use seaquel_core::{ConnectRequest, StoredKind, WorkspaceEvent};

use crate::state::app::{Conn, Load, Modal, Panel, SavedTab};
use crate::state::panels::{Row, TableKind};
use crate::state::secrets::SecretKind;
use crate::testing::core::{connection, connection_with, project, saved_query, Seed};
use crate::testing::harness::{Harness, HarnessOptions};

/// `SEAQUEL_TEST_<name>`, or `None` to skip (a failure under
/// `SEAQUEL_TEST_REQUIRE_ENGINES=1`).
pub(super) fn live(name: &str) -> Option<serde_json::Value> {
    let var = format!("SEAQUEL_TEST_{name}");
    match std::env::var(&var) {
        Ok(raw) => Some(serde_json::from_str(&raw).unwrap_or_else(|_| panic!("{var} isn't JSON"))),
        Err(_) if std::env::var("SEAQUEL_TEST_REQUIRE_ENGINES").as_deref() == Ok("1") => {
            panic!("{var} is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => {
            eprintln!("skipping: {var} is not set");
            None
        }
    }
}

fn connected(m: &crate::state::app::Model) -> bool {
    matches!(m.conn, Conn::Connected { .. })
}

/// A SQLite file with two tables and a view, made by the app's Core.
async fn sqlite_seed() -> (Seed, String, String) {
    let seed = Seed::new().await;
    let file = seed.path().join("shop.db");
    let file_text = file.to_string_lossy().into_owned();
    let (project_id, conn) = seed
        .with(|core, ws| async move {
            let project_id = project(&core, &ws, "Shop").await;
            let conn = connection(
                &core,
                &ws,
                serde_json::json!({"projectId": project_id, "name": "shop", "type": "sqlite",
                                   "databaseName": file_text}),
            )
            .await;
            let id = ws
                .connect(
                    &core,
                    ConnectRequest::saved(&conn).with_create_if_missing(true),
                )
                .await
                .unwrap();
            for sql in [
                "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT)",
                "CREATE TABLE invoices (id INTEGER PRIMARY KEY, total REAL)",
                "CREATE VIEW big AS SELECT * FROM invoices WHERE total > 100",
            ] {
                ws.execute(&core, &id, sql, Vec::new()).await.unwrap();
            }
            saved_query(
                &core,
                &ws,
                &project_id,
                "all invoices",
                "SELECT * FROM invoices",
                None,
            )
            .await;
            saved_query(&core, &ws, &project_id, "q4", "SELECT 4", Some("reports")).await;
            (project_id, conn)
        })
        .await;
    (seed, project_id, conn)
}

#[tokio::test]
async fn a_sqlite_connection_fills_panels_one_to_three() {
    let (seed, project_id, conn) = sqlite_seed().await;
    let mut h = Harness::open(HarnessOptions {
        connection: Some("shop"),
        ..HarnessOptions::new(seed.path(), seed.store.clone())
    })
    .await;
    h.until("connected and listed", |m| {
        connected(m) && m.schema_load == Load::Loaded && m.saved_items.len() == 2
    })
    .await;
    assert_eq!(h.model.project.as_deref(), Some(project_id.as_str()));
    assert_eq!(h.model.conn.id(), Some(conn.as_str()));
    // Panel 2: one schema, two tables; a view under Views.
    let names: Vec<_> = h
        .model
        .schema
        .iter()
        .map(|t| (t.name.as_str(), t.kind))
        .collect();
    assert!(names.contains(&("invoices", TableKind::Table)), "{names:?}");
    assert!(names.contains(&("big", TableKind::View)), "{names:?}");
    assert_eq!(h.model.tables.len, 3, "main + 2 tables");
    assert_eq!(h.model.views.len, 2);
    assert!(h.model.log.last(10).any(|l| l.text == "schema tables"));
    // Panel 3: the project's saved queries, a folder last.
    assert_eq!(h.model.saved.len, 3);
    assert_eq!(h.model.history.len, 0);
    // The state file remembers it.
    h.until("remembered", |m| m.remember_dirty.is_none()).await;
    // Written on a blocking thread.
    let dir = seed.path().to_path_buf();
    let written = |_: &crate::state::app::Model| {
        crate::runtime::state_file::read(&dir)
            .is_ok_and(|r| r.last_connection.get(&project_id) == Some(&conn))
    };
    h.until("the state file", written).await;
    h.close().await;
}

#[tokio::test]
async fn the_picker_lists_and_connects_and_r_reloads() {
    let (seed, _, _) = sqlite_seed().await;
    let mut h = Harness::open(HarnessOptions::new(seed.path(), seed.store.clone())).await;
    assert!(matches!(h.model.modal, Some(Modal::Picker(_))));
    h.press(KeyCode::Enter);
    h.press(KeyCode::Enter);
    h.until("connected", |m| {
        connected(m) && m.schema_load == Load::Loaded
    })
    .await;
    // A table created meanwhile shows after `r`.
    let id = h.model.conn.core_id().unwrap().to_string();
    h.session
        .ws
        .execute(
            &h.session.core,
            &id,
            "CREATE TABLE later (x INT)",
            Vec::new(),
        )
        .await
        .unwrap();
    h.keys("2r");
    h.until("reloaded", |m| m.schema.iter().any(|t| t.name == "later"))
        .await;
    h.close().await;
}

#[tokio::test]
async fn an_external_change_refreshes_panel_three_within_two_polls() {
    let (seed, project_id, _) = sqlite_seed().await;
    let mut h = Harness::open(HarnessOptions {
        connection: Some("shop"),
        ..HarnessOptions::new(seed.path(), seed.store.clone())
    })
    .await;
    h.until("listed", |m| m.saved_items.len() == 2).await;
    // The app, a second process here (its own Core and storage on the file),
    // saves a query.
    let pid = project_id.clone();
    let started = std::time::Instant::now();
    seed.with(|core, ws| async move {
        saved_query(&core, &ws, &pid, "from the app", "SELECT 'app'", None).await;
    })
    .await;
    h.until("the app's query", |m| {
        m.saved_items.iter().any(|s| s.name == "from the app")
    })
    .await;
    let took = started.elapsed();
    assert!(
        took < crate::testing::harness::TEST_POLL * 2 + Duration::from_millis(500),
        "{took:?}"
    );
    h.close().await;
}

/// Two TUIs on one file: each its own origin, both polling; the app's
/// write reaches both.
#[tokio::test]
async fn two_tuis_at_once_both_hear_the_app() {
    let (seed, project_id, _) = sqlite_seed().await;
    let mut a = Harness::open(HarnessOptions {
        connection: Some("shop"),
        origin: "tui-aaaaaaaa",
        ..HarnessOptions::new(seed.path(), seed.store.clone())
    })
    .await;
    let mut b = Harness::open(HarnessOptions {
        connection: Some("shop"),
        origin: "tui-bbbbbbbb",
        ..HarnessOptions::new(seed.path(), seed.store.clone())
    })
    .await;
    assert_ne!(a.session.origin, b.session.origin);
    a.until("a listed", |m| m.saved_items.len() == 2).await;
    b.until("b listed", |m| m.saved_items.len() == 2).await;
    let pid = project_id.clone();
    seed.with(|core, ws| async move {
        saved_query(&core, &ws, &pid, "for both", "SELECT 2", None).await;
    })
    .await;
    a.until("a sees it", |m| m.saved_items.len() == 3).await;
    b.until("b sees it", |m| m.saved_items.len() == 3).await;
    a.close().await;
    b.close().await;
}

#[tokio::test]
async fn enter_on_a_saved_query_shows_its_sql_in_the_main_view() {
    let (seed, _, _) = sqlite_seed().await;
    let mut h = Harness::open(HarnessOptions {
        connection: Some("shop"),
        ..HarnessOptions::new(seed.path(), seed.store.clone())
    })
    .await;
    h.until("listed", |m| m.saved_items.len() == 2).await;
    h.keys("3");
    h.press(KeyCode::Enter);
    assert_eq!(h.model.focus, Panel::Main);
    assert_eq!(h.model.selected_saved_row(), Some(Row::Item(0)));
    h.keys("3]");
    assert_eq!(h.model.saved_tab, SavedTab::History);
    h.close().await;
}

// ── Passwords (Postgres) ──

/// A Postgres connection saved without its password, through the
/// connection string in `SEAQUEL_TEST_POSTGRES` (trust auth: any typed
/// password connects).
async fn postgres_seed(store: Arc<dyn SecretStore>, missing_db: bool) -> Option<(Seed, String)> {
    let config = live("POSTGRES")?;
    let mut url = config["connection_string"].as_str().unwrap().to_string();
    if missing_db {
        // The server answers at once (a closed port would wait out sqlx's
        // 30 s pool timeout).
        let at = url.rfind('/').unwrap();
        url = format!("{}/seaquel_tui_no_such_db", &url[..at]);
    }
    let seed = Seed {
        dir: tempfile::tempdir().unwrap(),
        store,
    };
    let conn = seed
        .with(|core, ws| async move {
            let p = project(&core, &ws, "Pg").await;
            connection(
                &core,
                &ws,
                serde_json::json!({"projectId": p, "name": "pg", "type": "postgres",
                                   "connectionString": url}),
            )
            .await
        })
        .await;
    Some((seed, conn))
}

/// Types `pw`, ticks the box when `save`, and connects.
fn type_password(h: &mut Harness, pw: &str, save: bool) {
    assert!(
        matches!(&h.model.modal, Some(Modal::Password(p)) if p.kind == SecretKind::Db),
        "{:?}",
        h.model.modal
    );
    h.keys(pw);
    if save {
        h.press(KeyCode::Tab);
    }
    h.press(KeyCode::Enter);
}

#[tokio::test]
async fn a_ticked_password_is_saved_through_connection_update() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let Some((seed, conn)) = postgres_seed(store.clone(), false).await else {
        return;
    };
    let mut h = Harness::open(HarnessOptions {
        connection: Some("pg"),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    let mut events = h.session.ws.events();
    type_password(&mut h, "typed-pw", true);
    h.until("connected", connected).await;
    h.until("saved", |m| m.saving.is_none() && m.modal.is_some())
        .await;
    let Some(Modal::Notice(n)) = &h.model.modal else {
        panic!("{:?}", h.model.modal)
    };
    assert!(n.0.starts_with("Saved."), "{}", n.0);
    assert_eq!(
        store.get(&format!("db:{conn}")).await.unwrap().as_deref(),
        Some("typed-pw")
    );
    let lib = h.session.library().await.unwrap();
    assert!(lib.connection(&conn).unwrap().save_password);
    // One connection write, with the TUI's origin.
    let mut writes = Vec::new();
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_millis(200), events.next()).await
    {
        if let WorkspaceEvent::StorageChanged(change) = event {
            if change.kind == StoredKind::Connection {
                writes.push(change);
            }
        }
    }
    assert_eq!(writes.len(), 1, "{writes:?}");
    assert_eq!(writes[0].origin.as_deref(), Some("tui-harness1"));
    assert_eq!(writes[0].ids.as_deref(), Some([conn.clone()].as_slice()));
    h.close().await;
}

#[tokio::test]
async fn an_unticked_password_is_used_but_not_saved() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let Some((seed, conn)) = postgres_seed(store.clone(), false).await else {
        return;
    };
    let mut h = Harness::open(HarnessOptions {
        connection: Some(conn.as_str()),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    type_password(&mut h, "typed-pw", false);
    h.until("connected", |m| {
        connected(m) && m.schema_load == Load::Loaded
    })
    .await;
    assert!(store.get(&format!("db:{conn}")).await.unwrap().is_none());
    assert!(
        !h.session
            .library()
            .await
            .unwrap()
            .connection(&conn)
            .unwrap()
            .save_password
    );
    assert!(h
        .effects
        .iter()
        .all(|e| !matches!(e, crate::state::app::Effect::SavePassword(_))));
    h.close().await;
}

#[tokio::test]
async fn a_failed_connect_saves_nothing_and_offers_a_retry() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let Some((seed, conn)) = postgres_seed(store.clone(), true).await else {
        return;
    };
    let mut h = Harness::open(HarnessOptions {
        connection: Some("pg"),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    type_password(&mut h, "typed-pw", true);
    h.until("failed", |m| matches!(m.modal, Some(Modal::Problem(_))))
        .await;
    assert_eq!(h.model.conn, Conn::Failed { id: conn.clone() });
    assert!(store.get(&format!("db:{conn}")).await.unwrap().is_none());
    assert!(
        !h.session
            .library()
            .await
            .unwrap()
            .connection(&conn)
            .unwrap()
            .save_password
    );
    h.keys("r");
    assert!(matches!(h.model.modal, Some(Modal::Password(_))));
    h.close().await;
}

/// A store whose writes fail (a denied keychain dialog).
struct RefusesWrites(MemoryStore);

#[seaquel_runtime::async_trait]
impl SecretStore for RefusesWrites {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        self.0.get(key).await
    }
    async fn set(&self, key: &str, _: &str) -> Result<(), SecretError> {
        Err(SecretError::Store {
            op: SecretOp::Set,
            key: key.to_string(),
            message: "the user denied it".into(),
        })
    }
    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.0.delete(key).await
    }
}

#[tokio::test]
async fn a_store_that_refuses_the_save_leaves_the_flag_off_and_the_connection_up() {
    let store: Arc<dyn SecretStore> = Arc::new(RefusesWrites(MemoryStore::new()));
    let Some((seed, conn)) = postgres_seed(store.clone(), false).await else {
        return;
    };
    let mut h = Harness::open(HarnessOptions {
        connection: Some("pg"),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    type_password(&mut h, "typed-pw", true);
    h.until("answered", |m| {
        connected(m) && m.saving.is_none() && m.modal.is_some()
    })
    .await;
    let Some(Modal::Notice(n)) = &h.model.modal else {
        panic!("{:?}", h.model.modal)
    };
    assert!(n.0.contains("wasn't saved"), "{}", n.0);
    assert!(connected(&h.model));
    assert!(
        !h.session
            .library()
            .await
            .unwrap()
            .connection(&conn)
            .unwrap()
            .save_password
    );
    h.close().await;
}

/// A store with nothing behind it (a headless Linux host with no Secret
/// Service): every call fails as unavailable, and its writes are counted.
#[derive(Default)]
struct NoStore {
    sets: AtomicUsize,
}

#[seaquel_runtime::async_trait]
impl SecretStore for NoStore {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        Err(SecretError::Unavailable {
            op: SecretOp::Get,
            key: key.to_string(),
            message: "no Secret Service".into(),
        })
    }
    async fn set(&self, key: &str, _: &str) -> Result<(), SecretError> {
        self.sets.fetch_add(1, Ordering::SeqCst);
        Err(SecretError::Unavailable {
            op: SecretOp::Set,
            key: key.to_string(),
            message: "no Secret Service".into(),
        })
    }
    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        Err(SecretError::Unavailable {
            op: SecretOp::Delete,
            key: key.to_string(),
            message: "no Secret Service".into(),
        })
    }
}

/// Probe F4 (behind `SEAQUEL_TEST_POSTGRES`, trust auth): a row whose
/// password the app saved, opened where there's no secret store. The TUI
/// asks for the password with "Save password" off and disabled, says why,
/// connects with what was typed, and writes nothing to the store.
#[tokio::test]
async fn an_unavailable_store_asks_for_the_password_and_saves_nothing() {
    let store = Arc::new(NoStore::default());
    let Some((seed, conn)) = saved_password_seed(store.clone()).await else {
        return;
    };
    store.sets.store(0, Ordering::SeqCst);
    let mut h = Harness::open(HarnessOptions {
        connection: Some("pg"),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    h.until("the prompt", |m| {
        matches!(m.modal, Some(Modal::Password(_)))
    })
    .await;
    let Some(Modal::Password(prompt)) = &h.model.modal else {
        unreachable!()
    };
    assert_eq!(prompt.kind, SecretKind::Db);
    assert!(!prompt.can_save && !prompt.save);
    assert_eq!(
        prompt.reason,
        Some(crate::state::text::store_unavailable(h.model.store))
    );
    // Tab (the box's toggle) can't tick it.
    type_password(&mut h, "typed-pw", true);
    h.until("connected", connected).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.model.saving.is_none());
    assert_eq!(store.sets.load(Ordering::SeqCst), 0, "nothing was saved");
    assert!(h.model.modal.is_none(), "{:?}", h.model.modal);
    assert_eq!(h.model.conn.id(), Some(conn.as_str()));
    h.close().await;
}

// ── The keychain wait ──

/// A store whose calls wait until released (a keychain dialog nobody has
/// answered yet).
#[derive(Default)]
struct Blocking {
    inner: MemoryStore,
    released: AtomicBool,
    entered: AtomicUsize,
    block_get: bool,
    block_set: bool,
}

impl Blocking {
    async fn hold(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        while !self.released.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

#[seaquel_runtime::async_trait]
impl SecretStore for Blocking {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        if self.block_get {
            self.hold().await;
        }
        self.inner.get(key).await
    }
    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        if self.block_set {
            self.hold().await;
        }
        self.inner.set(key, value).await
    }
    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.inner.delete(key).await
    }
}

/// A Postgres row whose password the store holds (so the connect reads it).
async fn saved_password_seed(store: Arc<dyn SecretStore>) -> Option<(Seed, String)> {
    let config = live("POSTGRES")?;
    let url = config["connection_string"].as_str().unwrap().to_string();
    let seed = Seed {
        dir: tempfile::tempdir().unwrap(),
        store: Arc::new(MemoryStore::new()),
    };
    let conn = seed
        .with(|core, ws| async move {
            let p = project(&core, &ws, "Pg").await;
            connection_with(
                &core,
                &ws,
                serde_json::json!({"projectId": p, "name": "pg", "type": "postgres",
                                   "connectionString": url, "savePassword": true}),
                SecretChanges::default(),
            )
            .await
        })
        .await;
    store.set(&format!("db:{conn}"), "stored-pw").await.ok();
    Some((seed, conn))
}

#[tokio::test]
async fn a_pending_keychain_read_shows_the_box_and_esc_gives_up() {
    let blocking = Arc::new(Blocking {
        block_get: true,
        ..Blocking::default()
    });
    let Some((seed, conn)) = saved_password_seed(blocking.clone()).await else {
        return;
    };
    let mut h = Harness::open(HarnessOptions {
        connection: Some("pg"),
        ..HarnessOptions::new(seed.path(), blocking.clone())
    })
    .await;
    h.until("the box", |m| m.keychain_box()).await;
    let mut m = h.model.clone();
    m.size = (100, 30);
    // The log's lines carry the wall clock; the wording is macOS's on
    // every OS.
    m.log = Default::default();
    m.store = crate::state::text::Store::MacKeychain;
    crate::testing::snapshot::assert_snapshot(
        "keychain_wait_100x30",
        &crate::testing::snapshot::draw(&m),
    );
    h.press(KeyCode::Esc);
    let Some(Modal::Problem(p)) = &h.model.modal else {
        panic!("{:?}", h.model.modal)
    };
    assert_eq!(p.code, "SECRET_UNREADABLE");
    assert_eq!(h.model.conn, Conn::Failed { id: conn });
    blocking.released.store(true, Ordering::SeqCst);
    h.until("the box is gone", |m| m.keychain.is_none()).await;
    assert!(!connected(&h.model));
    // The dropped connect closed what it had opened.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(h.session.ws.connection_ids(&h.session.core).is_empty());
    h.close().await;
}

#[tokio::test]
async fn a_pending_keychain_write_shows_the_box_during_a_save() {
    let blocking = Arc::new(Blocking {
        block_set: true,
        ..Blocking::default()
    });
    let Some((seed, conn)) = postgres_seed(blocking.clone(), false).await else {
        return;
    };
    let mut h = Harness::open(HarnessOptions {
        connection: Some("pg"),
        ..HarnessOptions::new(seed.path(), blocking.clone())
    })
    .await;
    type_password(&mut h, "typed-pw", true);
    h.until("saving waits", |m| connected(m) && m.keychain_box())
        .await;
    assert_eq!(h.model.saving.as_deref(), Some(conn.as_str()));
    blocking.released.store(true, Ordering::SeqCst);
    h.until("saved", |m| m.saving.is_none()).await;
    assert_eq!(
        blocking
            .inner
            .get(&format!("db:{conn}"))
            .await
            .unwrap()
            .as_deref(),
        Some("typed-pw")
    );
    h.close().await;
}

// ── SSH (the compose `ssh` service) ──

/// OpenSSH's `SHA256:` fingerprint of a known_hosts key (base64 of the
/// key blob's SHA-256, unpadded).
fn key_fingerprint(key_base64: &str) -> String {
    use base64::Engine;
    use sha2::Digest;
    let blob = base64::engine::general_purpose::STANDARD
        .decode(key_base64)
        .unwrap();
    let digest = sha2::Sha256::digest(&blob);
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest)
    )
}

struct SshTarget {
    host: String,
    port: u16,
    remote_host: String,
    remote_port: u16,
}

fn ssh_target() -> Option<SshTarget> {
    let v = live("SSH")?;
    Some(SshTarget {
        host: v["host"].as_str().unwrap().into(),
        port: v["port"].as_u64().unwrap() as u16,
        remote_host: v["remote_host"].as_str().unwrap().into(),
        remote_port: v["remote_port"].as_u64().unwrap() as u16,
    })
}

fn key_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../e2e/test-databases/ssh")
        .join(name)
}

/// A Postgres row behind the compose bastion; password or key auth, nothing
/// saved.
async fn ssh_seed(store: Arc<dyn SecretStore>, auth: &str) -> Option<(Seed, String)> {
    let t = ssh_target()?;
    let seed = Seed {
        dir: tempfile::tempdir().unwrap(),
        store,
    };
    let key = key_file("id_ed25519_passphrase")
        .to_string_lossy()
        .into_owned();
    let auth = auth.to_string();
    let conn = seed
        .with(|core, ws| async move {
            let p = project(&core, &ws, "Bastion").await;
            let mut tunnel = serde_json::json!({"enabled": true, "host": t.host, "port": t.port,
                                                "username": "seaquel", "authMethod": auth});
            if auth == "key" {
                tunnel["keyPath"] = serde_json::json!(key);
            }
            connection(
                &core,
                &ws,
                serde_json::json!({"projectId": p, "name": "tunnelled", "type": "postgres",
                                   "host": t.remote_host, "port": t.remote_port,
                                   "databaseName": "seaquel_test", "username": "postgres",
                                   "savePassword": true, "sshTunnel": tunnel}),
            )
            .await
        })
        .await;
    Some((seed, conn))
}

#[tokio::test]
async fn an_unknown_host_is_trusted_with_its_fingerprint_and_the_ssh_password_saved() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let Some((seed, conn)) = ssh_seed(store.clone(), "password").await else {
        return;
    };
    let known = seed.path().join("known_hosts");
    std::fs::write(&known, "").unwrap();
    let mut h = Harness::open(HarnessOptions {
        connection: Some("tunnelled"),
        known_hosts: Some(&known),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    let Some(Modal::Password(p)) = &h.model.modal else {
        panic!("{:?}", h.model.modal)
    };
    assert_eq!(p.kind, SecretKind::Ssh);
    h.keys("seaquel-test-password");
    h.press(KeyCode::Tab);
    h.press(KeyCode::Enter);
    h.until("asked to trust", |m| {
        matches!(m.modal, Some(Modal::Trust(_)))
    })
    .await;
    let Some(Modal::Trust(t)) = &h.model.modal else {
        unreachable!()
    };
    assert!(t.fingerprint.starts_with("SHA256:"), "{}", t.fingerprint);
    let trusted = t.fingerprint.clone();
    let target = ssh_target().unwrap();
    assert_eq!(
        std::fs::read_to_string(&known).unwrap(),
        "",
        "nothing written yet"
    );
    h.keys("t");
    h.until("connected", |m| connected(m) && m.saving.is_none())
        .await;
    let line = std::fs::read_to_string(&known).unwrap();
    let entries: Vec<_> = line.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(entries.len(), 1, "{line}");
    // `[host]:port algo key`, and the key is the one the dialog showed.
    let mut fields = entries[0].split_whitespace();
    assert_eq!(
        fields.next(),
        Some(format!("[{}]:{}", target.host, target.port).as_str()),
        "{line}"
    );
    let (_algo, key) = (fields.next().unwrap(), fields.next().unwrap());
    assert_eq!(key_fingerprint(key), trusted, "{line}");
    assert_eq!(
        store.get(&format!("ssh:{conn}")).await.unwrap().as_deref(),
        Some("seaquel-test-password")
    );
    let row = h.session.library().await.unwrap();
    assert!(row.connection(&conn).unwrap().save_ssh_password);
    h.close().await;
}

#[tokio::test]
async fn cancelling_the_trust_dialog_writes_nothing() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let Some((seed, conn)) = ssh_seed(store.clone(), "password").await else {
        return;
    };
    let known = seed.path().join("known_hosts");
    std::fs::write(&known, "").unwrap();
    let mut h = Harness::open(HarnessOptions {
        connection: Some(conn.as_str()),
        known_hosts: Some(&known),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    h.keys("seaquel-test-password");
    h.press(KeyCode::Tab);
    h.press(KeyCode::Enter);
    h.until("asked to trust", |m| {
        matches!(m.modal, Some(Modal::Trust(_)))
    })
    .await;
    h.press(KeyCode::Esc);
    assert_eq!(h.model.modal, None);
    assert_eq!(std::fs::read_to_string(&known).unwrap(), "");
    assert!(store.get(&format!("ssh:{conn}")).await.unwrap().is_none());
    h.close().await;
}

#[tokio::test]
async fn a_changed_host_key_is_refused_with_no_trust() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let Some((seed, _)) = ssh_seed(store.clone(), "password").await else {
        return;
    };
    let t = ssh_target().unwrap();
    // A different key recorded for the bastion.
    let known = seed.path().join("known_hosts");
    let other = std::fs::read_to_string(key_file("id_ed25519.pub")).unwrap();
    let mut parts = other.split_whitespace();
    let (algo, key) = (parts.next().unwrap(), parts.next().unwrap());
    std::fs::write(&known, format!("[{}]:{} {algo} {key}\n", t.host, t.port)).unwrap();
    let before = std::fs::read_to_string(&known).unwrap();
    let mut h = Harness::open(HarnessOptions {
        connection: Some("tunnelled"),
        known_hosts: Some(&known),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    h.keys("seaquel-test-password");
    h.press(KeyCode::Enter);
    h.until("refused", |m| matches!(m.modal, Some(Modal::Problem(_))))
        .await;
    let Some(Modal::Problem(p)) = &h.model.modal else {
        unreachable!()
    };
    assert_eq!(p.code, "HOST_KEY_MISMATCH");
    assert!(p.retry.is_none());
    h.keys("t");
    assert!(matches!(h.model.modal, Some(Modal::Problem(_))));
    assert_eq!(std::fs::read_to_string(&known).unwrap(), before);
    h.close().await;
}

#[tokio::test]
async fn a_key_passphrase_is_asked_after_the_key_fails_to_load_and_saved_when_ticked() {
    let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::new());
    let Some((seed, conn)) = ssh_seed(store.clone(), "key").await else {
        return;
    };
    let known = seed.path().join("known_hosts");
    std::fs::write(&known, "").unwrap();
    let mut h = Harness::open(HarnessOptions {
        connection: Some("tunnelled"),
        known_hosts: Some(&known),
        ..HarnessOptions::new(seed.path(), store.clone())
    })
    .await;
    // The host first, then the key that doesn't load without its passphrase.
    h.until("asked to trust", |m| {
        matches!(m.modal, Some(Modal::Trust(_)))
    })
    .await;
    h.keys("t");
    h.until("refused", |m| matches!(m.modal, Some(Modal::Problem(_))))
        .await;
    h.keys("r");
    let Some(Modal::Password(p)) = &h.model.modal else {
        panic!("{:?}", h.model.modal)
    };
    assert_eq!(p.kind, SecretKind::SshKey);
    h.keys("seaquel-test-passphrase");
    h.press(KeyCode::Tab);
    h.press(KeyCode::Enter);
    h.until("connected and saved", |m| {
        connected(m) && m.saving.is_none()
    })
    .await;
    assert_eq!(
        store
            .get(&format!("ssh-key:{conn}"))
            .await
            .unwrap()
            .as_deref(),
        Some("seaquel-test-passphrase")
    );
    assert!(
        h.session
            .library()
            .await
            .unwrap()
            .connection(&conn)
            .unwrap()
            .save_ssh_key_passphrase
    );
    h.close().await;
}

#[tokio::test]
async fn a_connection_core_closes_shows_in_panel_one() {
    let (seed, _, conn) = sqlite_seed().await;
    let mut h = Harness::open(HarnessOptions {
        connection: Some("shop"),
        ..HarnessOptions::new(seed.path(), seed.store.clone())
    })
    .await;
    h.until("connected", connected).await;
    // Core closes the workspace's connections (as an eviction does): the
    // event reaches panel 1.
    h.session.ws.close_all(&h.session.core).await;
    h.until("closed", |m| matches!(m.conn, Conn::Closed { .. }))
        .await;
    assert_eq!(h.model.conn, Conn::Closed { id: conn });
    assert!(h.model.typed.is_empty());
    h.keys("2r");
    assert!(!h
        .effects
        .iter()
        .rev()
        .take(2)
        .any(|e| matches!(e, crate::state::app::Effect::LoadSchema { .. })));
    h.close().await;
}
