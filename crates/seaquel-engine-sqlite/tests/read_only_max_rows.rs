//! `max_rows` (and `max_bytes`) on `query_read_only`, in its own test binary: `max_query_rows`
//! reads SEAQUEL_MAX_QUERY_ROWS once per process, and this test lowers it.

use seaquel_engine::ConnectConfig;
use seaquel_engine_testkit::{run_max_bytes, run_max_rows, use_max_rows_test_cap};

#[tokio::test]
async fn max_rows_truncates() {
    use_max_rows_test_cap();
    let config: ConnectConfig = serde_json::from_value(serde_json::json!({
        "driver": "sqlite",
        "connection_string": "sqlite::memory:"
    }))
    .unwrap();
    run_max_rows(&*seaquel_engine_sqlite::engine(), &config).await;
}

/// 200 rows of 100,000 characters each (`hex(zeroblob(50000))` is 100,000
/// zeros).
#[tokio::test]
async fn max_bytes_truncates() {
    use_max_rows_test_cap();
    let config: ConnectConfig = serde_json::from_value(serde_json::json!({
        "driver": "sqlite",
        "connection_string": "sqlite::memory:"
    }))
    .unwrap();
    let sql = "WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM s WHERE i < 200) \
               SELECT replace(hex(zeroblob(50000)), '0', 'x') AS n FROM s";
    run_max_bytes(&*seaquel_engine_sqlite::engine(), &config, sql).await;
}
