//! `run_typed_cells` against SQLite, which needs no server: passing cases,
//! the failure report, and teardown after a failure.

use futures::FutureExt;
use seaquel_engine::{ConnectConfig, Value};
use seaquel_engine_testkit::{run_typed_cells, TypedCellCase};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

fn sqlite_file() -> (PathBuf, ConnectConfig) {
    let path = std::env::temp_dir().join(format!("seaquel-typed-{}.sqlite", uuid::Uuid::new_v4()));
    let config = serde_json::from_value(serde_json::json!({
        "driver": "sqlite",
        "connection_string": format!("sqlite:{}", path.display()),
        "create_if_missing": true
    }))
    .unwrap();
    (path, config)
}

fn table_case(table: &str, expected: Value) -> TypedCellCase {
    TypedCellCase {
        name: format!("{table} column"),
        setup: vec![
            format!("CREATE TABLE {table} (v INTEGER)"),
            format!("INSERT INTO {table} (v) VALUES (42)"),
        ],
        teardown: vec![format!("DROP TABLE {table}")],
        select: format!("SELECT v FROM {table}"),
        literal: None,
        expected,
        bind_back: Some(format!("SELECT ? = (SELECT v FROM {table})")),
    }
}

fn remove_sqlite(path: &std::path::Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(p);
    }
}

async fn table_names(config: &ConnectConfig) -> Vec<String> {
    let driver = seaquel_engine_sqlite::engine()
        .open(config)
        .await
        .expect("open");
    let r = driver
        .query(
            "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name",
            vec![],
        )
        .await
        .expect("sqlite_master");
    driver.close().await.expect("close");
    r.rows
        .into_iter()
        .map(|r| r[0].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn passing_cases() {
    let (path, config) = sqlite_file();
    let cases = vec![
        TypedCellCase::literal("7", Value::Int(7)).bind_back_eq("?"),
        TypedCellCase::literal("'abc'", Value::from("abc")).bind_back_eq("?"),
        TypedCellCase::literal("1.5", Value::Float(1.5)).bind_back_eq("?"),
        // No bind-back: NULL = NULL is NULL.
        TypedCellCase::literal("NULL", Value::Null),
        // `bind_back_eq` compares with the literal, not the name, whichever
        // comes first.
        TypedCellCase::literal("8", Value::Int(8))
            .named("renamed first")
            .bind_back_eq("?"),
        TypedCellCase::literal("9", Value::Int(9))
            .bind_back_eq("?")
            .named("renamed after"),
        table_case("typed_ok", Value::Int(42)),
    ];
    run_typed_cells(&*seaquel_engine_sqlite::engine(), &config, &cases).await;
    assert!(
        table_names(&config).await.is_empty(),
        "teardown must drop the setup table"
    );
    remove_sqlite(&path);
}

#[tokio::test]
async fn reports_every_failure_and_still_tears_down() {
    let (path, config) = sqlite_file();
    let cases = vec![
        TypedCellCase::literal("7", Value::Int(8)),
        TypedCellCase::literal("7", Value::Int(7)).bind_back_eq("?"),
        // Decodes right, but binding back compares false.
        TypedCellCase::literal("'abc'", Value::from("abc")).bind_back("SELECT ? = 'abd'"),
        table_case("typed_bad", Value::from("42")),
        // Only Bool(true) and Int(1) count as true: text "1" doesn't.
        TypedCellCase::literal("1", Value::Int(1))
            .named("text one")
            .bind_back("SELECT CAST(? AS TEXT)"),
        TypedCellCase {
            setup: vec!["CREATE TABLE".into()],
            ..TypedCellCase::literal("1", Value::Int(1)).named("broken setup")
        },
    ];
    let outcome = AssertUnwindSafe(run_typed_cells(
        &*seaquel_engine_sqlite::engine(),
        &config,
        &cases,
    ))
    .catch_unwind()
    .await;
    let panic = outcome.expect_err("mismatches must fail the run");
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| panic!("panic payload is not a String"));

    assert!(message.contains("5 of 6 typed cells failed"), "{message}");
    assert!(message.contains("SELECT 7 AS v"), "{message}");
    assert!(message.contains("expected: Int(8)"), "{message}");
    assert!(message.contains("actual:   Int(7)"), "{message}");
    assert!(
        message.contains("'abc'") && message.contains("bind back"),
        "{message}"
    );
    assert!(message.contains("typed_bad column"), "{message}");
    assert!(message.contains("expected: Text(\"42\")"), "{message}");
    assert!(
        message.contains("broken setup") && message.contains("setup"),
        "{message}"
    );
    assert!(
        message.contains("text one") && message.contains("wasn't true: Some(Text(\"1\"))"),
        "{message}"
    );

    assert!(
        table_names(&config).await.is_empty(),
        "teardown must run after a failure"
    );
    remove_sqlite(&path);
}

#[tokio::test]
async fn nan_equals_nan() {
    assert!(seaquel_engine_testkit::same_value(
        &Value::Float(f64::NAN),
        &Value::Float(f64::NAN)
    ));
    assert!(seaquel_engine_testkit::same_value(
        &Value::Array(vec![Value::Float(f64::NAN), Value::Null]),
        &Value::Array(vec![Value::Float(f64::NAN), Value::Null]),
    ));
    assert!(!seaquel_engine_testkit::same_value(
        &Value::Float(f64::NAN),
        &Value::Float(1.0)
    ));
    assert!(!seaquel_engine_testkit::same_value(
        &Value::Int(1),
        &Value::Float(1.0)
    ));
}

#[test]
#[should_panic(expected = "bind_back_eq needs a literal case")]
fn bind_back_eq_needs_a_literal() {
    let _ = TypedCellCase {
        literal: None,
        ..TypedCellCase::literal("1", Value::Int(1))
    }
    .bind_back_eq("?");
}
