//! `explain_read_only` on SQLite (phase 4 security review): one statement,
//! on the read-only path's own connection and gate. `explain` runs every
//! statement in the string, the second one for real; this one refuses it.

use std::sync::Arc;

use seaquel_engine::{ConnectConfig, Driver, Value};

async fn open() -> Arc<dyn Driver> {
    let config: ConnectConfig = serde_json::from_value(serde_json::json!({
        "driver": "sqlite",
        "connection_string": "sqlite::memory:"
    }))
    .unwrap();
    let driver = seaquel_engine_sqlite::engine()
        .open(&config)
        .await
        .expect("open");
    driver
        .execute("CREATE TABLE t (n INTEGER PRIMARY KEY, s TEXT)", vec![])
        .await
        .expect("create");
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
    assert_eq!(plan.plan.relation_name.as_deref(), Some("t"));
    assert_eq!(plan.plan.index_cond.as_deref(), Some("rowid=?"));
    // A trailing `;` is still one statement.
    driver
        .explain_read_only("SELECT * FROM t;", vec![], None)
        .await
        .expect("trailing semicolon");

    for sql in [
        "SELECT 1; INSERT INTO t VALUES (1, 'x')",
        "SELECT 1; PRAGMA query_only = OFF; INSERT INTO t VALUES (1, 'x')",
    ] {
        let err = driver
            .explain_read_only(sql, vec![], None)
            .await
            .expect_err(sql);
        assert_eq!(err.code, "READ_ONLY", "{sql}: {}", err.message);
        assert_eq!(rows(&*driver).await, 0, "{sql}");
    }

    // The fixture reaches the write through a plain EXPLAIN.
    driver
        .explain("SELECT 1; INSERT INTO t VALUES (1, 'x')", vec![], false)
        .await
        .ok();
    assert_eq!(rows(&*driver).await, 1);
    driver.close().await.expect("close");
}
