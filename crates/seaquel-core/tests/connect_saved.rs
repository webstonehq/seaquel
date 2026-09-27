//! `Workspace::connect_saved`: a saved row plus its secrets, connected on
//! Core, with its SSH tunnel tied to the connection.
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

use std::path::Path;
use std::sync::Arc;

use seaquel_core::secrets::{MemoryStore, SecretError, SecretOp, SecretStore};
use seaquel_core::storage::{connections, projects};
use seaquel_core::{Core, HostKeyPolicy, Workspace, WorkspaceSpec};
use seaquel_types::storage::{PersistedConnection, PersistedProject};
use serde_json::{json, Value};

const PROJECT: &str = "p1";

struct Fixture {
    dir: tempfile::TempDir,
    known_hosts: std::path::PathBuf,
    core: Core,
    ws: Arc<Workspace>,
    store: Arc<MemoryStore>,
}

/// A `MemoryStore` whose reads of some keys fail, as a denied keychain
/// prompt does.
struct FailingStore {
    inner: Arc<MemoryStore>,
    fail: Vec<String>,
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
    None,
}

async fn fixture() -> Fixture {
    fixture_with(Store::Memory).await
}

async fn fixture_with(kind: Store) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let known_hosts = dir.path().join("known_hosts");
    let core = seaquel_core::with_default_plugins()
        .ssh_known_hosts(&known_hosts)
        .build();
    let store = Arc::new(MemoryStore::new());
    let spec = WorkspaceSpec::new(dir.path());
    let spec = match kind {
        Store::Memory => spec.with_secrets(store.clone()),
        Store::FailingOn(keys) => spec.with_secrets(Arc::new(FailingStore {
            inner: store.clone(),
            fail: keys.iter().map(|k| k.to_string()).collect(),
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

    async fn connect(
        &self,
        id: &str,
        policy: HostKeyPolicy,
    ) -> Result<String, seaquel_core::CoreError> {
        self.ws.connect_saved(&self.core, id, policy).await
    }

    /// Connect, run `SELECT 1`, disconnect.
    async fn round_trip(&self, id: &str) {
        let connection_id = self
            .connect(id, HostKeyPolicy::KnownOnly)
            .await
            .unwrap_or_else(|e| panic!("{id}: {e}"));
        let result = self
            .core
            .query(&connection_id, "SELECT 1", vec![])
            .await
            .unwrap_or_else(|e| panic!("{id}: {e:?}"));
        assert_eq!(result.rows.len(), 1, "{id}");
        self.core.disconnect(&connection_id).await.unwrap();
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

/// `ConnectSavedOptions::restricted` (the MCP server) locks a saved DuckDB
/// connection's instance down: its own tables work, another file doesn't.
/// A bare `HostKeyPolicy` leaves it as the app has it. The DuckDB crate's
/// `restricted*.rs` tests cover the settings themselves.
#[cfg(feature = "engine-duckdb")]
#[tokio::test]
async fn a_restricted_duckdb_connection_reads_only_its_own_file() {
    use seaquel_core::ConnectSavedOptions;

    let f = fixture().await;
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

    let restricted = ConnectSavedOptions::new(HostKeyPolicy::KnownOnly).restricted(true);
    let id =
        f.ws.connect_saved(&f.core, "c-duck", restricted)
            .await
            .unwrap();
    let rows = f.core.query(&id, "SELECT a FROM t", vec![]).await.unwrap();
    assert_eq!(rows.rows.len(), 1);
    let err = f.core.query(&id, &read_secret, vec![]).await.unwrap_err();
    assert!(err.message.contains("disabled by configuration"), "{err:?}");
    assert!(f.core.execute(&id, "INSTALL json", vec![]).await.is_err());
    assert!(f
        .core
        .execute(&id, "SET autoload_known_extensions = true", vec![])
        .await
        .is_err());
    f.core.disconnect(&id).await.unwrap();

    // Off (a bare policy): the file function works, as in the app.
    let id = f.connect("c-duck", HostKeyPolicy::KnownOnly).await.unwrap();
    let rows = f.core.query(&id, &read_secret, vec![]).await.unwrap();
    assert_eq!(rows.rows.len(), 1);
    f.core.disconnect(&id).await.unwrap();
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
    assert!(err.message.contains("keychain"), "{}", err.message);
    assert_eq!(f.core.connection_count(), 0);
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

#[tokio::test]
async fn no_secret_store_fails_only_for_rows_that_need_a_secret() {
    let f = fixture_with(Store::None).await;
    f.save(
        "c-no-store",
        json!({ "connectionString": "postgresql://alice@127.0.0.1:1/app" }),
    )
    .await;
    let err = f
        .connect("c-no-store", HostKeyPolicy::KnownOnly)
        .await
        .unwrap_err();
    assert_eq!(err.code, "NO_SECRET_STORE", "{err}");
    assert!(err.message.contains("Saved c-no-store"), "{}", err.message);

    // SQLite reads no secret, so it still connects.
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
    f.core.disconnect(&trusted).await.unwrap();
    assert_eq!(f.core.ssh_tunnel_count(), 0);
    let recorded = known_hosts_bytes(&f.known_hosts);

    // Now the host is known: KnownOnly connects, and writes nothing.
    let id = f
        .connect("c-ssh", HostKeyPolicy::KnownOnly)
        .await
        .expect("connect through a known host");
    assert_eq!(f.core.ssh_tunnel_count(), 1);
    f.core
        .query(&id, "SELECT 1", vec![])
        .await
        .expect("query through the tunnel");
    assert_eq!(known_hosts_bytes(&f.known_hosts), recorded);

    f.core.disconnect(&id).await.unwrap();
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

/// A row whose tunnel opens but whose database never answers: the SSH
/// server's connect to a blackholed address hangs.
async fn save_hanging_ssh_row(f: &Fixture, id: &str, ssh: &Ssh) {
    save_ssh_row(f, id, ssh, "postgres").await;
    let mut row = connections::load_all(f.ws.storage())
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.id == id)
        .unwrap();
    row.host = "10.255.255.1".into();
    connections::save(f.ws.storage(), &row).await.unwrap();
}

#[tokio::test]
async fn a_dropped_connect_saved_closes_its_tunnel() {
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

    f.core.disconnect(&a).await.unwrap();
    assert_eq!(f.core.ssh_tunnel_count(), 1);
    f.core
        .query(&b, "SELECT 1", vec![])
        .await
        .expect("the other connection keeps its tunnel");
    f.core.disconnect(&b).await.unwrap();
    assert_eq!(f.core.ssh_tunnel_count(), 0);
}
