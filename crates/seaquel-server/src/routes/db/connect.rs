use axum::{extract::State, Json};
use seaquel_types::{ConnectConfig, ConnectResult};

use crate::{error::ApiError, AppState};

pub async fn connect(
    State(state): State<AppState>,
    Json(config): Json<ConnectConfig>,
) -> Result<Json<ConnectResult>, ApiError> {
    Ok(Json(state.core.connect(&config).await?))
}
