use axum::{extract::State, Json};
use serde::Deserialize;

use crate::{error::ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct DisconnectRequest {
    pub connection_id: String,
}

/// Close a connection. Idempotent — disconnecting an unknown id succeeds
/// silently, matching the Tauri `db_disconnect` command's behavior.
pub async fn disconnect(
    State(state): State<AppState>,
    Json(req): Json<DisconnectRequest>,
) -> Result<(), ApiError> {
    state.core.disconnect(&req.connection_id).await?;
    Ok(())
}
