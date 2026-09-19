use axum::{extract::State, Json};
use log::debug;
use serde::Deserialize;
use seaquel_db::QueryResult;

use crate::{error::ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct QueryRequest {
    pub connection_id: String,
    pub sql: String,
    #[serde(default)]
    pub values: Vec<serde_json::Value>,
}

pub async fn query(
    State(state): State<AppState>,
    Json(req): Json<QueryRequest>,
) -> Result<Json<QueryResult>, ApiError> {
    debug!(
        activity = "db.query",
        connection_id = req.connection_id.as_str(),
        sql_len = req.sql.len(),
        params = req.values.len();
        "Query"
    );

    let driver = state.connection_manager.get_driver(&req.connection_id).await?;
    let result = driver.query(&req.sql, req.values).await?;
    Ok(Json(result))
}
