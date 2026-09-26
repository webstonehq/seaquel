//! Live tunnel tests against the SSH container in
//! `e2e/test-databases/docker-compose.yml`, forwarding to its Postgres.
//!
//! Set `SEAQUEL_TEST_SSH` to run them, e.g.
//! `{"host":"127.0.0.1","port":2222,"remote_host":"postgres","remote_port":5432}`
//! (`remote_host` as the SSH server sees Postgres). The users, password and
//! keys are the container's fixtures. Every test uses its own temp
//! known_hosts file; none reads or writes `~/.ssh`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use seaquel_ssh::{open, TunnelOptions};
use seaquel_types::ssh::TunnelConfig;
use serde::Deserialize;
use sqlx::{Connection, PgConnection};

const USER: &str = "seaquel";
const PASSWORD: &str = "seaquel-test-password";
const PASSPHRASE: &str = "seaquel-test-passphrase";

#[derive(Deserialize)]
struct Env {
    host: String,
    port: u16,
    remote_host: String,
    remote_port: u16,
}

fn env() -> Option<Env> {
    match std::env::var("SEAQUEL_TEST_SSH") {
        Ok(raw) => Some(
            serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("SEAQUEL_TEST_SSH is not valid JSON: {e}")),
        ),
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_SSH is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => {
            eprintln!("skipping: SEAQUEL_TEST_SSH is not set");
            None
        }
    }
}

fn fixture(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../e2e/test-databases/ssh")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn password_config(env: &Env) -> TunnelConfig {
    TunnelConfig {
        ssh_host: env.host.clone(),
        ssh_port: env.port,
        ssh_username: USER.into(),
        auth_method: "password".into(),
        password: Some(PASSWORD.into()),
        key_path: None,
        key_passphrase: None,
        remote_host: env.remote_host.clone(),
        remote_port: env.remote_port,
        trust_host_key: None,
    }
}

fn key_config(env: &Env, key: &str, passphrase: Option<&str>) -> TunnelConfig {
    TunnelConfig {
        auth_method: "key".into(),
        password: None,
        key_path: Some(fixture(key)),
        key_passphrase: passphrase.map(Into::into),
        ..password_config(env)
    }
}

/// A temp dir holding the known_hosts path (the file itself doesn't exist
/// until a key is learned).
struct KnownHosts {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl KnownHosts {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("known_hosts");
        Self { _dir: dir, path }
    }

    fn options(&self) -> TunnelOptions {
        TunnelOptions::default().with_known_hosts(&self.path)
    }

    /// A known_hosts file that already trusts the server.
    async fn trusted(env: &Env) -> Self {
        let kh = Self::new();
        let config = TunnelConfig {
            trust_host_key: Some(server_fingerprint(env, &kh).await),
            ..password_config(env)
        };
        open(&config, &kh.options())
            .await
            .expect("trust the host")
            .close()
            .await;
        kh
    }
}

/// The fingerprint in an error's `Fingerprint: SHA256:…` line.
fn fingerprint_in(message: &str) -> String {
    let at = message
        .find("SHA256:")
        .expect("a fingerprint in the message");
    message[at..].split_whitespace().next().unwrap().to_string()
}

/// The server's fingerprint, as the UNKNOWN_HOST_KEY prompt shows it.
async fn server_fingerprint(env: &Env, kh: &KnownHosts) -> String {
    let err = open(&password_config(env), &kh.options())
        .await
        .expect_err("an unknown host must be refused");
    assert_eq!(err.code, "UNKNOWN_HOST_KEY", "{err}");
    fingerprint_in(&err.message)
}

async fn pg(local_port: u16) -> PgConnection {
    let url = format!("postgres://postgres@127.0.0.1:{local_port}/postgres");
    PgConnection::connect(&url)
        .await
        .expect("connect through the tunnel")
}

async fn select_one(conn: &mut PgConnection) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar("SELECT 1").fetch_one(conn).await
}

async fn assert_select_one_through(config: &TunnelConfig, kh: &KnownHosts) {
    let tunnel = open(config, &kh.options()).await.expect("open the tunnel");
    let mut conn = pg(tunnel.local_port()).await;
    assert_eq!(select_one(&mut conn).await.unwrap(), 1);
    conn.close().await.unwrap();
    tunnel.close().await;
}

#[tokio::test]
async fn password_auth_reaches_postgres() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;
    assert_select_one_through(&password_config(&env), &kh).await;
}

#[tokio::test]
async fn key_auth_reaches_postgres() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;
    assert_select_one_through(&key_config(&env, "id_ed25519", None), &kh).await;
}

#[tokio::test]
async fn key_auth_with_a_passphrase_reaches_postgres() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;
    let config = key_config(&env, "id_ed25519_passphrase", Some(PASSPHRASE));
    assert_select_one_through(&config, &kh).await;
}

#[tokio::test]
async fn a_wrong_passphrase_fails_to_load_the_key() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;
    let config = key_config(&env, "id_ed25519_passphrase", Some("not-the-passphrase"));
    let err = open(&config, &kh.options()).await.expect_err("must fail");
    assert_eq!(err.code, "KEY_LOAD_ERROR", "{err}");
    assert!(!err.message.contains("not-the-passphrase"), "{err}");
}

#[tokio::test]
async fn a_missing_key_file_is_key_not_found() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;
    let config = key_config(&env, "no_such_key", None);
    let err = open(&config, &kh.options()).await.expect_err("must fail");
    assert_eq!(err.code, "KEY_NOT_FOUND", "{err}");
}

#[tokio::test]
async fn a_wrong_password_is_auth_failed() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;
    let config = TunnelConfig {
        password: Some("wrong-password-123".into()),
        ..password_config(&env)
    };
    let err = open(&config, &kh.options()).await.expect_err("must fail");
    assert_eq!(err.code, "AUTH_FAILED", "{err}");
    assert!(!err.message.contains("wrong-password-123"), "{err}");
}

