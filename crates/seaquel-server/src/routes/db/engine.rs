use axum::{extract::State, Json};
use seaquel_rpc::{EngineCall, EngineResponse};

use crate::{error::ApiError, AppState};

/// `POST /api/db/engine`: one `EngineCall` (introspection, EXPLAIN or SQL
/// generation) on a connection. `NOT_SUPPORTED` (501) means the connection's
/// engine still has its dialect in TypeScript.
pub async fn engine(
    State(state): State<AppState>,
    Json(call): Json<EngineCall>,
) -> Result<Json<EngineResponse>, ApiError> {
    Ok(Json(seaquel_rpc::dispatch(&state.core, call).await?))
}
