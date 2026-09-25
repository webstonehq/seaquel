//! Pins the JSON shape of every wire type. The TypeScript frontend (wire.ts,
//! both providers) depends on these exact shapes; a failure here means the
//! frontend breaks too.

use seaquel_types::{
    BatchStatement, ConnectConfig, ConnectResult, DbError, DriverType, ExecuteResult,
    QueryResult, StreamBatch, StreamEvent,
};
use serde_json::{from_value, json, to_value};

#[test]
fn query_result_is_columnar() {
    let r = QueryResult {
        columns: vec!["a".into(), "b".into()],
        rows: vec![vec![json!(1), json!("x")]],
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "columns": ["a", "b"], "rows": [[1, "x"]] })
    );
}

#[test]
fn stream_batch_keeps_snake_case_fields() {
    let b = StreamBatch {
        columns: None,
        rows: vec![],
        is_final: true,
    };
    assert_eq!(
        to_value(&b).unwrap(),
        json!({ "columns": null, "rows": [], "is_final": true })
    );
}

#[test]
fn execute_result_shape() {
    let r = ExecuteResult {
        rows_affected: 3,
        last_insert_id: None,
    };
    assert_eq!(
        to_value(&r).unwrap(),
        json!({ "rows_affected": 3, "last_insert_id": null })
    );
}

#[test]
fn connect_result_shape() {
    let r = ConnectResult {
        connection_id: "sqlite-1".into(),
    };
    assert_eq!(to_value(&r).unwrap(), json!({ "connection_id": "sqlite-1" }));
}

#[test]
fn db_error_shape_and_display() {
    let e = DbError::query_error("boom");
    assert_eq!(
        to_value(&e).unwrap(),
        json!({ "message": "Query failed: boom", "code": "QUERY_ERROR" })
    );
    assert_eq!(e.to_string(), "QUERY_ERROR: Query failed: boom");
}

#[test]
fn connect_config_accepts_every_field() {
    let c: ConnectConfig = from_value(json!({
        "driver": "mssql",
        "connection_string": "cs",
        "host": "db.local",
        "port": 1433,
        "database": "master",
        "username": "sa",
        "password": "pw",
        "encrypt": true,
        "trust_cert": false,
        "path": "/tmp/x.duckdb",
        "create_if_missing": true
    }))
    .unwrap();
    assert_eq!(c.driver, DriverType::Mssql);
    assert_eq!(c.connection_string.as_deref(), Some("cs"));
    assert_eq!(c.host.as_deref(), Some("db.local"));
    assert_eq!(c.port, Some(1433));
    assert_eq!(c.database.as_deref(), Some("master"));
    assert_eq!(c.username.as_deref(), Some("sa"));
    assert_eq!(c.password.as_deref(), Some("pw"));
    assert_eq!(c.encrypt, Some(true));
    assert_eq!(c.trust_cert, Some(false));
    assert_eq!(c.path.as_deref(), Some("/tmp/x.duckdb"));
    assert_eq!(c.create_if_missing, Some(true));
}

#[test]
fn connect_config_optional_fields_default_to_none() {
    let c: ConnectConfig = from_value(json!({ "driver": "duckdb" })).unwrap();
    assert_eq!(c.driver, DriverType::Duckdb);
    assert!(c.connection_string.is_none());
    assert!(c.path.is_none());
    assert!(c.create_if_missing.is_none());
}

#[test]
fn driver_type_is_lowercase_on_the_wire() {
    for (wire, expected) in [
        ("postgres", DriverType::Postgres),
        ("mysql", DriverType::Mysql),
        ("sqlite", DriverType::Sqlite),
        ("mssql", DriverType::Mssql),
        ("duckdb", DriverType::Duckdb),
    ] {
        assert_eq!(from_value::<DriverType>(json!(wire)).unwrap(), expected);
        assert_eq!(expected.as_str(), wire);
    }
    assert!(from_value::<DriverType>(json!("Postgres")).is_err());
    assert!(from_value::<DriverType>(json!("mariadb")).is_err());
}

#[test]
fn batch_statement_params_default_to_empty() {
    let s: BatchStatement = from_value(json!({ "sql": "DELETE FROM t" })).unwrap();
    assert_eq!(s.sql, "DELETE FROM t");
    assert!(s.params.is_empty());
}

#[test]
fn stream_event_batch_is_flattened() {
    let ev = StreamEvent::Batch(StreamBatch {
        columns: Some(vec!["n".into()]),
        rows: vec![vec![json!(1)]],
        is_final: false,
    });
    assert_eq!(
        to_value(&ev).unwrap(),
        json!({ "type": "batch", "columns": ["n"], "rows": [[1]], "is_final": false })
    );
}

#[test]
fn stream_event_done_and_error() {
    assert_eq!(to_value(StreamEvent::Done).unwrap(), json!({ "type": "done" }));
    assert_eq!(
        to_value(StreamEvent::from(DbError::connection_not_found("x"))).unwrap(),
        json!({
            "type": "error",
            "message": "Connection not found: x",
            "code": "CONNECTION_NOT_FOUND"
        })
    );
}

#[test]
fn engine_not_available_error() {
    let e = DbError::engine_not_available("mssql");
    assert_eq!(e.code, "ENGINE_NOT_AVAILABLE");
    assert!(e.message.contains("\"mssql\""), "{}", e.message);
}