#[tokio::test]
async fn an_unknown_host_asks_then_is_trusted_then_just_connects() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::new();

    let err = open(&password_config(&env), &kh.options())
        .await
        .expect_err("an unknown host must be refused");
    assert_eq!(err.code, "UNKNOWN_HOST_KEY", "{err}");
    assert!(err.message.contains("Fingerprint: SHA256:"), "{err}");
    assert!(
        !kh.path.exists(),
        "nothing is recorded before the user trusts it"
    );

    let trusted = TunnelConfig {
        trust_host_key: Some(fingerprint_in(&err.message)),
        ..password_config(&env)
    };
    open(&trusted, &kh.options()).await.unwrap().close().await;
    let recorded = std::fs::read_to_string(&kh.path).unwrap();
    assert!(
        recorded.contains(&format!("[{}]:{}", env.host, env.port)),
        "{recorded}"
    );

    // Recorded now: no prompt, and it works.
    assert_select_one_through(&password_config(&env), &kh).await;
}

#[tokio::test]
async fn a_changed_host_key_is_a_mismatch_even_when_trusting() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;

    // Swap the recorded key for another ed25519 key (the fixture client
    // key), as if the server's key had changed.
    let other = std::fs::read_to_string(fixture("id_ed25519.pub")).unwrap();
    let other_key = other.split_whitespace().nth(1).unwrap();
    std::fs::write(
        &kh.path,
        format!("[{}]:{} ssh-ed25519 {other_key}\n", env.host, env.port),
    )
    .unwrap();

    // Trusting the server's real fingerprint doesn't override a recorded
    // key either.
    let real = server_fingerprint(&env, &KnownHosts::new()).await;
    for trust_host_key in [None, Some(real)] {
        let config = TunnelConfig {
            trust_host_key,
            ..password_config(&env)
        };
        let err = open(&config, &kh.options()).await.expect_err("must fail");
        assert_eq!(err.code, "HOST_KEY_MISMATCH", "{err}");
        assert!(err.message.contains("Fingerprint: SHA256:"), "{err}");
    }
}

/// The trust retry records a key only if it is the one the user approved:
/// a different key (a second connection intercepted) records nothing.
#[tokio::test]
async fn trusting_a_wrong_fingerprint_records_nothing_and_fails() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::new();
    let real = server_fingerprint(&env, &kh).await;

    let config = TunnelConfig {
        trust_host_key: Some("SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into()),
        ..password_config(&env)
    };
    let err = open(&config, &kh.options()).await.expect_err("must fail");
    assert_eq!(err.code, "UNKNOWN_HOST_KEY", "{err}");
    assert_eq!(
        fingerprint_in(&err.message),
        real,
        "the presented key's own fingerprint"
    );
    assert!(!kh.path.exists(), "nothing may be recorded");

    // And the server is still unknown afterwards.
    assert_eq!(server_fingerprint(&env, &kh).await, real);
}

#[tokio::test]
async fn trusting_the_right_fingerprint_records_it() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::new();
    let real = server_fingerprint(&env, &kh).await;
    let config = TunnelConfig {
        trust_host_key: Some(real),
        ..password_config(&env)
    };
    open(&config, &kh.options()).await.unwrap().close().await;
    let recorded = std::fs::read_to_string(&kh.path).unwrap();
    // russh starts a new file with an empty line.
    let lines: Vec<&str> = recorded.lines().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.len(), 1, "{recorded}");
    assert!(
        lines[0].starts_with(&format!("[{}]:{} ", env.host, env.port)),
        "{recorded}"
    );
    assert_select_one_through(&password_config(&env), &kh).await;
}

/// Decision 8: closing a tunnel ends the forwards already running through
/// it, not just the listener.
#[tokio::test]
async fn close_ends_a_live_forward() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;
    let tunnel = open(&password_config(&env), &kh.options()).await.unwrap();
    let port = tunnel.local_port();
    let mut conn = pg(port).await;
    assert_eq!(select_one(&mut conn).await.unwrap(), 1);

    tunnel.close().await;

    let next = tokio::time::timeout(Duration::from_secs(10), select_one(&mut conn)).await;
    assert!(
        matches!(next, Ok(Err(_))),
        "a query on a closed tunnel must fail, got {next:?}"
    );
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err(),
        "the local port must be closed"
    );
}

/// Dropping a tunnel without `close` (Core dropping its tunnels) ends its
/// forwards too.
#[tokio::test]
async fn drop_ends_a_live_forward() {
    let Some(env) = env() else { return };
    let kh = KnownHosts::trusted(&env).await;
    let tunnel = open(&password_config(&env), &kh.options()).await.unwrap();
    let mut conn = pg(tunnel.local_port()).await;
    assert_eq!(select_one(&mut conn).await.unwrap(), 1);

    drop(tunnel);

    let next = tokio::time::timeout(Duration::from_secs(10), select_one(&mut conn)).await;
    assert!(
        matches!(next, Ok(Err(_))),
        "a query on a dropped tunnel must fail, got {next:?}"
    );
}

/// Needs no container.
#[tokio::test]
async fn an_unreachable_server_is_a_connection_error() {
    let kh = KnownHosts::new();
    // Port 1 on loopback: refused at once.
    let env = Env {
        host: "127.0.0.1".into(),
        port: 1,
        remote_host: "db".into(),
        remote_port: 5432,
    };
    let config = password_config(&env);
    let err = open(&config, &kh.options()).await.expect_err("must fail");
    assert_eq!(err.code, "CONNECTION_ERROR", "{err}");
}
