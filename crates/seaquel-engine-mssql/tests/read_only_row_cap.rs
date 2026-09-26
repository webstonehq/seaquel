//! `query_read_only` past the row cap, in its own test binary:
//! `max_query_rows` reads SEAQUEL_MAX_QUERY_ROWS once per process.

mod common;

use common::{ids, ints, open, with_table};
use seaquel_engine::{Driver, Value};

/// Past the cap the rest of the response is read and dropped, so the
/// driver can still check the transaction: a query that committed and then
/// returned too many rows is reported as an escape, not as
/// RESULT_TOO_LARGE.
#[tokio::test]
async fn an_escape_past_the_row_cap_is_still_reported() {
    std::env::set_var("SEAQUEL_MAX_QUERY_ROWS", "5");
    assert_eq!(seaquel_engine::max_query_rows(), 5);
    let Some(driver) = open().await else { return };
    const TEN: &str =
        "SELECT n FROM (VALUES (1), (2), (3), (4), (5), (6), (7), (8), (9), (10)) v(n)";

    with_table(&driver, |driver, table| async move {
        driver
            .execute(
                &format!("INSERT INTO {table} VALUES (1, N'one'), (2, N'two')"),
                vec![],
            )
            .await
            .expect("insert");

        // Nothing escaped: RESULT_TOO_LARGE as usual.
        let err = driver
            .query_read_only(TEN, vec![])
            .await
            .expect_err("over the cap");
        assert_eq!(err.code, "RESULT_TOO_LARGE", "{err:?}");

        // Rolled back past the cap too.
        let err = driver
            .query_read_only(&format!("DELETE FROM {table} WHERE id = 2; {TEN}"), vec![])
            .await
            .expect_err("over the cap");
        assert_eq!(err.code, "RESULT_TOO_LARGE", "{err:?}");
        assert_eq!(ids(driver, &table).await, ints(&[1, 2]));

        // Committed, then over the cap: reported as the escape it is.
        let err = driver
            .query_read_only(
                &format!("DELETE FROM {table} WHERE id = 1; COMMIT; COMMIT; {TEN}"),
                vec![],
            )
            .await
            .expect_err("an escape");
        assert_eq!(err.code, "READ_ONLY", "{err:?}");
        assert!(
            err.message
                .starts_with("The query ended the read-only transaction"),
            "{err:?}"
        );
        assert!(err.message.contains("5-row cap"), "{err:?}");
        assert_eq!(ids(driver, &table).await, ints(&[2]), "the documented gap");

        let r = driver
            .query_read_only("SELECT 1 AS a", vec![])
            .await
            .expect("the next call");
        assert_eq!(r.rows, vec![vec![Value::Int(1)]]);
    })
    .await;
}
