//! `explain_read_only` on DuckDB (phase 4 security review): one statement,
//! planned in a read-only transaction on a connection of its own. `explain`
//! hands the string to duckdb-rs's `prepare`, which runs every statement
//! but the last itself and returns the last to run: `SELECT 1; DELETE …`
//! deleted for real.

#[path = "common/engine.rs"]
mod engine_switch;

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, Driver, Value};

async fn open() -> Arc<dyn Driver> {
    let config: ConnectConfig =
        serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" }))
            .unwrap();
    let driver = engine_switch::engine().open(&config).await.expect("open");
    for sql in [
        "CREATE TABLE t (n INTEGER PRIMARY KEY)",
        "INSERT INTO t VALUES (1), (2)",
        "CREATE SEQUENCE s",
    ] {
        driver.execute(sql, vec![]).await.expect(sql);
    }
    driver
}

async fn rows(driver: &dyn Driver) -> i64 {
    driver
        .query("SELECT count(*) FROM t", vec![])
        .await
        .expect("count")
        .rows[0][0]
        .as_i64()
        .expect("count")
}

#[tokio::test]
async fn explain_read_only_plans_one_statement() {
    let driver = open().await;

    let plan = driver
        .explain_read_only("SELECT * FROM t WHERE n = ?", vec![Value::Int(1)], None)
        .await
        .expect("plan");
    assert!(!plan.is_analyze);
    // DuckDB's binder marks `nextval` as a write, which the read-only
    // transaction refuses even for a plan.
    let err = driver
        .explain_read_only("SELECT nextval('s') AS v;", vec![], None)
        .await
        .expect_err("plan of nextval");
    assert_eq!(err.code, "READ_ONLY", "{}", err.message);

    for sql in [
        "SELECT 1; DELETE FROM t",
        "SELECT 1; COMMIT; DELETE FROM t",
        "SELECT 1; SELECT 2",
    ] {
        let err = driver
            .explain_read_only(sql, vec![], None)
            .await
            .expect_err(sql);
        assert_eq!(err.code, "READ_ONLY", "{sql}: {}", err.message);
        assert_eq!(err.message, seaquel_engine::EXPLAIN_ONE_STATEMENT);
    }
    assert_eq!(rows(&*driver).await, 2);
    // Planning didn't advance the sequence.
    let next = driver
        .query("SELECT nextval('s') AS v", vec![])
        .await
        .expect("nextval");
    assert_eq!(next.rows, vec![vec![Value::Int(1)]]);

    // The fixture reaches the write through a plain EXPLAIN.
    let _ = driver
        .explain("SELECT 1; DELETE FROM t", vec![], false)
        .await;
    assert_eq!(rows(&*driver).await, 0);
    driver.close().await.expect("close");
}
