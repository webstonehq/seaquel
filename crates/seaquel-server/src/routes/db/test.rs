use axum::{extract::State, Json};
use seaquel_types::ConnectConfig;

use crate::{error::ApiError, web_config::check_connect_config, AppState};

/// Validate that the given config can open a connection. Opens the driver,
/// closes it immediately, returns `()`. Mirrors the Tauri `db_test` command.
/// Probes never leave a connection behind.
pub async fn test(
    State(state): State<AppState>,
    Json(config): Json<ConnectConfig>,
) -> Result<(), ApiError> {
    check_connect_config(&config)?;
    state.core.test(&config).await?;
    Ok(())
}
