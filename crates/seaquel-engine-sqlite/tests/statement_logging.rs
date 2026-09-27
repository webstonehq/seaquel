//! sqlx logs every statement at DEBUG and each one slower than a second at
//! WARN, with its whole SQL. The driver turns both off, on its pool and on
//! the read-only path's private connection, so user SQL never reaches a log.

use seaquel_engine_testkit::{capture_logs, run_no_statement_logging};
use seaquel_types::{ConnectConfig, DriverType};

#[tokio::test]
async fn statements_are_never_logged() {
    capture_logs();
    let dir = std::env::temp_dir().join(format!("seaquel-log-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.db");
    let config: ConnectConfig = serde_json::from_value(serde_json::json!({
        "driver": "sqlite",
        "connection_string": format!("sqlite://{}", path.display()),
        "create_if_missing": true,
    }))
    .unwrap();
    assert!(matches!(config.driver, DriverType::Sqlite));
    let marker = "seaquel_log_marker_7731";
    // No sleep function: a recursive count that takes over a second.
    let slow = format!(
        "WITH RECURSIVE c(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM c WHERE n < 8000000) \
         SELECT count(*), '{marker}' AS m FROM c"
    );
    run_no_statement_logging(
        &*seaquel_engine_sqlite::engine(),
        &config,
        &slow,
        &format!("SELECT '{marker}' AS m"),
        marker,
    )
    .await;
    let _ = std::fs::remove_dir_all(&dir);
}
