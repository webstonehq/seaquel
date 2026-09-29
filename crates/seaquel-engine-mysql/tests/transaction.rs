//! `Driver::transaction` against both live servers: all or nothing, and a
//! failure names the statement that failed. SEAQUEL_TEST_MYSQL and
//! SEAQUEL_TEST_MARIADB (see `smoke.rs`).

use seaquel_engine::{BatchStatement, ConnectConfig, Driver, Value};
use seaquel_engine_testkit::{
    config_from_env, run_transaction_case, scratch_name, SmokeSpec, TransactionCase,
};

const MYSQL: &str = "SEAQUEL_TEST_MYSQL";
const MARIADB: &str = "SEAQUEL_TEST_MARIADB";

async fn case(var: &str, case: TransactionCase) {
    let Some(config) = config_from_env(var) else {
        return;
    };
    run_transaction_case(
        &*seaquel_engine_mysql::engine(),
        &config,
        &SmokeSpec::QUESTION_MARK,
        case,
    )
    .await;
}

#[tokio::test]
async fn mysql_a_failing_statement_rolls_back_and_names_its_index() {
    case(MYSQL, TransactionCase::FailingStatementNamesItsIndex).await;
}

#[tokio::test]
async fn mariadb_a_failing_statement_rolls_back_and_names_its_index() {
    case(MARIADB, TransactionCase::FailingStatementNamesItsIndex).await;
}

#[tokio::test]
async fn mysql_a_short_expect_rows_rolls_back_with_no_rows_affected() {
    case(MYSQL, TransactionCase::ShortExpectRowsRollsBack).await;
}

#[tokio::test]
async fn mariadb_a_short_expect_rows_rolls_back_with_no_rows_affected() {
    case(MARIADB, TransactionCase::ShortExpectRowsRollsBack).await;
}

#[tokio::test]
async fn mysql_all_statements_commit() {
    case(MYSQL, TransactionCase::AllStatementsCommit).await;
}

#[tokio::test]
async fn mariadb_all_statements_commit() {
    case(MARIADB, TransactionCase::AllStatementsCommit).await;
}

/// Opens `config`, creates an InnoDB scratch table `(id INT PRIMARY KEY,
/// label VARCHAR(50))`, runs `body`, and drops every table it names.
async fn with_table<F, Fut>(config: &ConnectConfig, body: F)
where
    F: FnOnce(std::sync::Arc<dyn Driver>, String) -> Fut,
    Fut: std::future::Future<Output = Vec<String>>,
{
    let driver = seaquel_engine_mysql::engine()
        .open(config)
        .await
        .expect("open");
    let table = scratch_name("seaquel_tx_");
    driver
        .execute(
            &format!("CREATE TABLE {table} (id INT PRIMARY KEY, label VARCHAR(50)) ENGINE=InnoDB"),
            vec![],
        )
        .await
        .expect("CREATE TABLE");
    let extra = body(driver.clone(), table.clone()).await;
    for t in std::iter::once(table).chain(extra) {
        let _ = driver
            .execute(&format!("DROP TABLE IF EXISTS {t}"), vec![])
            .await;
    }
    driver.close().await.expect("close");
}

fn insert(table: &str, id: i64, label: &str) -> BatchStatement {
    BatchStatement {
        sql: format!("INSERT INTO {table} (id, label) VALUES (?, ?)"),
        params: vec![Value::Int(id), Value::from(label)],
        expect_rows: None,
    }
}

async fn labels(driver: &dyn Driver, table: &str) -> Vec<Vec<Value>> {
    driver
        .query(
            &format!("SELECT id, label FROM {table} ORDER BY id"),
            vec![],
        )
        .await
        .expect("SELECT")
        .rows
}

/// 5b found that MySQL refuses `BEGIN` sent through the prepared protocol
/// (error 1295). The driver starts its transaction with sqlx's `begin()`,
/// which sends it as text. That a real transaction is open shows in the
/// rollback: under autocommit the first INSERT would stay when the second
/// fails. (MySQL has no `@@in_transaction` to read; MariaDB does.)
async fn a_transaction_starts_through_sqlx_begin(var: &str) {
    let Some(config) = config_from_env(var) else {
        return;
    };
    with_table(&config, |driver, table| async move {
        let err = driver
            .transaction(vec![insert(&table, 1, "one"), insert(&table, 1, "dup")])
            .await
            .expect_err("the duplicate key fails");
        assert_eq!(err.index, Some(1), "{}", err.error.message);
        assert_eq!(labels(&*driver, &table).await, Vec::<Vec<Value>>::new());

        driver
            .transaction(vec![insert(&table, 1, "one")])
            .await
            .expect("BEGIN, INSERT, COMMIT");
        assert_eq!(
            labels(&*driver, &table).await,
            vec![vec![Value::Int(1), Value::from("one")]]
        );
        vec![]
    })
    .await;
}

#[tokio::test]
async fn mysql_a_transaction_starts_through_sqlx_begin() {
    a_transaction_starts_through_sqlx_begin(MYSQL).await;
}

#[tokio::test]
async fn mariadb_a_transaction_starts_through_sqlx_begin() {
    a_transaction_starts_through_sqlx_begin(MARIADB).await;
}

/// Recorded, not a behaviour Seaquel wants: DDL commits implicitly on MySQL
/// and MariaDB, so a batch with DDL can't be all or nothing. The INSERT
/// before the `CREATE TABLE` stays committed when a later statement fails,
/// and the failure still names its index. This is why a batch holding DDL
/// applies in order rather than in one transaction (phase 5c, Decision 5).
async fn ddl_inside_commits_implicitly(var: &str) {
    let Some(config) = config_from_env(var) else {
        return;
    };
    with_table(&config, |driver, table| async move {
        let other = scratch_name("seaquel_tx_ddl_");
        let err = driver
            .transaction(vec![
                insert(&table, 1, "one"),
                BatchStatement {
                    sql: format!("CREATE TABLE {other} (id INT PRIMARY KEY)"),
                    params: vec![],
                    expect_rows: None,
                },
                insert(&table, 1, "dup"),
            ])
            .await
            .expect_err("the duplicate key fails");
        assert_eq!(err.index, Some(2), "{}", err.error.message);
        assert_eq!(
            labels(&*driver, &table).await,
            vec![vec![Value::Int(1), Value::from("one")]],
            "the CREATE TABLE committed the INSERT before it"
        );
        vec![other]
    })
    .await;
}

#[tokio::test]
async fn mysql_ddl_inside_commits_implicitly() {
    ddl_inside_commits_implicitly(MYSQL).await;
}

#[tokio::test]
async fn mariadb_ddl_inside_commits_implicitly() {
    ddl_inside_commits_implicitly(MARIADB).await;
}
