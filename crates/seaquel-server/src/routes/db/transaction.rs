use axum::{extract::State, Json};
use seaquel_types::BatchStatement;
use serde::Deserialize;

use crate::{error::ApiError, AppState};

#[derive(Debug, Deserialize)]
pub struct TransactionRequest {
    pub connection_id: String,
    pub statements: Vec<BatchStatement>,
}

pub async fn transaction(
    State(state): State<AppState>,
    Json(req): Json<TransactionRequest>,
) -> Result<(), ApiError> {
    state.core.transaction(&req.connection_id, req.statements).await?;
    Ok(())
}
