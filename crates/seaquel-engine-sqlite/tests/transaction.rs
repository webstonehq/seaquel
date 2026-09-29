//! `Driver::transaction` on a temp-file database: all or nothing, and a
//! failure names the statement that failed.

use std::path::PathBuf;

use seaquel_engine::{BatchStatement, ConnectConfig, Value};
use seaquel_engine_testkit::{run_transaction_case, SmokeSpec, TransactionCase};

/// A fresh database file, deleted on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("seaquel-tx-{}.sqlite", uuid::Uuid::new_v4())))
    }

    fn config(&self) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": "sqlite",
            "connection_string": format!("sqlite:{}", self.0.display()),
            "create_if_missing": true
        }))
        .unwrap()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn case(case: TransactionCase) {
    let db = TempDb::new();
    run_transaction_case(
        &*seaquel_engine_sqlite::engine(),
        &db.config(),
        &SmokeSpec::QUESTION_MARK,
        case,
    )
    .await;
}

#[tokio::test]
async fn a_failing_statement_rolls_back_and_names_its_index() {
    case(TransactionCase::FailingStatementNamesItsIndex).await;
}

#[tokio::test]
async fn a_short_expect_rows_rolls_back_with_no_rows_affected() {
    case(TransactionCase::ShortExpectRowsRollsBack).await;
}

#[tokio::test]
async fn all_statements_commit() {
    case(TransactionCase::AllStatementsCommit).await;
}

/// A deferred foreign key is checked at COMMIT, so the failure belongs to no
/// statement: `index` is `None`, and nothing is written.
#[tokio::test]
async fn a_failing_commit_names_no_index() {
    let db = TempDb::new();
    let driver = seaquel_engine_sqlite::engine()
        .open(&db.config())
        .await
        .expect("open");
    driver
        .execute("CREATE TABLE parent (id INTEGER PRIMARY KEY)", vec![])
        .await
        .unwrap();
    driver
        .execute(
            "CREATE TABLE child (id INTEGER PRIMARY KEY, parent_id INTEGER \
             REFERENCES parent (id) DEFERRABLE INITIALLY DEFERRED)",
            vec![],
        )
        .await
        .unwrap();
    let err = driver
        .transaction(vec![
            BatchStatement {
                sql: "INSERT INTO child (id, parent_id) VALUES (?, ?)".into(),
                params: vec![Value::Int(1), Value::Int(42)],
                expect_rows: None,
            },
            BatchStatement {
                sql: "INSERT INTO child (id, parent_id) VALUES (?, NULL)".into(),
                params: vec![Value::Int(2)],
                expect_rows: None,
            },
        ])
        .await
        .expect_err("COMMIT must fail on the deferred foreign key");
    assert_eq!(err.index, None, "{}", err.error.message);
    assert_eq!(err.error.code, "EXECUTE_ERROR");
    let count = driver
        .query("SELECT COUNT(*) FROM child", vec![])
        .await
        .unwrap();
    assert_eq!(count.rows[0][0].as_i64(), Some(0));
    driver.close().await.expect("close");
}
