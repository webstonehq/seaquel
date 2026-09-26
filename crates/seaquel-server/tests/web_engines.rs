//! Decision 11b: the web server never opens SQLite or DuckDB.
//!
//! Their "connection string" is a path on the server, so a signed-in user
//! could open `auth.db` or another user's `meta.db`, and DuckDB's `read_*`,
//! `sqlite_scan` and `COPY TO` reach any file the server can. The server's
//! Core registers only `WEB_ENGINES`, so both routes that take a driver
//! refuse them. These tests use the state the binary serves
//! (`AppState::default()`), and they hold in a workspace test build too,
//! where Cargo's feature unification compiles both engines into Core
//! (this crate's own dev-dependencies pull in the SQLite engine).

mod common;

use axum::http::StatusCode;
use common::post_json;
use seaquel_server::{build_router, AppState, WEB_ENGINES};
use serde_json::{json, Value};

/// Bodies a user could send for each refused driver: the server's own
/// databases, an in-memory DuckDB (which can still read and write files) and
/// a DuckDB file.
fn refused_configs(data_dir: &std::path::Path) -> Vec<Value> {
    let auth_db = data_dir.join("auth.db");
    let meta_db = data_dir.join("users").join("someone-else").join("meta.db");
    vec![
        json!({ "driver": "sqlite", "connection_string": format!("sqlite:{}", auth_db.display()) }),
        json!({ "driver": "sqlite", "connection_string": format!("sqlite:{}", meta_db.display()) }),
        json!({ "driver": "sqlite", "connection_string": "sqlite::memory:" }),
        json!({
            "driver": "sqlite",
            "connection_string": format!("sqlite:{}", data_dir.join("new.db").display()),
            "create_if_missing": true,
        }),
        json!({ "driver": "duckdb" }),
        json!({ "driver": "duckdb", "path": ":memory:" }),
        json!({ "driver": "duckdb", "path": data_dir.join("x.duckdb").display().to_string() }),
    ]
}

fn assert_refused(route: &str, config: &Value, status: StatusCode, body: &Value) {
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{route} {config}: body={body}"
    );
    assert_eq!(body["code"], "ENGINE_NOT_AVAILABLE", "{route} {config}");
    let driver = config["driver"].as_str().unwrap();
    assert_eq!(
        body["message"],
        format!("Database engine \"{driver}\" is not available in this build"),
        "{route} {config}"
    );
}

#[tokio::test]
async fn connect_and_test_refuse_sqlite_and_duckdb() {
    let dir = tempfile::tempdir().unwrap();
    // A real SQLite file where auth.db would be, so a refusal isn't just a
    // missing file.
    std::fs::write(dir.path().join("auth.db"), b"").unwrap();
    let app = build_router(AppState::default());

    for config in refused_configs(dir.path()) {
        for route in ["/api/db/connect", "/api/db/test"] {
            let (status, body) = post_json(app.clone(), route, config.clone()).await;
            assert_refused(route, &config, status, &body);
        }
    }
    // Nothing was created: no DuckDB or SQLite file appeared.
    assert!(!dir.path().join("new.db").exists());
    assert!(!dir.path().join("x.duckdb").exists());
}

#[tokio::test]
async fn the_server_core_holds_only_the_web_engines() {
    let state = AppState::default();
    let ids = state.core.engine_ids();
    assert!(!ids.contains(&"sqlite"), "{ids:?}");
    assert!(!ids.contains(&"duckdb"), "{ids:?}");
    for id in &ids {
        assert!(WEB_ENGINES.contains(id), "{id} isn't a web engine");
    }
    // The server's Cargo features compile all three in, so they're offered.
    assert_eq!(ids, ["mssql", "mysql", "postgres"]);
}

/// Connections reach databases over the network only: no TLS files or
/// Unix sockets on the server (`web_config`). Checked before Core, so no
/// driver ever sees them.
#[tokio::test]
async fn server_files_and_sockets_are_refused_in_connection_strings() {
    let app = build_router(AppState::default());
    for (driver, conn_str) in [
        (
            "postgres",
            "postgres://u@db.example.com/app?sslkey=/data/auth.db",
        ),
        (
            "postgres",
            "postgres://u@db.example.com/app?sslrootcert=/etc/passwd",
        ),
        (
            "postgres",
            "postgres://u@db.example.com/app?host=/var/run/postgresql",
        ),
        ("postgres", "postgres:///app"),
        (
            "mysql",
            "mysql://root@db.example.com/app?ssl-ca=/data/auth.db",
        ),
        (
            "mysql",
            "mysql://root@db.example.com/app?socket=/run/mysqld/mysqld.sock",
        ),
    ] {
        let config = json!({ "driver": driver, "connection_string": conn_str });
        for route in ["/api/db/connect", "/api/db/test"] {
            let (status, body) = post_json(app.clone(), route, config.clone()).await;
            assert_eq!(
                status,
                StatusCode::BAD_REQUEST,
                "{route} {conn_str}: {body}"
            );
            assert_eq!(
                body["code"], "CONNECTION_OPTION_NOT_ALLOWED",
                "{route} {conn_str}"
            );
        }
    }
}
