//! The HTTP status for an error code on `/rpc`.
//!
//! The body always carries the code (`RpcError` JSON), which is what the
//! client reads; the status only keeps logs and proxies honest.

use axum::http::StatusCode;
use seaquel_rpc::{INVALID_ARGUMENT, NOT_SUPPORTED};

pub fn status_for(code: &str) -> StatusCode {
    match code {
        // A missing or unsafe header, a bad body, a refused option or
        // engine (SQLite and DuckDB on web, Decision 11b), a query the
        // database refused.
        INVALID_ARGUMENT
        | "ENGINE_NOT_AVAILABLE"
        | "CONNECTION_OPTION_NOT_ALLOWED"
        | "CREDENTIALS_REQUIRED"
        | "INVALID_CONNECTION"
        | "QUERY_ERROR"
        | "EXECUTE_ERROR" => StatusCode::BAD_REQUEST,
        // Secrets (the web workspace has no store), SSH tunnels, and calls
        // an engine has no Rust implementation for.
        NOT_SUPPORTED => StatusCode::NOT_IMPLEMENTED,
        // Not this user's, or not open: the same answer either way.
        "CONNECTION_NOT_FOUND" => StatusCode::NOT_FOUND,
        // The database refused the login (not the app's session: that's
        // Node's 401).
        "AUTH_ERROR" => StatusCode::BAD_REQUEST,
        "CONNECTION_ERROR" | "TLS_ERROR" => StatusCode::BAD_GATEWAY,
        "TIMEOUT" => StatusCode::GATEWAY_TIMEOUT,
        "RESULT_TOO_LARGE" => StatusCode::PAYLOAD_TOO_LARGE,
        // A transaction statement matched fewer rows than it expected (a
        // stale key); the transaction was rolled back. Or the workspace was
        // evicted while this call ran.
        "NO_ROWS_AFFECTED" | "WORKSPACE_CLOSED" => StatusCode::CONFLICT,
        // The user holds as many connections as the web server allows
        // (`WEB_CONNECTION_LIMITS`).
        seaquel_core::TOO_MANY_CONNECTIONS => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_error_codes_to_statuses() {
        assert_eq!(status_for("INVALID_ARGUMENT"), StatusCode::BAD_REQUEST);
        assert_eq!(status_for("CONNECTION_NOT_FOUND"), StatusCode::NOT_FOUND);
        assert_eq!(status_for("CONNECTION_ERROR"), StatusCode::BAD_GATEWAY);
        assert_eq!(status_for("QUERY_ERROR"), StatusCode::BAD_REQUEST);
        assert_eq!(status_for("NOT_SUPPORTED"), StatusCode::NOT_IMPLEMENTED);
        assert_eq!(status_for("NO_ROWS_AFFECTED"), StatusCode::CONFLICT);
        assert_eq!(status_for("WORKSPACE_CLOSED"), StatusCode::CONFLICT);
        assert_eq!(
            status_for("TOO_MANY_CONNECTIONS"),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(status_for("AUTH_ERROR"), StatusCode::BAD_REQUEST);
        assert_eq!(status_for("TLS_ERROR"), StatusCode::BAD_GATEWAY);
        assert_eq!(status_for("TIMEOUT"), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            status_for("RESULT_TOO_LARGE"),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(status_for("ENGINE_NOT_AVAILABLE"), StatusCode::BAD_REQUEST);
        assert_eq!(
            status_for("CONNECTION_OPTION_NOT_ALLOWED"),
            StatusCode::BAD_REQUEST
        );
        for code in ["STORAGE_ERROR", "STORAGE_CORRUPT", "SOMETHING_ELSE"] {
            assert_eq!(status_for(code), StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
}
