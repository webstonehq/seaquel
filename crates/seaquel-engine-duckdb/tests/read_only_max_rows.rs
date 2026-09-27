//! `max_rows` (and `max_bytes`) on `query_read_only`, in its own test binary: `max_query_rows`
//! reads SEAQUEL_MAX_QUERY_ROWS once per process, and these tests lower it.

use seaquel_engine::ConnectConfig;
use seaquel_engine_testkit::{run_max_bytes, run_max_rows, use_max_rows_test_cap};

fn memory() -> ConnectConfig {
    serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" })).unwrap()
}

#[tokio::test]
async fn max_rows_truncates() {
    use_max_rows_test_cap();
    run_max_rows(&*seaquel_engine_duckdb::engine(), &memory()).await;
}

/// The wrapper's `LIMIT` is `max_rows + 1`, so DuckDB stops there instead of
/// materializing two billion rows.
#[tokio::test]
async fn a_truncated_huge_result_returns_fast() {
    use_max_rows_test_cap();
    let driver = seaquel_engine_duckdb::engine()
        .open(&memory())
        .await
        .expect("open");
    let started = tokio::time::Instant::now();
    let r = driver
        .query_read_only("SELECT * FROM range(2000000000)", vec![], Some(10))
        .await
        .expect("truncated");
    assert_eq!(r.rows.len(), 10);
    assert!(r.truncated);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "{:?} to truncate to 10 rows",
        started.elapsed()
    );
}

#[tokio::test]
async fn max_bytes_truncates() {
    use_max_rows_test_cap();
    let sql = "SELECT repeat('x', 100000) AS n FROM range(200)";
    run_max_bytes(&*seaquel_engine_duckdb::engine(), &memory(), sql).await;
}
