//! HTTP error mapping for `seaquel_types::DbError`.
//!
//! The database layer is platform-agnostic (no axum dependency), so this
//! wrapper lives in the server crate. Handlers return `Result<T, ApiError>`
//! and `ApiError` turns itself into a JSON error response.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use seaquel_types::DbError;

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
            // The engine has no Rust dialect or introspection yet; the client
            // falls back to its TypeScript adapter.
            "NOT_SUPPORTED" => StatusCode::NOT_IMPLEMENTED,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(self.0)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status_of(code: &str) -> StatusCode {
        ApiError(DbError {
            message: "m".into(),
            code: code.into(),
        })
        .into_response()
        .status()
    }

    #[test]
    fn maps_error_codes_to_statuses() {
        assert_eq!(status_of("CONNECTION_NOT_FOUND"), StatusCode::NOT_FOUND);
        assert_eq!(status_of("CONNECTION_ERROR"), StatusCode::BAD_GATEWAY);
        assert_eq!(status_of("QUERY_ERROR"), StatusCode::BAD_REQUEST);
        assert_eq!(status_of("NOT_SUPPORTED"), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(
            status_of("SOMETHING_ELSE"),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
