//! `Driver::transaction` on SQL Server's held session: all or nothing, and a
//! failure names the statement that failed. SEAQUEL_TEST_MSSQL (see
//! `smoke.rs`). The refusal to nest inside a transaction opened by hand is
//! `src/live_tests.rs`'s `a_transaction_refuses_to_nest_in_one_opened_by_hand`.

mod common;

use seaquel_engine_testkit::{run_transaction_case, SmokeSpec, TransactionCase};

async fn case(case: TransactionCase) {
    let Some(config) = common::config() else {
        return;
    };
    run_transaction_case(
        &*seaquel_engine_mssql::engine(),
        &config,
        &SmokeSpec::AT_P,
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
