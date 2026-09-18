use axum::{extract::State, Json};
use log::info;
use seaquel_db::{ConnectConfig, ConnectResult};

use crate::{error::ApiError, AppState};

pub async fn connect(
    State(state): State<AppState>,
    Json(config): Json<ConnectConfig>,
) -> Result<Json<ConnectResult>, ApiError> {
    let driver_name = format!("{:?}", config.driver);
    info!(activity = "db.connect", driver = driver_name.as_str(); "Connecting");

    let connection_id = format!(
        "{}-{}",
        driver_name.to_lowercase(),
        uuid::Uuid::new_v4()
    );

    let driver = seaquel_db::open(&config).await?;

    state
        .connection_manager
        .connections
        .write()
        .await
        .insert(connection_id.clone(), driver);

    info!(
        activity = "db.connect",
        driver = driver_name.as_str(),
        connection_id = connection_id.as_str();
        "Connected"
    );
    Ok(Json(ConnectResult { connection_id }))
}
