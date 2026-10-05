//! `Workspace::connect` and `Workspace::test`: a saved row or a form, plus
//! supplied and stored secrets, connected on Core and owned by the
//! workspace, with its SSH tunnel tied to the connection. Also ownership:
//! another workspace can't reach the connection or its streams, and
//! `close_all` closes everything a workspace owns.
//!
//! Storage is a temp dir, secrets a `MemoryStore` and known_hosts a temp
//! file; nothing touches the real keychain or `~/.ssh`. The live cases use
//! the e2e Docker databases and SSH container
//! (`e2e/test-databases/docker-compose.yml`), through the same
//! `SEAQUEL_TEST_<ENGINE>` and `SEAQUEL_TEST_SSH` variables as the engine
//! and SSH tests; without them they skip, unless
//! `SEAQUEL_TEST_REQUIRE_ENGINES` is set.
#![cfg(all(
    feature = "storage",
    feature = "secrets",
    feature = "ssh",
    feature = "workspace",
    feature = "engine-postgres",
    feature = "engine-mysql",
    feature = "engine-sqlite",
    feature = "engine-mssql"
))]

#[path = "common/duckdb.rs"]
mod duckdb_helper;

use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use seaquel_core::secrets::{MemoryStore, SecretError, SecretOp, SecretStore};
use seaquel_core::storage::{connections, projects};
use seaquel_core::{
    ConnectRequest, ConnectionForm, Core, CoreError, HostKeyPolicy, QueryOptions, StreamEvent,
    SuppliedSecrets, Workspace, WorkspaceSpec,
};
use seaquel_types::storage::{PersistedConnection, PersistedProject};
use serde_json::{json, Value};

const PROJECT: &str = "p1";

struct Fixture {
    dir: tempfile::TempDir,
    known_hosts: std::path::PathBuf,
    core: Core,
    ws: Arc<Workspace>,
    store: Arc<MemoryStore>,
    /// The DuckDB helper's install, when the fixture has one.
    _helper: Option<tempfile::TempDir>,
}

/// A `MemoryStore` whose reads of some keys fail, as a denied keychain
/// prompt does.
struct FailingStore {
    inner: Arc<MemoryStore>,
    fail: Vec<String>,
    /// These fail as a store that isn't there (no Secret Service) does.
    unavailable: Vec<String>,
}

#[seaquel_runtime::async_trait]
impl SecretStore for FailingStore {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        if self.fail.iter().any(|k| k == key) {
            return Err(SecretError::Store {
                op: SecretOp::Get,
                key: key.to_string(),
                message: "the user denied access".to_string(),
            });
        }
        if self.unavailable.iter().any(|k| k == key) {
            return Err(SecretError::Unavailable {
                op: SecretOp::Get,
                key: key.to_string(),
                message: "no Secret Service".to_string(),
            });
        }
        self.inner.get(key).await
    }
    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        self.inner.set(key, value).await
    }
    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        self.inner.delete(key).await
    }
}

/// How the fixture's workspace gets its secrets.
enum Store {
    Memory,
    FailingOn(&'static [&'static str]),
    /// Reads of the first keys are refused, of the second unavailable.
    Unavailable(&'static [&'static str], &'static [&'static str]),
    None,
}

async fn fixture() -> Fixture {
    fixture_with(Store::Memory).await
}

async fn fixture_with(kind: Store) -> Fixture {
    fixture_on(kind, seaquel_core::with_default_plugins(), None).await
}

/// The fixture with DuckDB through the helper (`common/duckdb.rs`), or
/// `None` when there is no helper to run.
async fn fixture_with_duckdb() -> Option<Fixture> {
    let (plugins, helper) = duckdb_helper::default_plugins();
    helper.as_ref()?;
    Some(fixture_on(Store::Memory, plugins, helper).await)
}

async fn fixture_on(
    kind: Store,
    plugins: seaquel_core::CoreBuilder,
    helper: Option<tempfile::TempDir>,
) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let core = plugins
        .ssh_known_hosts(&known_hosts)
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let store = Arc::new(MemoryStore::new());
    let spec = WorkspaceSpec::new(dir.path());
    let spec = match kind {
        Store::Memory => spec.with_secrets(store.clone()),
        Store::FailingOn(keys) => spec.with_secrets(Arc::new(FailingStore {
            inner: store.clone(),
            fail: keys.iter().map(|k| k.to_string()).collect(),
            unavailable: Vec::new(),
        })),
        Store::Unavailable(refused, missing) => spec.with_secrets(Arc::new(FailingStore {
            inner: store.clone(),
            fail: refused.iter().map(|k| k.to_string()).collect(),
            unavailable: missing.iter().map(|k| k.to_string()).collect(),
        })),
        Store::None => spec,
    };
    let ws = core.open_workspace(spec).await.unwrap();
    let project: PersistedProject = serde_json::from_value(json!({
        "id": PROJECT,
        "name": "Project",
        "createdAt": "2026-01-02T03:04:05.000Z",
        "updatedAt": "2026-01-02T03:04:05.000Z",
        "customLabels": [],
    }))
    .unwrap();
    projects::save(ws.storage(), &project).await.unwrap();
    Fixture {
        dir,
        known_hosts,
        core,
        ws,
        store,
        _helper: helper,
    }
}

impl Fixture {
    /// Save a row: `fields` over a Postgres row with `savePassword` on.
    async fn save(&self, id: &str, fields: Value) -> PersistedConnection {
        let mut row = json!({
            "id": id,
            "projectId": PROJECT,
            "name": format!("Saved {id}"),
            "type": "postgres",
            "host": "127.0.0.1",
            "port": 5432,
            "databaseName": "seaquel_test",
            "username": "",
            "savePassword": true,
            "saveSshPassword": false,
            "saveSshKeyPassphrase": false,
            "labelIds": [],
        });
        for (k, v) in fields.as_object().unwrap() {
            row[k] = v.clone();
        }
        let row: PersistedConnection = serde_json::from_value(row).unwrap();
        connections::save(self.ws.storage(), &row).await.unwrap();
        row
    }

    async fn secret(&self, key: &str, value: &str) {
        self.store.set(key, value).await.unwrap();
    }

    async fn connect(&self, id: &str, policy: HostKeyPolicy) -> Result<String, CoreError> {
        self.ws
            .connect(&self.core, ConnectRequest::saved(id).with_host_key(policy))
            .await
    }

    /// Connect, run `SELECT 1`, disconnect.
    async fn round_trip(&self, id: &str) {
        let connection_id = self
            .connect(id, HostKeyPolicy::KnownOnly)
            .await
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let result = self
            .ws
            .query(&self.core, &connection_id, "SELECT 1", vec![])
            .await
            .unwrap_or_else(|e| panic!("{id}: {e:?}"));
        assert_eq!(result.rows.len(), 1, "{id}");
        self.ws
            .disconnect(&self.core, &connection_id)
            .await
            .unwrap();
        assert_eq!(self.core.connection_count(), 0);
    }
}

