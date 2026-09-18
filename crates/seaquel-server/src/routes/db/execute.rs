use axum::{extract::State, Json};
use log::debug;
use serde::Deserialize;
use seaquel_db::ExecuteResult;

use crate::{error::ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    pub connection_id: String,
    pub sql: String,
    #[serde(default)]
    pub values: Vec<serde_json::Value>,
}

pub async fn execute(
    State(state): State<AppState>,
    Json(req): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResult>, ApiError> {
    debug!(
        activity = "db.execute",
        connection_id = req.connection_id.as_str(),
        sql_len = req.sql.len(),
        params = req.values.len();
        "Execute"
    );

    let driver = state.connection_manager.get_driver(&req.connection_id).await?;
    let result = driver.execute(&req.sql, req.values).await?;
    Ok(Json(result))
}
