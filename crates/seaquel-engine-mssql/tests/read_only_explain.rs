//! `explain_read_only` on SQL Server (phase 4 security review): SHOWPLAN
//! only compiles, so the plain EXPLAIN stays, but more than one statement
//! is refused, counted from the plans (`SELECT 1 SELECT 2` needs no `;`).
//! Also the read-only query's timeout below the driver's own 60 s.

mod common;

use std::time::Duration;

use common::{ids, ints, open, with_table};
use seaquel_engine::{Driver, ReadOnlyOptions, Value};
use seaquel_engine_testkit::scratch_name;

#[tokio::test]
async fn explain_read_only_plans_one_statement_without_running_it() {
    let Some(driver) = open().await else { return };
    let seq = scratch_name("seaquel_ex_seq_");
    driver
        .execute(&format!("CREATE SEQUENCE {seq} START WITH 1"), vec![])
        .await
        .expect("create sequence");

    with_table(&driver, |driver, table| {
        let seq = seq.clone();
        async move {
            driver
                .execute(&format!("INSERT INTO {table} VALUES (1, N'one')"), vec![])
                .await
                .expect("insert");

            let plan = driver
                .explain_read_only(
                    &format!("SELECT label FROM {table} WHERE id = @P1"),
                    vec![Value::Int(1)],
                    Some(Duration::from_secs(10)),
                )
                .await
                .expect("plan");
            assert!(!plan.is_analyze);

            // Compiled, not run: the row and the sequence are untouched.
            driver
                .explain_read_only(&format!("DELETE FROM {table}"), vec![], None)
                .await
                .expect("plan of a DELETE");
            driver
                .explain_read_only(&format!("SELECT NEXT VALUE FOR {seq} AS n"), vec![], None)
                .await
                .expect("plan of NEXT VALUE FOR");

            for sql in [
                "SELECT 1 AS a SELECT 2 AS b".to_string(),
                format!("SELECT 1 AS a; DELETE FROM {table}"),
                format!("SELECT 1 AS a DELETE FROM {table}"),
            ] {
                let err = driver
                    .explain_read_only(&sql, vec![], None)
                    .await
                    .expect_err(&sql);
                assert_eq!(err.code, "READ_ONLY", "{sql}: {}", err.message);
                assert_eq!(err.message, seaquel_engine::EXPLAIN_ONE_STATEMENT);
            }
            assert_eq!(ids(driver, &table).await, ints(&[1]));
            let next = driver
                .query(
                    &format!(
                        "SELECT CAST(current_value AS bigint) AS v FROM sys.sequences \
                         WHERE name = N'{seq}'"
                    ),
                    vec![],
                )
                .await
                .expect("sequence");
            assert_eq!(next.rows, vec![vec![Value::Int(1)]]);

            // The held session still works after the refusals.
            let one = driver
                .query("SELECT 1 AS one", vec![])
                .await
                .expect("SELECT 1");
            assert_eq!(one.rows, vec![vec![Value::Int(1)]]);
        }
    })
    .await;

    driver
        .execute(&format!("DROP SEQUENCE {seq}"), vec![])
        .await
        .expect("drop sequence");
}

/// A timeout under the driver's own 60 s ends the call with `TIMEOUT`.
#[tokio::test]
async fn read_only_timeout_below_the_drivers_own() {
    let Some(driver) = open().await else { return };
    let options = ReadOnlyOptions::default().with_timeout(Some(Duration::from_millis(500)));
    let err = tokio::time::timeout(
        Duration::from_secs(10),
        driver.query_read_only_with("SELECT 1 AS a; WAITFOR DELAY '00:00:30'", vec![], options),
    )
    .await
    .expect("the timeout didn't end the call")
    .expect_err("timed out");
    assert_eq!(err.code, "TIMEOUT", "{}", err.message);
    assert!(err.message.contains("0.5 seconds"), "{}", err.message);

    let r = driver
        .query_read_only_with("SELECT 1 AS one", vec![], ReadOnlyOptions::default())
        .await
        .expect("SELECT 1");
    assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
}
