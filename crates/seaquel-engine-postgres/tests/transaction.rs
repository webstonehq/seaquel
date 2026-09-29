//! `Driver::transaction` against a live server: all or nothing, and a failure
//! names the statement that failed. Set SEAQUEL_TEST_POSTGRES (see
//! `smoke.rs`).

use seaquel_engine::{BatchStatement, Value};
use seaquel_engine_testkit::{
    config_from_env, run_transaction_case, scratch_name, SmokeSpec, TransactionCase,
};

async fn case(case: TransactionCase) {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    run_transaction_case(
        &*seaquel_engine_postgres::engine(),
        &config,
        &SmokeSpec::DOLLAR,
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

/// A deferred constraint is checked at COMMIT, so the failure belongs to no
/// statement: `index` is `None`, and nothing is written.
#[tokio::test]
async fn a_failing_commit_names_no_index() {
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let driver = seaquel_engine_postgres::engine()
        .open(&config)
        .await
        .expect("open");
    let table = scratch_name("seaquel_tx_");
    driver
        .execute(
            &format!(
                "CREATE TABLE {table} (id INTEGER, \
                 CONSTRAINT {table}_u UNIQUE (id) DEFERRABLE INITIALLY DEFERRED)"
            ),
            vec![],
        )
        .await
        .expect("CREATE TABLE");
    let insert = |id: i64| BatchStatement {
        sql: format!("INSERT INTO {table} (id) VALUES ($1)"),
        params: vec![Value::Int(id)],
        expect_rows: None,
    };
    let err = driver
        .transaction(vec![insert(1), insert(1)])
        .await
        .expect_err("COMMIT must fail on the deferred UNIQUE");
    let count = driver
        .query(&format!("SELECT COUNT(*) FROM {table}"), vec![])
        .await
        .expect("COUNT");
    let _ = driver.execute(&format!("DROP TABLE {table}"), vec![]).await;
    driver.close().await.expect("close");

    assert_eq!(err.index, None, "{}", err.error.message);
    assert_eq!(err.error.code, "EXECUTE_ERROR");
    assert_eq!(count.rows[0][0].as_i64(), Some(0));
}
