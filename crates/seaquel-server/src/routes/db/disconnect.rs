use axum::{extract::State, Json};
use log::info;
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
    info!(
        activity = "db.disconnect",
        connection_id = req.connection_id.as_str();
        "Disconnecting"
    );
    let driver = {
        let mut connections = state.connection_manager.connections.write().await;
        connections.remove(&req.connection_id)
    };
    if let Some(driver) = driver {
        // `pool.close()` waits for in-flight queries to return their
        // connections. Any stream holding an Arc clone drops it when done;
        // close() only resolves once the driver has truly shut down.
        driver.close().await?;
    }
    Ok(())
}
