//! Helpers shared by the live test files. Tests run in `seaquel_test`.
#![allow(dead_code)]

use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;
use seaquel_engine::{BatchStatement, ConnectConfig, Driver, Value};
use seaquel_engine_mssql::MssqlDriver;
use seaquel_engine_testkit::{config_from_env, scratch_name};

/// SEAQUEL_TEST_MSSQL, pointed at the `seaquel_test` database.
pub fn config() -> Option<ConnectConfig> {
    let mut config = config_from_env("SEAQUEL_TEST_MSSQL")?;
    config.database = Some("seaquel_test".to_string());
    Some(config)
}

pub async fn open() -> Option<MssqlDriver> {
    Some(MssqlDriver::connect(&config()?).await.expect("connect"))
}

/// Runs `body` with a fresh `t11_…` table (`id INT PRIMARY KEY, label
/// NVARCHAR(50)`), dropping it afterwards even if `body` panics.
pub async fn with_table<'a, F, Fut>(driver: &'a MssqlDriver, body: F)
where
    F: FnOnce(&'a MssqlDriver, String) -> Fut,
    Fut: Future<Output = ()> + 'a,
{
    let table = scratch_name("t11_");
    driver
        .execute(
            &format!("CREATE TABLE {table} (id INT PRIMARY KEY, label NVARCHAR(50))"),
            vec![],
        )
        .await
        .expect("CREATE TABLE");
    let outcome = AssertUnwindSafe(body(driver, table.clone()))
        .catch_unwind()
        .await;
    // Best effort, so a failure here can't hide the test's own.
    if let Err(e) = driver.execute(&format!("DROP TABLE {table}"), vec![]).await {
        eprintln!("dropping {table} failed: {}", e.message);
    }
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

pub fn ints(values: &[i64]) -> Vec<Value> {
    values.iter().map(|&v| Value::Int(v)).collect()
}

pub async fn ids(driver: &MssqlDriver, table: &str) -> Vec<Value> {
    driver
        .query(&format!("SELECT id FROM {table} ORDER BY id"), vec![])
        .await
        .expect("SELECT ids")
        .rows
        .into_iter()
        .map(|mut r| r.remove(0))
        .collect()
}

pub async fn trancount(driver: &MssqlDriver) -> i64 {
    let r = driver
        .query("SELECT @@TRANCOUNT AS n", vec![])
        .await
        .expect("@@TRANCOUNT");
    r.rows[0][0].as_i64().expect("an integer")
}

pub fn insert(table: &str, id: Value, label: &str) -> BatchStatement {
    BatchStatement {
        sql: format!("INSERT INTO {table} (id, label) VALUES (@P1, @P2)"),
        params: vec![id, Value::from(label)],
        expect_rows: None,
    }
}

/// Drops what earlier runs killed before their cleanup left behind: every
/// view, procedure, function and table whose name starts with `prefix` (in
/// any schema), and every schema whose name does. Best effort. Test
/// databases only: it can't tell a crashed run's objects from those of a
/// run still going.
pub async fn drop_stale(driver: &MssqlDriver, prefix: &str) {
    let like = prefix.replace('_', "[_]");
    let sql = format!(
        "DECLARE @sql nvarchar(max) = N''; \
         SELECT @sql += CASE o.type WHEN 'V' THEN N'DROP VIEW ' WHEN 'P' THEN N'DROP PROCEDURE ' \
             WHEN 'U' THEN N'DROP TABLE ' ELSE N'DROP FUNCTION ' END \
             + QUOTENAME(s.name) + N'.' + QUOTENAME(o.name) + N'; ' \
           FROM sys.objects o JOIN sys.schemas s ON s.schema_id = o.schema_id \
           WHERE o.type IN ('V', 'P', 'FN', 'IF', 'TF', 'U') AND (o.name LIKE N'{like}%' OR s.name LIKE N'{like}%') \
           ORDER BY CASE o.type WHEN 'U' THEN 1 ELSE 0 END; \
         SELECT @sql += N'DROP SCHEMA ' + QUOTENAME(name) + N'; ' FROM sys.schemas WHERE name LIKE N'{like}%'; \
         EXEC (@sql);"
    );
    if let Err(e) = driver.execute(&sql, vec![]).await {
        eprintln!("stale cleanup of {prefix}*: {}", e.message);
    }
}
