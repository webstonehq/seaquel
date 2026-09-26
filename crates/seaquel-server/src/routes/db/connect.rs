use axum::{extract::State, Json};
use seaquel_types::{ConnectConfig, ConnectResult};

use crate::{error::ApiError, web_config::check_connect_config, AppState};

pub async fn connect(
    State(state): State<AppState>,
    Json(config): Json<ConnectConfig>,
) -> Result<Json<ConnectResult>, ApiError> {
    // No server files or sockets (Decision 11b); Core refuses other engines.
    check_connect_config(&config)?;
    Ok(Json(state.core.connect(&config).await?))
}
