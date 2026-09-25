use axum::{extract::State, Json};
use seaquel_types::{QueryResult, Value};
use serde::Deserialize;

use crate::{error::ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub connection_id: String,
    pub sql: String,
    #[serde(default)]
    pub values: Vec<Value>,
}

pub async fn query(
    State(state): State<AppState>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<QueryResult>, ApiError> {
    Ok(Json(state.core.query(&req.connection_id, &req.sql, req.values).await?))
}
