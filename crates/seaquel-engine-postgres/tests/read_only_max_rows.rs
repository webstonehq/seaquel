//! `max_rows` (and `max_bytes`) on `query_read_only`, in its own test binary: `max_query_rows`
//! reads SEAQUEL_MAX_QUERY_ROWS once per process, and these tests lower it.
//!
//! Set SEAQUEL_TEST_POSTGRES to a ConnectConfig JSON to run this.

use seaquel_engine::Value;
use seaquel_engine_testkit::{config_from_env, run_max_bytes, run_max_rows, use_max_rows_test_cap};

#[tokio::test]
async fn max_rows_truncates() {
    use_max_rows_test_cap();
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    run_max_rows(&*seaquel_engine_postgres::engine(), &config).await;
}

/// A truncated result closes the connection instead of sending `ROLLBACK`,
/// which would first read every row the server still streams (about 4 s
/// here when it did; well under 0.1 s without).
#[tokio::test]
async fn a_truncated_huge_result_returns_fast() {
    use_max_rows_test_cap();
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let driver = seaquel_engine_postgres::engine()
        .open(&config)
        .await
        .expect("open");
    let started = tokio::time::Instant::now();
    let r = driver
        .query_read_only("SELECT generate_series(1, 20000000) AS n", vec![], Some(10))
        .await
        .expect("truncated");
    let elapsed = started.elapsed();
    let one = driver
        .query_read_only("SELECT 1 AS one", vec![], None)
        .await;
    driver.close().await.expect("close");
    assert_eq!(r.rows.len(), 10);
    assert!(r.truncated);
    assert_eq!(r.rows[0], vec![Value::Int(1)]);
    assert!(
        elapsed < std::time::Duration::from_secs(1),
        "{elapsed:?} to truncate 20M rows to 10"
    );
    assert_eq!(one.expect("SELECT 1").rows, vec![vec![Value::Int(1)]]);
}

#[tokio::test]
async fn max_bytes_truncates() {
    use_max_rows_test_cap();
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let sql = "SELECT repeat('x', 100000) AS n FROM generate_series(1, 200)";
    run_max_bytes(&*seaquel_engine_postgres::engine(), &config, sql).await;
}

/// The budget stops a result the row cap alone would let through whole:
/// 1,000 rows of 1 MB (1 GB) come back as 9 rows under an 8 MB budget,
/// quickly, because the server's stream is dropped there.
#[tokio::test]
async fn a_byte_budget_stops_a_huge_result_early() {
    use_max_rows_test_cap();
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let driver = seaquel_engine_postgres::engine()
        .open(&config)
        .await
        .expect("open");
    let started = tokio::time::Instant::now();
    let options = seaquel_engine::ReadOnlyOptions::default()
        .with_max_rows(Some(1000))
        .with_max_bytes(Some(8 * 1024 * 1024));
    let r = driver
        .query_read_only_with(
            "SELECT repeat('x', 1048576) AS n FROM generate_series(1, 1000)",
            vec![],
            options,
        )
        .await
        .expect("truncated");
    let elapsed = started.elapsed();
    driver.close().await.expect("close");
    assert!(r.truncated);
    assert_eq!(r.rows.len(), 8, "the 8th row reaches 8 MB");
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "{elapsed:?} to stop at 8 MB"
    );
}
