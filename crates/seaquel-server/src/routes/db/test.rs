use axum::Json;
use log::debug;
use seaquel_db::ConnectConfig;

use crate::error::ApiError;

/// Validate that the given config can open a connection. Opens the driver,
/// closes it immediately, returns `()`. Mirrors the Tauri `db_test` command.
///
/// Note: this endpoint takes no state because it never touches the
/// ConnectionManager — probes must not leave a connection behind.
pub async fn test(Json(config): Json<ConnectConfig>) -> Result<(), ApiError> {
    debug!(
        activity = "db.test",
        driver = format!("{:?}", config.driver).as_str();
        "Testing connection"
    );
    let driver = seaquel_db::open(&config).await?;
    driver.close().await?;
    Ok(())
}
