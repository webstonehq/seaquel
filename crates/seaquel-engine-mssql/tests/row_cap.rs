//! The per-query row cap, in its own test binary: `max_query_rows` reads
//! SEAQUEL_MAX_QUERY_ROWS once per process.

mod common;

use common::{ids, insert, ints, open, trancount, with_table};
use seaquel_engine::{Driver, Value};

#[tokio::test]
async fn the_row_cap_fails_the_query_and_leaves_the_connection_usable() {
    std::env::set_var("SEAQUEL_MAX_QUERY_ROWS", "3");
    assert_eq!(seaquel_engine::max_query_rows(), 3);
    let Some(driver) = open().await else { return };
    const FOUR: &str = "SELECT n FROM (VALUES (1), (2), (3), (4)) v(n)";
    const THREE: &str = "SELECT n FROM (VALUES (1), (2), (3)) v(n)";

    with_table(&driver, |driver, table| async move {
        let three = driver.query(THREE, vec![]).await.expect("at the cap");
        assert_eq!(three.rows.len(), 3);

        // Over the cap: the rest of the response is left unread.
        let err = driver.query(FOUR, vec![]).await.expect_err("over the cap");
        assert_eq!(err.code, "RESULT_TOO_LARGE");
        let next = driver
            .query("SELECT 5 AS n", vec![])
            .await
            .expect("next query");
        assert_eq!(next.rows, vec![ints(&[5])]);

        let err = driver
            .query(FOUR, vec![])
            .await
            .expect_err("over the cap again");
        assert_eq!(err.code, "RESULT_TOO_LARGE");
        driver
            .transaction(vec![insert(&table, Value::Int(1), "one")])
            .await
            .expect("transaction after RESULT_TOO_LARGE");
        assert_eq!(ids(driver, &table).await, ints(&[1]));
        assert_eq!(trancount(driver).await, 0);

        // `query` keeps only the first set, so a bigger trailing set is
        // read and dropped, not counted.
        let first = driver
            .query(&format!("SELECT 1 AS a; {FOUR}; {FOUR}"), vec![])
            .await
            .expect("trailing sets over the cap");
        assert_eq!(first.rows, vec![ints(&[1])]);

        // `query_results` keeps them all, and counts them all.
        let err = driver
            .query_results(
                "SELECT 1 AS a UNION ALL SELECT 2; SELECT 3 AS b UNION ALL SELECT 4",
                vec![],
            )
            .await
            .expect_err("2 + 2 rows over a cap of 3");
        assert_eq!(err.code, "RESULT_TOO_LARGE");
        let next = driver
            .query("SELECT 6 AS n", vec![])
            .await
            .expect("next query");
        assert_eq!(next.rows, vec![ints(&[6])]);
    })
    .await;
}
