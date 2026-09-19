use axum::{extract::State, Json};
use log::debug;
use serde::Deserialize;
use seaquel_db::BatchStatement;

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
    debug!(
        activity = "db.transaction",
        connection_id = req.connection_id.as_str(),
        statements = req.statements.len();
        "Executing transaction"
    );

    let driver = state.connection_manager.get_driver(&req.connection_id).await?;
    driver.transaction(req.statements).await?;
    Ok(())
}
