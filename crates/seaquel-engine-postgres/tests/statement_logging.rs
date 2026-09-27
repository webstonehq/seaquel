//! sqlx logs every statement at DEBUG and each one slower than a second at
//! WARN, with its whole SQL. The driver turns both off, so user SQL (and
//! the values it quotes) never reaches a log (the phase 5a probe found the
//! web server's log holding it).
//!
//! Set SEAQUEL_TEST_POSTGRES to a ConnectConfig JSON to run this, e.g.
//! {"driver":"postgres","connection_string":"postgres://postgres@127.0.0.1:5432/seaquel_test"}

use seaquel_engine_testkit::{capture_logs, config_from_env, run_no_statement_logging};

#[tokio::test]
async fn statements_are_never_logged() {
    capture_logs();
    let Some(config) = config_from_env("SEAQUEL_TEST_POSTGRES") else {
        return;
    };
    let marker = "seaquel_log_marker_7731";
    run_no_statement_logging(
        &*seaquel_engine_postgres::engine(),
        &config,
        &format!("SELECT pg_sleep(1.2), '{marker}' AS m"),
        &format!("SELECT '{marker}' AS m"),
        marker,
    )
    .await;
}
