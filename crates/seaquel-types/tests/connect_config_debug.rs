//! `ConnectConfig`'s `Debug` never shows a password, in `{:?}` or `{:#?}`.

use seaquel_types::ConnectConfig;
use serde_json::json;

fn config(v: serde_json::Value) -> ConnectConfig {
    serde_json::from_value(v).unwrap()
}

fn both(c: &ConnectConfig) -> String {
    format!("{c:?}\n{c:#?}")
}

#[test]
fn the_password_field_is_redacted() {
    let c = config(json!({
        "driver": "mssql", "host": "sql.example.com", "port": 1433,
        "username": "sa", "password": "hunter2-mssql", "encrypt": true, "trust_cert": false,
    }));
    let debug = both(&c);
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("<redacted>"), "{debug}");
    assert!(
        debug.contains("sql.example.com") && debug.contains("1433"),
        "{debug}"
    );
}

#[test]
fn a_url_loses_its_password_and_keeps_the_rest() {
    let c = config(json!({
        "driver": "postgres",
        "connection_string": "postgres://alice:hunter2%40x@db.example.com:5432/app?sslmode=require",
    }));
    let debug = both(&c);
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(
        debug.contains("postgres://alice@db.example.com:5432/app?sslmode=require"),
        "{debug}"
    );

    // No password: unchanged.
    let c = config(json!({
        "driver": "sqlite", "connection_string": "sqlite:///Users/me/app.db",
    }));
    assert!(format!("{c:?}").contains("sqlite:///Users/me/app.db"));

    // An empty username with a password.
    let c = config(json!({
        "driver": "mysql", "connection_string": "mysql://:hunter2@h/db",
    }));
    let debug = both(&c);
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(debug.contains("mysql://@h/db"), "{debug}");
}

#[test]
fn a_string_it_cant_read_safely_is_redacted_whole() {
    for s in [
        // key=value
        "Server=tcp:h,1433;User Id=sa;Password=hunter2;",
        // a raw `/` in the password moves the `@` out of the authority
        "postgres://alice:hun/ter2@h/db",
        // TablePlus: the db password sits in the path
        "postgres+ssh://deploy@bastion/alice:hunter2@127.0.0.1:5432/app",
        // a password query parameter
        "postgres://h/db?user=a&password=hunter2",
        "mysql://h/db?PWD=hunter2",
        // not a scheme
        "1x://a:hunter2@h/db",
    ] {
        let c = config(json!({ "driver": "postgres", "connection_string": s }));
        let debug = both(&c);
        assert!(!debug.contains("hunter2"), "{s}: {debug}");
        assert!(!debug.contains("ter2"), "{s}: {debug}");
        assert!(debug.contains("<redacted>"), "{s}: {debug}");
    }
}

#[test]
fn debug_shows_the_duckdb_restriction() {
    let c = config(json!({ "driver": "duckdb", "path": "/tmp/x.duckdb", "restricted": true }));
    assert!(format!("{c:?}").contains("restricted: Some(true)"));
}

#[test]
fn duckdb_config_shows_its_keys_only() {
    let c = config(json!({
        "driver": "duckdb", "path": ":memory:",
        "duckdb_config": { "access_mode": "read_only", "s3_secret_access_key": "hunter2-s3" },
        "tls_server_name": "sql.internal",
    }));
    let debug = both(&c);
    assert!(!debug.contains("hunter2"), "{debug}");
    assert!(!debug.contains("read_only"), "{debug}");
    assert!(
        debug.contains("access_mode") && debug.contains("s3_secret_access_key"),
        "{debug}"
    );
    assert!(debug.contains("sql.internal"), "{debug}");
}
