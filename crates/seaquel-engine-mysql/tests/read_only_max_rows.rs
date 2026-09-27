//! `max_rows` and `max_bytes` on `query_read_only` on MySQL and MariaDB, in its own test
//! binary: `max_query_rows` reads SEAQUEL_MAX_QUERY_ROWS once per process,
//! and these tests lower it.
//!
//! Set SEAQUEL_TEST_MYSQL and SEAQUEL_TEST_MARIADB to ConnectConfig JSON to
//! run this.

use seaquel_engine_testkit::{config_from_env, run_max_bytes, run_max_rows, use_max_rows_test_cap};

#[tokio::test]
async fn max_rows_truncates_on_mysql() {
    use_max_rows_test_cap();
    let Some(config) = config_from_env("SEAQUEL_TEST_MYSQL") else {
        return;
    };
    run_max_rows(&*seaquel_engine_mysql::engine(), &config).await;
}

#[tokio::test]
async fn max_rows_truncates_on_mariadb() {
    use_max_rows_test_cap();
    let Some(config) = config_from_env("SEAQUEL_TEST_MARIADB") else {
        return;
    };
    run_max_rows(&*seaquel_engine_mysql::engine(), &config).await;
}

const BIG_CELLS: &str =
    "WITH RECURSIVE s(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM s WHERE i < 200) \
                         SELECT REPEAT('x', 100000) AS n FROM s";

#[tokio::test]
async fn max_bytes_truncates_on_mysql() {
    use_max_rows_test_cap();
    let Some(config) = config_from_env("SEAQUEL_TEST_MYSQL") else {
        return;
    };
    run_max_bytes(&*seaquel_engine_mysql::engine(), &config, BIG_CELLS).await;
}

#[tokio::test]
async fn max_bytes_truncates_on_mariadb() {
    use_max_rows_test_cap();
    let Some(config) = config_from_env("SEAQUEL_TEST_MARIADB") else {
        return;
    };
    run_max_bytes(&*seaquel_engine_mysql::engine(), &config, BIG_CELLS).await;
}
