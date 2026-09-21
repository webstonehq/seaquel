//! HTTP error mapping for `seaquel_db::DbError`.
//!
//! The database layer is platform-agnostic (no axum dependency), so this
//! wrapper lives in the server crate. Handlers return `Result<T, ApiError>`
//! and `ApiError` turns itself into a JSON error response.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use seaquel_db::DbError;

pub struct ApiError(pub DbError);

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self.0.code.as_str() {
            "CONNECTION_NOT_FOUND" => StatusCode::NOT_FOUND,
            "CONNECTION_ERROR" => StatusCode::BAD_GATEWAY,
            "QUERY_ERROR" | "EXECUTE_ERROR" | "FILE_NOT_FOUND" => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}
