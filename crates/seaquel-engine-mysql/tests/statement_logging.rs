//! sqlx logs every statement at DEBUG and each one slower than a second at
//! WARN, with its whole SQL. The driver turns both off, so user SQL (and
//! the values it quotes) never reaches a log (the phase 5a probe found the
//! web server's log holding it).
//!
//! Set SEAQUEL_TEST_MYSQL and/or SEAQUEL_TEST_MARIADB to a ConnectConfig
//! JSON to run this (see `smoke.rs`).

use seaquel_engine_testkit::{capture_logs, config_from_env, run_no_statement_logging};

#[tokio::test]
async fn statements_are_never_logged() {
    capture_logs();
    for var in ["SEAQUEL_TEST_MYSQL", "SEAQUEL_TEST_MARIADB"] {
        let Some(config) = config_from_env(var) else {
            continue;
        };
        let marker = "seaquel_log_marker_7731";
        run_no_statement_logging(
            &*seaquel_engine_mysql::engine(),
            &config,
            &format!("SELECT SLEEP(1.2), '{marker}' AS m"),
            &format!("SELECT '{marker}' AS m"),
            marker,
        )
        .await;
    }
}
