//! `Driver::transaction` on an in-memory database: all or nothing, and a
//! failure names the statement that failed.

use seaquel_engine::{BatchStatement, ConnectConfig, Value};
use seaquel_engine_testkit::{run_transaction_case, SmokeSpec, TransactionCase};

fn memory() -> ConnectConfig {
    serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" })).unwrap()
}

async fn case(case: TransactionCase) {
    run_transaction_case(
        &*seaquel_engine_duckdb::engine(),
        &memory(),
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

/// A transaction opened by hand refuses the batch before BEGIN, so DuckDB
/// doesn't abort the user's transaction (a failed nested BEGIN would). The
/// refusal belongs to no statement, and the user's COMMIT keeps their row.
#[tokio::test]
async fn a_hand_opened_transaction_refuses_the_batch() {
    let driver = seaquel_engine_duckdb::engine()
        .open(&memory())
        .await
        .expect("open");
    driver
        .execute("CREATE TABLE t (n BIGINT)", vec![])
        .await
        .unwrap();
    driver
        .execute("BEGIN", vec![])
        .await
        .expect("BEGIN by hand");
    driver
        .execute("INSERT INTO t VALUES (1)", vec![])
        .await
        .unwrap();
    let err = driver
        .transaction(vec![BatchStatement {
            sql: "INSERT INTO t VALUES (?)".into(),
            params: vec![Value::Int(2)],
            expect_rows: None,
        }])
        .await
        .expect_err("nested transaction");
    assert_eq!(err.index, None, "{}", err.error.message);
    assert_eq!(err.error.code, seaquel_engine::TRANSACTION_OPEN);
    assert_eq!(err.error.message, seaquel_engine::TRANSACTION_ALREADY_OPEN);

    // The hand-opened transaction is untouched and still usable.
    let inside = driver.query("SELECT n FROM t", vec![]).await.unwrap();
    assert_eq!(inside.rows, vec![vec![Value::Int(1)]]);
    driver
        .execute("COMMIT", vec![])
        .await
        .expect("COMMIT by hand");
    let count = driver
        .query("SELECT count(*) FROM t", vec![])
        .await
        .unwrap();
    assert_eq!(count.rows[0][0].as_i64(), Some(1), "the user's row stays");

    // In autocommit again: a transaction runs.
    driver
        .transaction(vec![BatchStatement {
            sql: "INSERT INTO t VALUES (?)".into(),
            params: vec![Value::Int(3)],
            expect_rows: None,
        }])
        .await
        .expect("a transaction now");
    let count = driver
        .query("SELECT count(*) FROM t", vec![])
        .await
        .unwrap();
    assert_eq!(count.rows[0][0].as_i64(), Some(2));
    driver.close().await.expect("close");
}
