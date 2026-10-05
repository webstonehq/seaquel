//! `ConnectConfig::duckdb_config` (phase 5a): options
//! parsed out of a `duckdb://path?key=value` string open the database with
//! them. An unknown option fails the connect with DuckDB's message, and a
//! `restricted` instance takes only an allowlist.

#[path = "common/engine.rs"]
mod engine_switch;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, Driver};
use seaquel_engine_testkit::scratch_name;
use serde_json::json;

fn config(v: serde_json::Value) -> ConnectConfig {
    serde_json::from_value(v).unwrap()
}

async fn open(config: &ConnectConfig) -> Result<Arc<dyn Driver>, seaquel_engine::DbError> {
    engine_switch::engine().open(config).await
}

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(scratch_name("seaquel-duckdb-config-"));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn access_mode_read_only_opens_the_file_read_only() {
    let dir = TempDir::new();
    let path = dir.0.join("app.duckdb");
    let path = path.to_str().unwrap();
    let setup = open(&config(
        json!({ "driver": "duckdb", "path": path, "create_if_missing": true }),
    ))
    .await
    .unwrap();
    setup
        .execute("CREATE TABLE t AS SELECT 42 AS a", vec![])
        .await
        .unwrap();
    setup.close().await.unwrap();

    let d = open(&config(json!({
        "driver": "duckdb", "path": path,
        "duckdb_config": { "access_mode": "read_only" },
    })))
    .await
    .unwrap();
    let rows = d.query("SELECT a FROM t", vec![]).await.unwrap();
    assert_eq!(rows.rows.len(), 1);
    let err = d
        .execute("INSERT INTO t VALUES (1)", vec![])
        .await
        .unwrap_err();
    assert!(err.message.to_lowercase().contains("read-only"), "{err:?}");
    d.close().await.unwrap();
}

#[tokio::test]
async fn an_unknown_option_fails_the_connect() {
    let err = open(&config(json!({
        "driver": "duckdb", "path": ":memory:",
        "duckdb_config": { "no_such_option_xyz": "1" },
    })))
    .await
    .err()
    .unwrap();
    assert!(err.message.contains("no_such_option_xyz"), "{err:?}");
}

#[tokio::test]
async fn threads_applies_in_memory() {
    let d = open(&config(json!({
        "driver": "duckdb", "path": ":memory:",
        "duckdb_config": { "threads": "3" },
    })))
    .await
    .unwrap();
    let rows = d
        .query("SELECT current_setting('threads')::INTEGER AS n", vec![])
        .await
        .unwrap();
    assert_eq!(rows.rows[0][0], seaquel_engine::Value::Int(3));
}

/// A restricted instance takes only an allowlist of options: anything that
/// could reopen file or extension access is refused, naming the key.
#[tokio::test]
async fn restricted_refuses_options_outside_its_allowlist() {
    let dir = TempDir::new();
    for key in [
        "allowed_directories",
        "allowed_paths",
        "ALLOWED_DIRECTORIES",
        "enable_external_access",
        "lock_configuration",
        "extension_directory",
        "allow_unsigned_extensions",
        "allow_community_extensions",
        "temp_directory",
        "secret_directory",
    ] {
        let err = open(&config(json!({
            "driver": "duckdb", "path": ":memory:", "restricted": true,
            "duckdb_config": { key: dir.0.to_str().unwrap() },
        })))
        .await
        .err()
        .unwrap_or_else(|| panic!("{key} was accepted"));
        assert_eq!(err.code, "INVALID_CONNECTION", "{key}: {err:?}");
        assert!(err.message.contains(key), "{err:?}");
    }
}

/// An allowed option still applies when restricted, and the lock-down holds.
#[tokio::test]
async fn restricted_takes_allowed_options() {
    let dir = TempDir::new();
    let secret = dir.0.join("secret.txt");
    std::fs::write(&secret, "top secret").unwrap();
    let d = open(&config(json!({
        "driver": "duckdb", "path": ":memory:", "restricted": true,
        "duckdb_config": { "threads": "2", "memory_limit": "512MB" },
    })))
    .await
    .unwrap();
    let rows = d
        .query("SELECT current_setting('threads')::INTEGER AS n", vec![])
        .await
        .unwrap();
    assert_eq!(rows.rows[0][0], seaquel_engine::Value::Int(2));
    let sql = format!("SELECT content FROM read_text('{}')", secret.display());
    let err = d.query(&sql, vec![]).await.unwrap_err();
    assert!(err.message.contains("disabled by configuration"), "{err:?}");
    assert!(d
        .execute("SET autoload_known_extensions = true", vec![])
        .await
        .is_err());
}

/// `streaming_buffer_size` (how far a streamed query runs ahead of its
/// reader, 1 MB by default) is a session setting, not an open option: DuckDB
/// refuses it in the open config, so a connection string can't set it, and a
/// restricted instance refuses it earlier, off its allowlist. A session
/// `SET` changes it; on a restricted instance only the editor's own path
/// could send one (the read-only path refuses `SET`), and the MCP server's
/// tools use the read-only path.
#[tokio::test]
async fn streaming_buffer_size_is_a_session_setting() {
    for size in ["64KB", "64GB"] {
        let err = open(&config(json!({
            "driver": "duckdb", "path": ":memory:", "restricted": true,
            "duckdb_config": { "streaming_buffer_size": size },
        })))
        .await
        .err()
        .unwrap_or_else(|| panic!("{size} was accepted"));
        assert_eq!(err.code, "INVALID_CONNECTION", "{size}: {err:?}");
        assert!(err.message.contains("streaming_buffer_size"), "{err:?}");

        let err = open(&config(json!({
            "driver": "duckdb", "path": ":memory:",
            "duckdb_config": { "streaming_buffer_size": size },
        })))
        .await
        .err()
        .unwrap_or_else(|| panic!("{size} was accepted in the open config"));
        assert_eq!(err.code, "CONNECTION_ERROR", "{size}: {err:?}");
        assert!(err.message.contains("streaming_buffer_size"), "{err:?}");
    }
    let d = open(&config(json!({ "driver": "duckdb", "path": ":memory:" })))
        .await
        .unwrap();
    let setting = || {
        d.query(
            "SELECT current_setting('streaming_buffer_size') AS s",
            vec![],
        )
    };
    assert_eq!(
        setting().await.unwrap().rows[0][0],
        seaquel_engine::Value::Text("976.5 KiB".into())
    );
    d.execute("SET streaming_buffer_size = '64KB'", vec![])
        .await
        .unwrap();
    assert_eq!(
        setting().await.unwrap().rows[0][0],
        seaquel_engine::Value::Text("62.5 KiB".into())
    );
}
