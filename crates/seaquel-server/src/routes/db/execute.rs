use axum::{extract::State, Json};
use seaquel_types::{ExecuteResult, Value};
use serde::Deserialize;

use crate::{error::ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct ExecuteRequest {
    pub connection_id: String,
    pub sql: String,
    #[serde(default)]
    pub values: Vec<Value>,
}

pub async fn execute(
    State(state): State<AppState>,
    Json(req): Json<ExecuteRequest>,
) -> Result<Json<ExecuteResult>, ApiError> {
    Ok(Json(state.core.execute(&req.connection_id, &req.sql, req.values).await?))
}
