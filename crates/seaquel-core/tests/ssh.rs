//! Core's SSH tunnels: open and close by id, and every tunnel ends when Core
//! is dropped. The live cases use the SSH container from
//! `e2e/test-databases/docker-compose.yml` (set `SEAQUEL_TEST_SSH`, as for
//! `crates/seaquel-ssh/tests/tunnel.rs`) and a temp known_hosts file.
#![cfg(all(feature = "ssh", feature = "engine-postgres"))]

use std::path::Path;
use std::time::Duration;

use seaquel_core::ssh::{TunnelConfig, TUNNEL_NOT_FOUND};
use seaquel_core::Core;
use seaquel_engine::ConnectConfig;
use serde_json::{json, Value};

struct Env {
    host: String,
    port: u16,
    remote_host: String,
    remote_port: u16,
}

impl Env {
    fn parse(raw: &str) -> Self {
        let v: Value = serde_json::from_str(raw).expect("SEAQUEL_TEST_SSH is not valid JSON");
        let text = |k: &str| v[k].as_str().expect(k).to_string();
        let port = |k: &str| u16::try_from(v[k].as_u64().expect(k)).expect(k);
        Self {
            host: text("host"),
            port: port("port"),
            remote_host: text("remote_host"),
            remote_port: port("remote_port"),
        }
    }
}

fn env() -> Option<Env> {
    match std::env::var("SEAQUEL_TEST_SSH") {
        Ok(raw) => Some(Env::parse(&raw)),
        Err(_) if std::env::var_os("SEAQUEL_TEST_REQUIRE_ENGINES").is_some() => {
            panic!("SEAQUEL_TEST_SSH is not set, and SEAQUEL_TEST_REQUIRE_ENGINES requires it")
        }
        Err(_) => {
            eprintln!("skipping: SEAQUEL_TEST_SSH is not set");
            None
        }
    }
}

fn config(env: &Env) -> TunnelConfig {
    TunnelConfig {
        ssh_host: env.host.clone(),
        ssh_port: env.port,
        ssh_username: "seaquel".into(),
        auth_method: "password".into(),
        password: Some("seaquel-test-password".into()),
        key_path: None,
        key_passphrase: None,
        remote_host: env.remote_host.clone(),
        remote_port: env.remote_port,
        trust_host_key: None,
    }
}

/// `config` trusting the server's key: the fingerprint from the
/// UNKNOWN_HOST_KEY answer, as the prompt shows it. The known_hosts file is
/// a fresh temp file per test.
async fn trusted_config(core: &Core, env: &Env) -> TunnelConfig {
    let err = core.ssh_open(&config(env)).await.unwrap_err();
    assert_eq!(err.code, "UNKNOWN_HOST_KEY", "{err:?}");
    let at = err.message.find("SHA256:").expect("a fingerprint");
    let fingerprint = err.message[at..].split_whitespace().next().unwrap();
    TunnelConfig {
        trust_host_key: Some(fingerprint.to_string()),
        ..config(env)
    }
}

/// A query after its tunnel is gone must fail. The Postgres pool keeps
/// retrying the closed port until its 30 s acquire timeout, so a query still
/// waiting after 3 s counts as failed too.
async fn query_fails(core: &Core, connection_id: &str) -> bool {
    matches!(
        tokio::time::timeout(
            Duration::from_secs(3),
            core.query(connection_id, "SELECT 1", vec![])
        )
        .await,
        Err(_) | Ok(Err(_))
    )
}

fn core(known_hosts: &Path) -> Core {
    seaquel_core::with_default_plugins()
        .ssh_known_hosts(known_hosts)
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build()
}

fn postgres(local_port: u16) -> ConnectConfig {
    serde_json::from_value(json!({
        "driver": "postgres",
        "connection_string": format!("postgres://postgres@127.0.0.1:{local_port}/postgres"),
    }))
    .unwrap()
}

#[tokio::test]
async fn closing_an_unknown_tunnel_is_tunnel_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let core = core(&dir.path().join("known_hosts"));
    let err = core.ssh_close("tunnel-404").await.unwrap_err();
    assert_eq!(err.code, TUNNEL_NOT_FOUND);
    assert_eq!(err.message, "Tunnel not found: tunnel-404");
}

#[tokio::test]
async fn ssh_close_ends_queries_through_the_tunnel() {
    let Some(env) = env() else { return };
    let dir = tempfile::tempdir().unwrap();
    let core = core(&dir.path().join("known_hosts"));

    let config = trusted_config(&core, &env).await;
    let tunnel = core.ssh_open(&config).await.expect("open");
    assert!(tunnel.tunnel_id.starts_with("tunnel-"), "{tunnel:?}");
    let db = core.connect(&postgres(tunnel.local_port)).await.unwrap();
    core.query(&db.connection_id, "SELECT 1", vec![])
        .await
        .expect("query through the tunnel");

    core.ssh_close(&tunnel.tunnel_id).await.unwrap();

    assert!(
        query_fails(&core, &db.connection_id).await,
        "a query through a closed tunnel must fail"
    );
    let again = core.ssh_close(&tunnel.tunnel_id).await.unwrap_err();
    assert_eq!(again.code, TUNNEL_NOT_FOUND);
    let _ = core.disconnect(&db.connection_id).await;
}

#[tokio::test]
async fn dropping_core_closes_its_tunnels() {
    let Some(env) = env() else { return };
    let dir = tempfile::tempdir().unwrap();
    let tunnel_core = core(&dir.path().join("known_hosts"));
    let config = trusted_config(&tunnel_core, &env).await;
    let a = tunnel_core.ssh_open(&config).await.unwrap();
    let b = tunnel_core.ssh_open(&config).await.unwrap();
    assert_ne!(a.tunnel_id, b.tunnel_id);

    // The database connections live on another Core, so only the tunnels go.
    let db_core = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .build();
    let db = db_core.connect(&postgres(a.local_port)).await.unwrap();
    db_core
        .query(&db.connection_id, "SELECT 1", vec![])
        .await
        .unwrap();

    drop(tunnel_core);
    // Dropping aborts the forwarding tasks; let the runtime run them down.
    tokio::task::yield_now().await;

    assert!(
        query_fails(&db_core, &db.connection_id).await,
        "a query through a dropped Core's tunnel must fail"
    );
    for port in [a.local_port, b.local_port] {
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err(),
            "port {port} must be closed"
        );
    }
    let _ = db_core.disconnect(&db.connection_id).await;
}
