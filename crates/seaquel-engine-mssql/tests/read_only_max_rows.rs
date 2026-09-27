//! `max_rows` (and `max_bytes`) on `query_read_only`, in its own test binary: `max_query_rows`
//! reads SEAQUEL_MAX_QUERY_ROWS once per process, and these tests lower it.

mod common;

use common::{config, ids, ints, open, with_table};
use seaquel_engine::{Driver, Value};
use seaquel_engine_testkit::{
    run_max_bytes, run_max_rows, use_max_rows_test_cap, TEN_THOUSAND_ROWS,
};

#[tokio::test]
async fn max_rows_truncates() {
    use_max_rows_test_cap();
    let Some(config) = config() else { return };
    run_max_rows(&*seaquel_engine_mssql::engine(), &config).await;
}

/// Past `max_rows` the rest of the response is still read and dropped, so
/// the driver checks the transaction afterwards as it does without it: a
/// query that committed and then returned more rows is reported as an
/// escape, not as a truncated result, and one that only wrote is rolled
/// back.
#[tokio::test]
async fn an_escape_is_still_reported_when_truncating() {
    use_max_rows_test_cap();
    let Some(driver) = open().await else { return };

    with_table(&driver, |driver, table| async move {
        driver
            .execute(
                &format!("INSERT INTO {table} VALUES (1, N'one'), (2, N'two')"),
                vec![],
            )
            .await
            .expect("insert");

        // Rolled back, and truncated as asked.
        let r = driver
            .query_read_only(
                &format!("DELETE FROM {table} WHERE id = 2; {TEN_THOUSAND_ROWS}"),
                vec![],
                Some(10),
            )
            .await
            .expect("truncated");
        assert_eq!(r.rows.len(), 10);
        assert!(r.truncated);
        assert_eq!(ids(driver, &table).await, ints(&[1, 2]));

        // Committed, then more rows than `max_rows`: the escape wins.
        let err = driver
            .query_read_only(
                &format!("DELETE FROM {table} WHERE id = 1; COMMIT; COMMIT; {TEN_THOUSAND_ROWS}"),
                vec![],
                Some(10),
            )
            .await
            .expect_err("an escape");
        assert_eq!(err.code, "READ_ONLY", "{err:?}");
        assert!(
            err.message
                .starts_with("The query ended the read-only transaction"),
            "{err:?}"
        );
        assert_eq!(ids(driver, &table).await, ints(&[2]), "the documented gap");

        // An error after the rows `max_rows` kept still fails the query.
        let err = driver
            .query_read_only(
                &format!("{TEN_THOUSAND_ROWS}; SELECT 1 / 0 AS boom"),
                vec![],
                Some(10),
            )
            .await
            .expect_err("the error after the kept rows");
        assert_eq!(err.code, "QUERY_ERROR", "{err:?}");

        let r = driver
            .query_read_only("SELECT 1 AS a", vec![], Some(10))
            .await
            .expect("the next call");
        assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
        assert!(!r.truncated);
    })
    .await;
}

/// Past the budget the rest of the response is read and dropped, not kept.
#[tokio::test]
async fn max_bytes_truncates() {
    use_max_rows_test_cap();
    let Some(config) = config() else { return };
    let sql = "SELECT REPLICATE(CAST('x' AS varchar(max)), 100000) AS n \
               FROM (SELECT TOP 200 1 AS a FROM sys.all_objects a CROSS JOIN sys.all_objects b) t";
    run_max_bytes(&*seaquel_engine_mssql::engine(), &config, sql).await;
}