/// A live database's `ConnectConfig` JSON from `SEAQUEL_TEST_<name>`.
fn live(name: &str) -> Option<Value> {
    let var = format!("SEAQUEL_TEST_{name}");
    match std::env::var(&var) {
        Ok(raw) => Some(serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{var}: {e}"))),
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("{var} is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => {
            eprintln!("skipping: {var} is not set");
            None
        }
    }
}

fn text(v: &Value, key: &str) -> String {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key}"))
        .to_string()
}

fn known_hosts_bytes(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

// ── Offline ──

#[tokio::test]
async fn an_unknown_id_is_connection_not_found() {
    let f = fixture().await;
    let err = f
        .connect("nope", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "CONNECTION_NOT_FOUND");
    assert_eq!(err.message, "Saved connection not found: nope");
}

#[tokio::test]
async fn a_password_that_isnt_saved_is_credentials_required() {
    let f = fixture().await;
    f.save(
        "c-nosave",
        json!({ "savePassword": false,
                "connectionString": "postgresql://alice@127.0.0.1:5432/seaquel_test" }),
    )
    .await;
    // A stale secret under a flag that's off is never read.
    f.secret("db:c-nosave", "stale-secret").await;
    let err = f
        .connect("c-nosave", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "CREDENTIALS_REQUIRED");
    assert!(err.message.contains("Saved c-nosave"), "{}", err.message);
    assert!(!err.message.contains("stale-secret"), "{}", err.message);
    assert_eq!(f.core.connection_count(), 0);
}

#[tokio::test]
async fn a_missing_ssh_password_gives_up_before_opening_a_tunnel() {
    let f = fixture().await;
    f.save(
        "c-ssh-nopw",
        json!({
            "connectionString": "postgresql://postgres@db.internal:5432/app",
            "saveSshPassword": true,
            "sshTunnel": { "enabled": true, "host": "127.0.0.1", "port": 1,
                           "username": "u", "authMethod": "password" },
        }),
    )
    .await;
    let err = f
        .connect("c-ssh-nopw", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "CREDENTIALS_REQUIRED");
    assert_eq!(f.core.ssh_tunnel_count(), 0);
}

#[tokio::test]
async fn a_saved_sqlite_file_connects() {
    let f = fixture().await;
    let file = f.dir.path().join("app.db");
    std::fs::write(&file, b"").unwrap();
    f.save(
        "c-lite",
        json!({ "type": "sqlite", "host": "localhost", "port": 0,
                "databaseName": file.to_str().unwrap(),
                "connectionString": format!("sqlite://{}", file.display()) }),
    )
    .await;
    f.round_trip("c-lite").await;
}

/// `ConnectRequest::restricted` (the MCP server) locks a saved DuckDB
/// connection's instance down: its own tables work, another file doesn't.
/// Off, it stays as the app has it. The DuckDB crate's
/// `restricted*.rs` tests cover the settings themselves. Through the
/// helper; skipped without one unless `SEAQUEL_TEST_REQUIRE_ENGINES`.
#[cfg(feature = "engine-duckdb-remote")]
#[tokio::test]
async fn a_restricted_duckdb_connection_reads_only_its_own_file() {
    let Some(f) = fixture_with_duckdb().await else {
        return;
    };
    let file = f.dir.path().join("app.duckdb");
    let secret = f.dir.path().join("secret.txt");
    std::fs::write(&secret, "top secret").unwrap();
    let config: seaquel_types::ConnectConfig = serde_json::from_value(json!({
        "driver": "duckdb", "path": file.to_str().unwrap(), "create_if_missing": true,
    }))
    .unwrap();
    let setup = f.core.connect(&config).await.unwrap().connection_id;
    f.core
        .execute(&setup, "CREATE TABLE t AS SELECT 42 AS a", vec![])
        .await
        .unwrap();
    f.core.disconnect(&setup).await.unwrap();

    f.save(
        "c-duck",
        json!({ "type": "duckdb", "host": "", "port": 0, "savePassword": false,
                "databaseName": file.to_str().unwrap() }),
    )
    .await;
    let read_secret = format!("SELECT content FROM read_text('{}')", secret.display());

    let restricted = ConnectRequest::saved("c-duck").with_restricted(true);
    let id = f.ws.connect(&f.core, restricted).await.unwrap();
    let rows =
        f.ws.query(&f.core, &id, "SELECT a FROM t", vec![])
            .await
            .unwrap();
    assert_eq!(rows.rows.len(), 1);
    let err =
        f.ws.query(&f.core, &id, &read_secret, vec![])
            .await
            .unwrap_err();
    assert!(err.message.contains("disabled by configuration"), "{err:?}");
    assert!(f
        .ws
        .execute(&f.core, &id, "INSTALL json", vec![])
        .await
        .is_err());
    assert!(f
        .ws
        .execute(&f.core, &id, "SET autoload_known_extensions = true", vec![])
        .await
        .is_err());
    f.ws.disconnect(&f.core, &id).await.unwrap();

    // Off: the file function works, as in the app.
    let id = f.connect("c-duck", HostKeyPolicy::KnownOnly).await.unwrap();
    let rows =
        f.ws.query(&f.core, &id, &read_secret, vec![])
            .await
            .unwrap();
    assert_eq!(rows.rows.len(), 1);
    f.ws.disconnect(&f.core, &id).await.unwrap();
}

#[tokio::test]
async fn a_missing_sqlite_file_isnt_created() {
    let f = fixture().await;
    let file = f.dir.path().join("missing.db");
    f.save(
        "c-lite-missing",
        json!({ "type": "sqlite", "host": "localhost", "port": 0,
                "databaseName": file.to_str().unwrap() }),
    )
    .await;
    assert!(f
        .connect("c-lite-missing", HostKeyPolicy::KnownOnly)
        .await
        .is_err());
    assert!(!file.exists());
}

#[tokio::test]
async fn an_unreadable_db_password_fails_before_connecting() {
    let f = fixture_with(Store::FailingOn(&["db:c-db-denied"])).await;
    f.save(
        "c-db-denied",
        json!({ "connectionString": "postgresql://alice@127.0.0.1:1/app" }),
    )
    .await;
    let err = f
        .connect("c-db-denied", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "SECRET_UNREADABLE", "{err}");
    assert!(err.message.contains("Saved c-db-denied"), "{}", err.message);
    assert!(
        err.message.contains("SECRET_STORE_ERROR"),
        "{}",
        err.message
    );
    // The message names the platform's store.
    let store = if cfg!(target_os = "macos") {
        "the keychain"
    } else if cfg!(windows) {
        "Windows Credential Manager"
    } else {
        "the system keyring (Secret Service)"
    };
    assert!(err.message.contains(store), "{}", err.message);
    assert_eq!(f.core.connection_count(), 0);
}

/// A store that isn't there (a headless Linux host with
/// no Secret Service) isn't a refusal: the connect fails with its own code,
/// before anything opens, so the interface asks for the password instead.
/// A refusal among the reads still wins.
#[tokio::test]
async fn an_unavailable_store_is_its_own_code() {
    let f = fixture_with(Store::Unavailable(&[], &["db:c-db-nostore"])).await;
    f.save(
        "c-db-nostore",
        json!({ "connectionString": "postgresql://alice@127.0.0.1:1/app" }),
    )
    .await;
    let err = f
        .connect("c-db-nostore", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "SECRET_STORE_UNAVAILABLE", "{err}");
    assert!(
        err.message.contains("Saved c-db-nostore"),
        "{}",
        err.message
    );
    assert_eq!(f.core.connection_count(), 0);
    // Supplied, the password wins and the store isn't read.
    let mut req = ConnectRequest::saved("c-db-nostore");
    req.secrets = SuppliedSecrets {
        db: Some("typed".into()),
        ..SuppliedSecrets::default()
    };
    let err = f.ws.connect(&f.core, req).await.unwrap_err();
    assert_ne!(err.code, "SECRET_STORE_UNAVAILABLE", "{err}");
    assert_ne!(err.code, "SECRET_UNREADABLE", "{err}");

    let f = fixture_with(Store::Unavailable(&["ssh-key:c-mixed"], &["db:c-mixed"])).await;
    f.save(
        "c-mixed",
        json!({
            "connectionString": "postgresql://alice@db.internal:5432/app",
            "saveSshKeyPassphrase": true,
            "sshTunnel": { "enabled": true, "host": "127.0.0.1", "port": 1,
                           "username": "u", "authMethod": "key",
                           "keyPath": "/nonexistent/id_ed25519" },
        }),
    )
    .await;
    let err = f
        .connect("c-mixed", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "SECRET_UNREADABLE", "{err}");
}

#[tokio::test]
async fn an_unreadable_key_passphrase_fails_before_the_tunnel() {
    let f = fixture_with(Store::FailingOn(&["ssh-key:c-key-denied"])).await;
    f.save(
        "c-key-denied",
        json!({
            "connectionString": "postgresql://alice@db.internal:5432/app",
            "saveSshKeyPassphrase": true,
            "sshTunnel": { "enabled": true, "host": "127.0.0.1", "port": 1,
                           "username": "u", "authMethod": "key",
                           "keyPath": "/nonexistent/id_ed25519" },
        }),
    )
    .await;
    f.secret("db:c-key-denied", "db-password").await;
    let err = f
        .connect("c-key-denied", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "SECRET_UNREADABLE", "{err}");
    assert!(
        err.message.contains("SSH key passphrase"),
        "{}",
        err.message
    );
    assert!(!err.message.contains("db-password"));
    assert_eq!(f.core.ssh_tunnel_count(), 0);
    assert_eq!(f.core.connection_count(), 0);
}

/// A workspace with no secret store (the web): a saved row whose
/// save flag is on and that gets no supplied password connects with none,
/// as a form does, instead of failing with `NO_SECRET_STORE`. The vault
/// supplies what it has; a trust-auth database needs nothing.
#[tokio::test]
async fn no_secret_store_connects_a_saved_row_with_no_password() {
    let f = fixture_with(Store::None).await;
    // MSSQL on a closed port: refused at once (sqlx would retry for 30 s).
    f.save(
        "c-no-store",
        json!({ "type": "mssql", "host": "127.0.0.1", "port": 1, "username": "sa",
                "savePassword": true }),
    )
    .await;
    let err = f
        .connect("c-no-store", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_ne!(err.code, "NO_SECRET_STORE", "{err}");
    assert_ne!(err.code, "SECRET_UNREADABLE", "{err}");
    assert_ne!(err.code, "CREDENTIALS_REQUIRED", "{err}");

    // The same fields as a form fail the same way: the driver was dialled.
    let form_req = ConnectRequest::form(form(json!({
        "type": "mssql", "host": "127.0.0.1", "port": 1, "username": "sa",
    })));
    let form_err = f.ws.connect(&f.core, form_req).await.unwrap_err();
    assert_eq!(err.code, form_err.code, "{err} / {form_err}");
    assert_eq!(f.core.connection_count(), 0);

    // SQLite reads no secret, so it connects.
    let file = f.dir.path().join("app.db");
    std::fs::write(&file, b"").unwrap();
    f.save(
        "c-no-store-lite",
        json!({ "type": "sqlite", "host": "localhost", "port": 0,
                "databaseName": file.to_str().unwrap(),
                "connectionString": format!("sqlite://{}", file.display()) }),
    )
    .await;
    f.round_trip("c-no-store-lite").await;
}

/// A form with `fields` over a SQLite form.
fn form(fields: Value) -> ConnectionForm {
    let mut form = json!({
        "name": "Form", "type": "sqlite", "host": "localhost", "port": 0,
        "databaseName": "", "username": "", "connectionString": "",
        "sshEnabled": false, "sshHost": "", "sshPort": 22, "sshUsername": "",
        "sshAuthMethod": "password", "sshKeyPath": "",
        "savePassword": false, "saveSshPassword": false, "saveSshKeyPassphrase": false,
    });
    for (k, v) in fields.as_object().unwrap() {
        form[k] = v.clone();
    }
    serde_json::from_value(form).unwrap()
}

/// A supplied password wins with `savePassword` off: no
/// `CREDENTIALS_REQUIRED`, and the store isn't read (a read of `db:` would
/// fail here with `SECRET_UNREADABLE`).
#[tokio::test]
async fn a_supplied_password_wins_with_save_password_off() {
    let f = fixture_with(Store::FailingOn(&["db:c-typed"])).await;
    // MSSQL on a closed port: refused at once (sqlx would retry for 30 s).
    f.save(
        "c-typed",
        json!({ "type": "mssql", "host": "127.0.0.1", "port": 1, "username": "sa",
                "savePassword": false }),
    )
    .await;
    let req = ConnectRequest::saved("c-typed").with_secrets(SuppliedSecrets::db("typed-secret"));
    let err = f.ws.connect(&f.core, req).await.unwrap_err();
    assert_ne!(err.code, "CREDENTIALS_REQUIRED", "{err}");
    assert_ne!(err.code, "SECRET_UNREADABLE", "{err}");
    assert!(!format!("{err} {err:?}").contains("typed-secret"), "{err}");
    assert_eq!(f.core.connection_count(), 0);
}

/// On a workspace with no store (the web), a supplied password is all a
/// saved row needs.
#[tokio::test]
async fn a_supplied_password_needs_no_store() {
    let f = fixture_with(Store::None).await;
    f.save(
        "c-web",
        json!({ "type": "mssql", "host": "127.0.0.1", "port": 1, "username": "sa" }),
    )
    .await;
    let req = ConnectRequest::saved("c-web").with_secrets(SuppliedSecrets::db("typed-secret"));
    let err = f.ws.connect(&f.core, req).await.unwrap_err();
    assert_ne!(err.code, "NO_SECRET_STORE", "{err}");
    assert_ne!(err.code, "CREDENTIALS_REQUIRED", "{err}");
}

#[tokio::test]
async fn a_form_connects_and_its_connection_belongs_to_the_workspace() {
    let f = fixture().await;
    let file = f.dir.path().join("new.db");
    let req = ConnectRequest::form(form(json!({ "databaseName": file.to_str().unwrap() })));
    // Missing and not asked to create: refused, nothing created.
    assert!(f.ws.connect(&f.core, req.clone()).await.is_err());
    assert!(!file.exists());
    let id =
        f.ws.connect(&f.core, req.with_create_if_missing(true))
            .await
            .unwrap();
    assert!(file.exists());
    assert_eq!(f.ws.connection_ids(&f.core), vec![id.clone()]);
    f.ws.query(&f.core, &id, "SELECT 1", vec![]).await.unwrap();
    f.ws.disconnect(&f.core, &id).await.unwrap();
    assert_eq!(f.core.connection_count(), 0);
}

#[tokio::test]
async fn test_opens_nothing_that_stays_open() {
    let f = fixture().await;
    let file = f.dir.path().join("t.db");
    std::fs::write(&file, b"").unwrap();
    let req = ConnectRequest::form(form(json!({ "databaseName": file.to_str().unwrap() })));
    f.ws.test(&f.core, req).await.unwrap();
    assert_eq!(f.core.connection_count(), 0);

    // SSH password auth without a password gives up before any tunnel.
    let req = ConnectRequest::form(form(json!({
        "type": "postgres", "host": "db.internal", "port": 5432, "databaseName": "app",
        "sshEnabled": true, "sshHost": "127.0.0.1", "sshPort": 1, "sshUsername": "u",
    })));
    let err = f.ws.test(&f.core, req).await.unwrap_err();
    assert_eq!(err.code, "CREDENTIALS_REQUIRED", "{err}");
    assert_eq!(f.core.ssh_tunnel_count(), 0);
}

/// A SQLite connection in `ws` with a table of many rows, for streams.
async fn sqlite_connection(f: &Fixture, ws: &Workspace, name: &str) -> String {
    let file = f.dir.path().join(format!("{name}.db"));
    let req = ConnectRequest::form(form(json!({ "databaseName": file.to_str().unwrap() })))
        .with_create_if_missing(true);
    let id = ws.connect(&f.core, req).await.unwrap();
    ws.execute(
        &f.core,
        &id,
        "CREATE TABLE IF NOT EXISTS t AS WITH RECURSIVE c(x) AS \
         (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 200000) SELECT x FROM c",
        vec![],
    )
    .await
    .unwrap();
    id
}

const MANY_ROWS: &str = "SELECT a.x, b.x FROM t a, t b";

fn not_found(code: &str) {
    assert_eq!(code, "CONNECTION_NOT_FOUND");
}

/// Workspace B can't query, execute, run a transaction, make an engine
/// call, stream, cancel or disconnect on workspace A's connection. Every
/// refusal is `CONNECTION_NOT_FOUND`, the answer for an id that doesn't
/// exist, and A's connection and stream keep working.
#[tokio::test]
async fn another_workspace_is_refused_on_every_operation() {
    let f = fixture().await;
    let b = f
        .core
        .open_workspace(WorkspaceSpec::new(f.dir.path().join("b")))
        .await
        .unwrap();
    assert_ne!(f.ws.id(), b.id());
    let a_id = sqlite_connection(&f, &f.ws, "a").await;

    not_found(
        &b.query(&f.core, &a_id, "SELECT 1", vec![])
            .await
            .unwrap_err()
            .code,
    );
    not_found(
        &b.execute(&f.core, &a_id, "DELETE FROM t", vec![])
            .await
            .unwrap_err()
            .code,
    );
    let stmt: seaquel_types::BatchStatement =
        serde_json::from_value(json!({ "sql": "DELETE FROM t" })).unwrap();
    not_found(
        &b.transaction(&f.core, &a_id, vec![stmt])
            .await
            .unwrap_err()
            .error
            .code,
    );
    not_found(&b.engine(&f.core, &a_id).err().unwrap().code);
    // A handle A made is still A's: B can't borrow it, and it checks on
    // every call.
    let handle = f.ws.engine(&f.core, &a_id).unwrap();
    assert!(handle.list_schemas().await.is_ok());
    let mut stream = b.query_stream(
        &f.core,
        "s1".into(),
        a_id.clone(),
        "SELECT 1".into(),
        vec![],
        QueryOptions::default(),
    );
    match stream.next().await {
        Some(StreamEvent::Error { code, .. }) => not_found(&code),
        other => panic!("{other:?}"),
    }
    assert!(stream.next().await.is_none());
    drop(stream);
    assert_eq!(b.stream_count(&f.core), 0);
    // The same answer as an id that doesn't exist.
    let unknown = b
        .query(&f.core, "sqlite-nope", "SELECT 1", vec![])
        .await
        .unwrap_err();
    let refused = b
        .query(&f.core, &a_id, "SELECT 1", vec![])
        .await
        .unwrap_err();
    assert_eq!(unknown.code, refused.code);
    assert_eq!(
        unknown.message.replace("sqlite-nope", "<id>"),
        refused.message.replace(&a_id, "<id>")
    );

    // A's stream "s1" runs; B's cancel of "s1" doesn't touch it.
    let mut a_stream = f.ws.query_stream(
        &f.core,
        "s1".into(),
        a_id.clone(),
        MANY_ROWS.into(),
        vec![],
        QueryOptions::default(),
    );
    assert!(matches!(a_stream.next().await, Some(StreamEvent::Batch(_))));
    b.cancel(&f.core, "s1");
    f.core.cancel_stream("s1"); // Core's own scope is another one too.
    assert!(matches!(a_stream.next().await, Some(StreamEvent::Batch(_))));
    assert_eq!(f.ws.stream_count(&f.core), 1);
    // A's own cancel ends it, without a terminal event.
    f.ws.cancel(&f.core, "s1");
    while let Some(event) = a_stream.next().await {
        assert!(matches!(event, StreamEvent::Batch(_)), "{event:?}");
    }
    drop(a_stream);

    // B can't disconnect it; A can.
    not_found(&b.disconnect(&f.core, &a_id).await.unwrap_err().code);
    assert_eq!(f.ws.connection_ids(&f.core), vec![a_id.clone()]);
    f.ws.query(&f.core, &a_id, "SELECT 1", vec![])
        .await
        .unwrap();
    f.ws.disconnect(&f.core, &a_id).await.unwrap();
    not_found(&f.ws.disconnect(&f.core, &a_id).await.unwrap_err().code);
    not_found(&handle.list_schemas().await.unwrap_err().code);
}

/// `close_all` cancels a workspace's streams and closes its connections,
/// and leaves another workspace's alone. Afterwards it can't connect.
#[tokio::test]
async fn close_all_leaves_nothing_behind() {
    let f = fixture().await;
    let b = f
        .core
        .open_workspace(WorkspaceSpec::new(f.dir.path().join("b")))
        .await
        .unwrap();
    let a1 = sqlite_connection(&f, &f.ws, "a1").await;
    let _a2 = sqlite_connection(&f, &f.ws, "a2").await;
    let b1 = sqlite_connection(&f, &b, "b1").await;
    let mut stream = f.ws.query_stream(
        &f.core,
        "s".into(),
        a1.clone(),
        MANY_ROWS.into(),
        vec![],
        QueryOptions::default(),
    );
    assert!(matches!(stream.next().await, Some(StreamEvent::Batch(_))));
    let mut b_stream = b.query_stream(
        &f.core,
        "s".into(),
        b1.clone(),
        MANY_ROWS.into(),
        vec![],
        QueryOptions::default(),
    );
    assert!(matches!(b_stream.next().await, Some(StreamEvent::Batch(_))));

    // `close_all` waits for the driver to get its pooled connection back,
    // which the cancelled stream hands over when polled: drain it alongside.
    let drain = async {
        let mut last = None;
        while let Some(event) = stream.next().await {
            last = Some(event);
        }
        last
    };
    let ((), last) = tokio::join!(f.ws.close_all(&f.core), drain);
    assert!(
        matches!(&last, None | Some(StreamEvent::Batch(_)))
            || matches!(&last, Some(StreamEvent::Error { code, .. }) if code == "CONNECTION_CLOSED"),
        "{last:?}"
    );
    drop(stream);
    assert!(f.ws.connection_ids(&f.core).is_empty());
    assert_eq!(f.ws.stream_count(&f.core), 0);

    // B is untouched.
    assert_eq!(b.connection_ids(&f.core), vec![b1.clone()]);
    assert!(matches!(b_stream.next().await, Some(StreamEvent::Batch(_))));
    drop(b_stream);
    assert_eq!(f.core.connection_count(), 1);

    let file = f.dir.path().join("late.db");
    let req = ConnectRequest::form(form(json!({ "databaseName": file.to_str().unwrap() })))
        .with_create_if_missing(true);
    let err = f.ws.connect(&f.core, req).await.unwrap_err();
    assert_eq!(err.code, "WORKSPACE_CLOSED", "{err}");

    // A cancel for a stream that hasn't started is remembered, except on a
    // closed workspace, where no stream can start any more.
    f.ws.cancel(&f.core, "not-started");
    assert_eq!(f.ws.remembered_cancel_count(&f.core), 0);
    b.cancel(&f.core, "not-started");
    assert_eq!(b.remembered_cancel_count(&f.core), 1);
    b.disconnect(&f.core, &b1).await.unwrap();
}

// ── Live: each server engine from a saved row ──

#[tokio::test]
async fn postgres_from_a_saved_row() {
    let Some(env) = live("POSTGRES") else { return };
    let f = fixture().await;
    // The stored string, passwordless: savePassword on with nothing saved.
    f.save(
        "c-pg",
        json!({ "connectionString": text(&env, "connection_string") }),
    )
    .await;
    f.round_trip("c-pg").await;
}

#[tokio::test]
async fn postgres_rebuilt_from_the_fields() {
    let Some(env) = live("POSTGRES") else { return };
    let url = text(&env, "connection_string");
    let f = fixture().await;
    // No stored string: the reconnect tab's rebuild, from the fields.
    let (host, port) = host_port(&url);
    f.save(
        "c-pg-rebuilt",
        json!({ "host": host, "port": port, "username": "postgres",
                "databaseName": "seaquel_test", "sslMode": "disable" }),
    )
    .await;
    f.round_trip("c-pg-rebuilt").await;
}

#[tokio::test]
async fn mysql_from_a_saved_row() {
    let Some(env) = live("MYSQL") else { return };
    let f = fixture().await;
    f.save(
        "c-my",
        json!({ "type": "mysql", "connectionString": text(&env, "connection_string") }),
    )
    .await;
    f.round_trip("c-my").await;
}

#[tokio::test]
async fn mariadb_from_a_saved_row() {
    let Some(env) = live("MARIADB") else { return };
    let f = fixture().await;
    // The app stores MariaDB strings as `mariadb://`, and the mysql driver
    // takes them.
    let url = text(&env, "connection_string").replacen("mysql://", "mariadb://", 1);
    f.save(
        "c-maria",
        json!({ "type": "mariadb", "connectionString": url }),
    )
    .await;
    f.round_trip("c-maria").await;
}

#[tokio::test]
async fn mssql_from_a_saved_row() {
    let Some(env) = live("MSSQL") else { return };
    let f = fixture().await;
    // encrypt and trust_cert as the env has them: `prefer`.
    assert_eq!(env["encrypt"], json!(true));
    assert_eq!(env["trust_cert"], json!(true));
    f.save(
        "c-ms",
        json!({ "type": "mssql", "host": text(&env, "host"), "port": env["port"],
                "databaseName": "master", "username": text(&env, "username"),
                "sslMode": "prefer" }),
    )
    .await;
    f.secret("db:c-ms", &text(&env, "password")).await;
    f.round_trip("c-ms").await;
}

#[tokio::test]
async fn a_wrong_saved_password_never_reaches_the_error() {
    let Some(env) = live("MYSQL") else { return };
    let f = fixture().await;
    f.save(
        "c-my-wrong",
        json!({ "type": "mysql", "connectionString": text(&env, "connection_string") }),
    )
    .await;
    let secret = "wr0ng-S3cret%pw";
    f.secret("db:c-my-wrong", secret).await;
    let err = f
        .connect("c-my-wrong", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    let text = format!("{err} {err:?}");
    assert!(!text.contains("wr0ng-S3cret"), "{text}");
    assert!(!text.contains("wr0ng-S3cret%25pw"), "{text}");
}

// ── Live: SSH ──

struct Ssh {
    host: String,
    port: u16,
    remote_host: String,
    remote_port: u16,
}

fn live_ssh() -> Option<Ssh> {
    let v = live("SSH")?;
    let port = |k: &str| u16::try_from(v[k].as_u64().expect(k)).unwrap();
    Some(Ssh {
        host: text(&v, "host"),
        port: port("port"),
        remote_host: text(&v, "remote_host"),
        remote_port: port("remote_port"),
    })
}

/// A Postgres row through the SSH container, its SSH password saved.
async fn save_ssh_row(f: &Fixture, id: &str, ssh: &Ssh, database: &str) {
    f.save(
        id,
        json!({
            "host": ssh.remote_host,
            "port": ssh.remote_port,
            "databaseName": database,
            "connectionString":
                format!("postgresql://postgres@{}:{}/{database}", ssh.remote_host, ssh.remote_port),
            "saveSshPassword": true,
            "sshTunnel": { "enabled": true, "host": ssh.host, "port": ssh.port,
                           "username": "seaquel", "authMethod": "password" },
        }),
    )
    .await;
    f.secret(&format!("ssh:{id}"), "seaquel-test-password")
        .await;
}

/// The fingerprint in an `UNKNOWN_HOST_KEY` message.
fn fingerprint(message: &str) -> String {
    let at = message.find("SHA256:").expect("a fingerprint");
    message[at..]
        .split(|c: char| c.is_whitespace() || c == ')')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn ssh_known_only_refuses_an_unknown_host_and_writes_nothing() {
    let Some(ssh) = live_ssh() else { return };
    let f = fixture().await;
    save_ssh_row(&f, "c-ssh-unknown", &ssh, "postgres").await;
    std::fs::write(&f.known_hosts, b"").unwrap();

    let err = f
        .connect("c-ssh-unknown", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "UNKNOWN_HOST_KEY", "{err}");
    assert!(
        err.message.contains("Saved c-ssh-unknown"),
        "{}",
        err.message
    );
    assert!(err.message.contains("Seaquel app"), "{}", err.message);
    assert!(!err.message.contains("seaquel-test-password"));
    assert_eq!(known_hosts_bytes(&f.known_hosts).as_deref(), Some(&b""[..]));
    assert_eq!(f.core.ssh_tunnel_count(), 0);
    assert_eq!(f.core.connection_count(), 0);
}

#[tokio::test]
async fn ssh_through_a_known_host_and_disconnect_closes_the_tunnel() {
    let Some(ssh) = live_ssh() else { return };
    let f = fixture().await;
    save_ssh_row(&f, "c-ssh", &ssh, "postgres").await;

    // Put the host's key into the temp known_hosts: the approval the GUI's
    // trust prompt gives (`HostKeyPolicy::Trust`).
    let err = f
        .connect("c-ssh", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    let trusted = f
        .connect("c-ssh", HostKeyPolicy::Trust(fingerprint(&err.message)))
        .await
        .expect("connect trusting the approved key");
    assert!(known_hosts_bytes(&f.known_hosts).is_some_and(|b| !b.is_empty()));
    f.ws.disconnect(&f.core, &trusted).await.unwrap();
    assert_eq!(f.core.ssh_tunnel_count(), 0);
    let recorded = known_hosts_bytes(&f.known_hosts);

    // Now the host is known: KnownOnly connects, and writes nothing.
    let id = f
        .connect("c-ssh", HostKeyPolicy::KnownOnly)
        .await
        .expect("connect through a known host");
    assert_eq!(f.core.ssh_tunnel_count(), 1);
    f.ws.query(&f.core, &id, "SELECT 1", vec![])
        .await
        .expect("query through the tunnel");
    assert_eq!(known_hosts_bytes(&f.known_hosts), recorded);

    f.ws.disconnect(&f.core, &id).await.unwrap();
    assert_eq!(f.core.ssh_tunnel_count(), 0, "disconnect closes the tunnel");
    assert_eq!(f.core.connection_count(), 0);
}

#[tokio::test]
async fn a_failed_connect_closes_the_tunnel() {
    let Some(ssh) = live_ssh() else { return };
    let f = fixture().await;
    save_ssh_row(&f, "c-ssh-bad-db", &ssh, "no_such_database").await;
    let err = f
        .connect("c-ssh-bad-db", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    let fp = fingerprint(&err.message);
    let err = f
        .connect("c-ssh-bad-db", HostKeyPolicy::Trust(fp))
        .await
        .unwrap_err();
    assert_ne!(err.code, "UNKNOWN_HOST_KEY", "{err}");
    assert_eq!(f.core.ssh_tunnel_count(), 0);
    assert_eq!(f.core.connection_count(), 0);
}

/// `host` and `port` from a `scheme://user@host:port/db` URL.
fn host_port(url: &str) -> (String, u16) {
    let authority = url.split("://").nth(1).unwrap().split('/').next().unwrap();
    let host_port = authority.rsplit('@').next().unwrap();
    let (host, port) = host_port.rsplit_once(':').unwrap();
    (host.to_string(), port.parse().unwrap())
}

/// A row whose tunnel opens but whose database never answers: the compose
/// file's `blackhole` service (and CI's) accepts the bastion's connection
/// and sends nothing. (A blackholed address such as `10.255.255.1` is
/// refused at once on some Docker networks.)
async fn save_hanging_ssh_row(f: &Fixture, id: &str, ssh: &Ssh) {
    save_ssh_row(f, id, ssh, "postgres").await;
    let mut row = connections::load_all(f.ws.storage())
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap();
    // The tunnel forwards to where the string points (row 7c).
    row.host = "blackhole".into();
    row.connection_string = Some("postgresql://postgres@blackhole:5432/postgres".into());
    connections::save(f.ws.storage(), &row).await.unwrap();
}

#[tokio::test]
async fn a_dropped_connect_closes_its_tunnel() {
    let Some(ssh) = live_ssh() else { return };
    let f = fixture().await;
    save_hanging_ssh_row(&f, "c-ssh-hang", &ssh).await;
    let err = f
        .connect("c-ssh-hang", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    let policy = HostKeyPolicy::Trust(fingerprint(&err.message));

    let mut pending = Box::pin(f.connect("c-ssh-hang", policy));
    let waited = tokio::time::timeout(std::time::Duration::from_secs(3), pending.as_mut()).await;
    assert!(
        waited.is_err(),
        "the connect should still be waiting: {waited:?}"
    );
    assert_eq!(f.core.ssh_tunnel_count(), 1, "the tunnel is open meanwhile");
    drop(pending);
    assert_eq!(
        f.core.ssh_tunnel_count(),
        0,
        "dropping the future closes it"
    );
    assert_eq!(f.core.connection_count(), 0);
}

#[tokio::test]
async fn two_connections_from_one_row_each_own_a_tunnel() {
    let Some(ssh) = live_ssh() else { return };
    let f = fixture().await;
    save_ssh_row(&f, "c-ssh-twice", &ssh, "postgres").await;
    let err = f
        .connect("c-ssh-twice", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    let policy = HostKeyPolicy::Trust(fingerprint(&err.message));

    let a = f.connect("c-ssh-twice", policy.clone()).await.unwrap();
    let b = f
        .connect("c-ssh-twice", HostKeyPolicy::KnownOnly)
        .await
        .unwrap();
    assert_ne!(a, b);
    assert_eq!(f.core.ssh_tunnel_count(), 2);

    f.ws.disconnect(&f.core, &a).await.unwrap();
    assert_eq!(f.core.ssh_tunnel_count(), 1);
    f.ws.query(&f.core, &b, "SELECT 1", vec![])
        .await
        .expect("the other connection keeps its tunnel");
    f.ws.disconnect(&f.core, &b).await.unwrap();
    assert_eq!(f.core.ssh_tunnel_count(), 0);
}

// ── Live: the connect fixes that needed a live check ──

/// Connect `req`, run `SELECT 1`, disconnect: `Ok` or the connect error.
async fn try_round_trip(f: &Fixture, req: ConnectRequest) -> Result<(), CoreError> {
    let id = f.ws.connect(&f.core, req).await?;
    let rows = f.ws.query(&f.core, &id, "SELECT 1", vec![]).await;
    f.ws.disconnect(&f.core, &id).await.unwrap();
    assert_eq!(rows.map_err(|e| format!("{e:?}")).unwrap().rows.len(), 1);
    Ok(())
}

/// A form's request: `fields` over [`form`].
fn form_req(fields: Value) -> ConnectRequest {
    ConnectRequest::form(form(fields))
}

/// Row 3: an empty user name goes to the driver as it is. sqlx accepts
/// `scheme://:pw@host` and uses its own default user: Postgres the client's
/// `PGUSER` or OS user, MySQL and MariaDB `root`. SQL Server has no default
/// login, so Core refuses an empty MSSQL username itself.
#[tokio::test]
async fn row_3_an_empty_user_name_on_each_engine() {
    let (Some(_), Some(my), Some(maria), Some(ms)) = (
        live("POSTGRES"),
        live("MYSQL"),
        live("MARIADB"),
        live("MSSQL"),
    ) else {
        return;
    };
    let f = fixture().await;
    let pw = || SuppliedSecrets::db("not-the-password");

    // Postgres (`add/pg-empty-username-with-password`, and the saved
    // `pg/empty-username`): the URL is accepted and the password sent; the
    // user is the client's default, which this server may not have.
    let pg_form = form_req(
        json!({ "type": "postgres", "host": "127.0.0.1", "port": 5432,
                                   "databaseName": "seaquel_test", "sslMode": "disable" }),
    );
    f.save(
        "c-pg-nouser",
        json!({ "connectionString": "postgresql://127.0.0.1:5432/seaquel_test" }),
    )
    .await;
    f.secret("db:c-pg-nouser", "not-the-password").await;
    for req in [
        pg_form.with_secrets(pw()),
        ConnectRequest::saved("c-pg-nouser"),
    ] {
        match try_round_trip(&f, req).await {
            Ok(()) => {}
            Err(e) => assert!(
                e.message.contains("role \"") && e.message.contains("does not exist"),
                "{e}"
            ),
        }
    }

    // MySQL and MariaDB (`mysql/empty-username`): the user is `root`.
    for env in [&my, &maria] {
        let (host, port) = host_port(&text(env, "connection_string"));
        let fields = json!({ "type": "mysql", "host": host, "port": port,
                             "databaseName": "seaquel_test" });
        try_round_trip(&f, form_req(fields.clone()))
            .await
            .expect("an empty user is root");
        let err = try_round_trip(&f, form_req(fields).with_secrets(pw()))
            .await
            .unwrap_err();
        assert!(
            err.message.contains("'root'") && err.message.contains("using password: YES"),
            "{err}"
        );
        assert!(!err.message.contains("not-the-password"), "{err}");
    }

    // SQL Server (`mssql/empty-username-no-string`) refuses an empty login
    // (18456), so Core doesn't send one.
    let raw: seaquel_types::ConnectConfig = serde_json::from_value(json!({
        "driver": "mssql", "host": text(&ms, "host"), "port": ms["port"], "database": "master",
        "username": "", "password": text(&ms, "password"), "encrypt": true, "trust_cert": true,
    }))
    .unwrap();
    let err = f.core.connect(&raw).await.err().unwrap();
    assert!(err.message.contains("Login failed for user ''"), "{err:?}");
    let req = form_req(
        json!({ "type": "mssql", "host": text(&ms, "host"), "port": ms["port"],
                               "databaseName": "master" }),
    )
    .with_secrets(SuppliedSecrets::db(text(&ms, "password")));
    let err = f.ws.connect(&f.core, req).await.unwrap_err();
    assert_eq!(err.code, "CREDENTIALS_REQUIRED", "{err}");
    assert!(err.message.contains("username"), "{err}");
    assert_eq!(f.core.connection_count(), 0);
}

/// The SSH container's password, and the compose service names it reaches
/// the other databases by.
const SSH_PASSWORD: &str = "seaquel-test-password";
const MARIADB_SERVICE: &str = "mariadb";
const MSSQL_SERVICE: &str = "sqlserver";

/// `fields` over a form through the SSH container.
fn ssh_form(ssh: &Ssh, fields: Value) -> ConnectRequest {
    let mut v = json!({ "sshEnabled": true, "sshHost": ssh.host, "sshPort": ssh.port,
                        "sshUsername": "seaquel", "sshAuthMethod": "password" });
    for (k, x) in fields.as_object().unwrap() {
        v[k] = x.clone();
    }
    form_req(v)
}

fn with_ssh_password(db: Option<&str>) -> SuppliedSecrets {
    SuppliedSecrets {
        db: db.map(str::to_string),
        ssh: Some(SSH_PASSWORD.into()),
        ssh_key: None,
    }
}

/// Records the SSH container's host key in the fixture's known_hosts, the
/// way the GUI's trust prompt does: `UNKNOWN_HOST_KEY`, then `Trust`.
async fn trust_ssh_host(f: &Fixture, ssh: &Ssh) {
    let req = ssh_form(
        ssh,
        json!({ "type": "postgres", "host": ssh.remote_host, "port": ssh.remote_port,
                "databaseName": "postgres", "username": "postgres" }),
    )
    .with_secrets(with_ssh_password(None));
    let err = f.ws.test(&f.core, req.clone()).await.unwrap_err();
    assert_eq!(err.code, "UNKNOWN_HOST_KEY", "{err}");
    let trusted = req.with_host_key(HostKeyPolicy::Trust(fingerprint(&err.message)));
    f.ws.test(&f.core, trusted).await.unwrap();
}

/// Row 6: through a tunnel, MSSQL dials 127.0.0.1 and checks the
/// certificate as the server's own name (`tls_server_name`;
/// `tests/tls_server_name.rs` in the MSSQL crate shows the name tiberius
/// sends). The test container's certificate is SQL Server's self-signed
/// fallback (`CN=SSL_Self_Signed_Fallback`, an X.509 v1 certificate with no
/// subject alternative names), so `require` and `verify-full` fail on the
/// certificate itself, exactly as they do without the tunnel, and never on
/// the name. `prefer` trusts it and connects.
#[tokio::test]
async fn row_6_mssql_over_ssh_checks_the_certificate_as_the_server() {
    let (Some(ssh), Some(ms)) = (live_ssh(), live("MSSQL")) else {
        return;
    };
    let f = fixture().await;
    trust_ssh_host(&f, &ssh).await;
    let password = text(&ms, "password");
    let over_ssh = |mode: &str| {
        ssh_form(
            &ssh,
            json!({ "type": "mssql", "host": MSSQL_SERVICE, "port": 1433,
                    "databaseName": "master", "username": "sa", "sslMode": mode }),
        )
        .with_secrets(with_ssh_password(Some(&password)))
    };
    try_round_trip(&f, over_ssh("prefer"))
        .await
        .expect("prefer over SSH");
    let direct = form_req(
        json!({ "type": "mssql", "host": text(&ms, "host"), "port": ms["port"],
                                  "databaseName": "master", "username": "sa",
                                  "sslMode": "require" }),
    )
    .with_secrets(SuppliedSecrets::db(password.clone()));
    let baseline = try_round_trip(&f, direct).await.unwrap_err();
    for mode in ["require", "verify-ca", "verify-full"] {
        let err = try_round_trip(&f, over_ssh(mode)).await.unwrap_err();
        assert_eq!(err.code, "TLS_ERROR", "{mode}: {err}");
        assert!(err.message.contains("invalid peer certificate"), "{err}");
        assert!(!err.message.contains("NotValidForName"), "{err}");
        assert_eq!(err.message, baseline.message, "{mode}");
        assert!(!err.message.contains(&password), "{err}");
    }
    assert_eq!(f.core.ssh_tunnel_count(), 0);
    assert_eq!(f.core.connection_count(), 0);
}

/// Row 9: MariaDB connects through the mysql driver with a `mariadb://`
/// string, built from the fields in every SSL mode, stored, and through the
/// SSH tunnel (the eight MariaDB cases' shapes).
#[tokio::test]
async fn row_9_the_mariadb_scheme() {
    let (Some(maria), Some(ssh)) = (live("MARIADB"), live_ssh()) else {
        return;
    };
    let f = fixture().await;
    let (host, port) = host_port(&text(&maria, "connection_string"));

    // `add/mariadb-fields`, `add/mariadb-fields-disable`, and `require`
    // (`mariadb/no-string-require`, saved below).
    for mode in ["prefer", "disable", "require"] {
        let req = form_req(json!({ "type": "mariadb", "host": host, "port": port,
                                   "databaseName": "seaquel_test", "username": "root",
                                   "sslMode": mode }));
        try_round_trip(&f, req)
            .await
            .unwrap_or_else(|e| panic!("{mode}: {e}"));
    }
    // `mariadb/stored-string`, `add/mariadb-paste`, `mariadb/no-string-require`.
    f.save(
        "c-maria-stored",
        json!({ "type": "mariadb",
                "connectionString": format!("mariadb://root@{host}:{port}/seaquel_test?ssl-mode=PREFERRED") }),
    )
    .await;
    f.save(
        "c-maria-fields",
        json!({ "type": "mariadb", "host": host, "port": port, "username": "root",
                "sslMode": "require" }),
    )
    .await;
    for id in ["c-maria-stored", "c-maria-fields"] {
        f.round_trip(id).await;
    }
    let paste = form_req(json!({ "type": "mariadb",
        "connectionString": format!("mariadb://root@{host}:{port}/seaquel_test") }));
    try_round_trip(&f, paste)
        .await
        .expect("a pasted mariadb://");

    // Through the tunnel: `mariadb/default-port-string` (no port: 3306),
    // `mariadb/ssh` and `test/mariadb-ssh-password`.
    trust_ssh_host(&f, &ssh).await;
    let stored = ssh_form(
        &ssh,
        json!({ "type": "mariadb", "host": MARIADB_SERVICE, "port": 3306,
                "connectionString":
                    format!("mariadb://root@{MARIADB_SERVICE}/seaquel_test?ssl-mode=DISABLED") }),
    )
    .with_secrets(with_ssh_password(None));
    try_round_trip(&f, stored)
        .await
        .expect("mariadb:// over SSH");
    let test = ssh_form(
        &ssh,
        json!({ "type": "mariadb", "host": MARIADB_SERVICE, "port": 3306,
                "databaseName": "seaquel_test", "username": "root", "sslMode": "require" }),
    )
    .with_secrets(with_ssh_password(None));
    f.ws.test(&f.core, test)
        .await
        .expect("test over SSH, require");
    assert_eq!(f.core.ssh_tunnel_count(), 0);
}

/// `close_all` also closes the SSH tunnels its connections own.
#[tokio::test]
async fn close_all_closes_tunnels() {
    let Some(ssh) = live_ssh() else { return };
    let f = fixture().await;
    trust_ssh_host(&f, &ssh).await;
    let req = ssh_form(
        &ssh,
        json!({ "type": "postgres", "host": ssh.remote_host, "port": ssh.remote_port,
                "databaseName": "postgres", "username": "postgres" }),
    )
    .with_secrets(with_ssh_password(None));
    f.ws.connect(&f.core, req.clone()).await.unwrap();
    f.ws.connect(&f.core, req).await.unwrap();
    assert_eq!(f.core.ssh_tunnel_count(), 2);
    f.ws.close_all(&f.core).await;
    assert_eq!(f.core.ssh_tunnel_count(), 0);
    assert_eq!(f.core.connection_count(), 0);
}

/// A store whose reads wait until released, to hold a connect in flight.
struct GateStore {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[seaquel_runtime::async_trait]
impl SecretStore for GateStore {
    async fn get(&self, _key: &str) -> Result<Option<String>, SecretError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(None)
    }
    async fn set(&self, _key: &str, _value: &str) -> Result<(), SecretError> {
        Ok(())
    }
    async fn delete(&self, _key: &str) -> Result<(), SecretError> {
        Ok(())
    }
}

/// A connect in flight when `close_all` runs ends closed: it returns
/// `WORKSPACE_CLOSED` and leaves no connection behind.
#[tokio::test]
async fn a_connect_in_flight_during_close_all_ends_closed() {
    let Some(env) = live("POSTGRES") else { return };
    let dir = tempfile::tempdir().unwrap();
    let core = seaquel_core::with_default_plugins()
        .ssh_known_hosts(dir.path().join("known_hosts"))
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let gate = Arc::new(GateStore {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let ws = core
        .open_workspace(WorkspaceSpec::new(dir.path()).with_secrets(gate.clone()))
        .await
        .unwrap();
    let project: PersistedProject = serde_json::from_value(json!({
        "id": PROJECT, "name": "Project", "customLabels": [],
        "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
    }))
    .unwrap();
    projects::save(ws.storage(), &project).await.unwrap();
    // `savePassword` on: the connect reads `db:` and waits at the gate.
    let row: PersistedConnection = serde_json::from_value(json!({
        "id": "c-race", "projectId": PROJECT, "name": "Race", "type": "postgres",
        "host": "127.0.0.1", "port": 5432, "databaseName": "seaquel_test", "username": "",
        "connectionString": text(&env, "connection_string"),
        "savePassword": true, "saveSshPassword": false, "saveSshKeyPassphrase": false,
        "labelIds": [],
    }))
    .unwrap();
    connections::save(ws.storage(), &row).await.unwrap();

    let connect = ws.connect(&core, ConnectRequest::saved("c-race"));
    let close = async {
        gate.entered.notified().await;
        ws.close_all(&core).await;
        gate.release.notify_one();
    };
    let (result, ()) = tokio::join!(connect, close);
    let err = result.unwrap_err();
    assert_eq!(err.code, "WORKSPACE_CLOSED", "{err}");
    assert_eq!(core.connection_count(), 0);
    assert!(ws.connection_ids(&core).is_empty());
}
