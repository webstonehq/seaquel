//! The HTTP status for an error code on `/rpc`.
//!
//! The body always carries the code (`RpcError` JSON), which is what the
//! client reads; the status only keeps logs and proxies honest.

use axum::http::StatusCode;
use seaquel_rpc::{INVALID_ARGUMENT, NOT_SUPPORTED};

pub fn status_for(code: &str) -> StatusCode {
    match code {
        // A missing or unsafe header, a bad body, a refused option or
        // engine (SQLite and DuckDB on web), a query the
        // database refused.
        INVALID_ARGUMENT
        | "ENGINE_NOT_AVAILABLE"
        | "CONNECTION_OPTION_NOT_ALLOWED"
        | "CREDENTIALS_REQUIRED"
        | "INVALID_CONNECTION"
        | "QUERY_ERROR"
        | "EXECUTE_ERROR"
        // The edits service (phase 5c): an edit Core won't build (no such
        // table, no primary key, a key that isn't the primary key). Its
        // limits (`WEB_EDIT_LIMITS`) refuse with `INVALID_ARGUMENT`.
        | "NOT_EDITABLE"
        // The assistant (phase 6): Core's own refusals of a
        // model call, a turn on a connection opened for another saved
        // connection, and a model that asked for too many tool calls (a
        // turn's ending; never an HTTP answer, mapped for completeness).
        | "NO_PROVIDER"
        | "NO_MODEL"
        | "NO_API_KEY"
        | "AI_DISABLED"
        | "CONNECTION_MISMATCH"
        | "TOOL_LIMIT"
        // A read of a secret the web workspace has nowhere to keep (it has no store):
        // the client must supply it.
        | "NO_SECRET_STORE" => StatusCode::BAD_REQUEST,
        // Secrets (the web workspace has no store), SSH tunnels, and calls
        // an engine has no Rust implementation for.
        NOT_SUPPORTED => StatusCode::NOT_IMPLEMENTED,
        // Not this user's, or not open: the same answer either way. The
        // library's rows (phase 5d-1) too: another user's id is simply not
        // in this user's file. (`SAVED_CONNECTION_NOT_FOUND`'s wire code is
        // `CONNECTION_NOT_FOUND`.) Phase 5d-2 adds dashboards, saved
        // workflows, chats, user themes and AI providers, and a dashboard's
        // version (`dashboardVersionGet`).
        "CONNECTION_NOT_FOUND"
        | "PROJECT_NOT_FOUND"
        | "SAVED_QUERY_NOT_FOUND"
        | "LABEL_NOT_FOUND"
        | "DASHBOARD_NOT_FOUND"
        | "DASHBOARD_VERSION_NOT_FOUND"
        | "WORKFLOW_NOT_FOUND"
        | "CHAT_NOT_FOUND"
        | "THEME_NOT_FOUND"
        | "AI_PROVIDER_NOT_FOUND"
        // `ai.respond` for a turn or call the workspace isn't waiting on
        // (another user's, a finished one, a closed socket's).
        | "NOT_FOUND" => StatusCode::NOT_FOUND,
        // The database refused the login (not the app's session: that's
        // Node's 401).
        "AUTH_ERROR" => StatusCode::BAD_REQUEST,
        // The model provider refused or failed (phase 6): its status, its
        // error event, or a reply that can't be read.
        "CONNECTION_ERROR" | "TLS_ERROR" | "PROVIDER_ERROR" => StatusCode::BAD_GATEWAY,
        // Model calls are off on this server (`SEAQUEL_AI_EGRESS=off`), or
        // the provider's address isn't one it may reach.
        "AI_EGRESS_BLOCKED" => StatusCode::SERVICE_UNAVAILABLE,
        "TIMEOUT" => StatusCode::GATEWAY_TIMEOUT,
        // A user's message to the assistant past the web's cap.
        "RESULT_TOO_LARGE" | "MESSAGE_TOO_LONG" => StatusCode::PAYLOAD_TOO_LARGE,
        // A transaction statement matched fewer rows than it expected (a
        // stale key); the transaction was rolled back. Or the workspace was
        // evicted while this call ran. Or the call needs the user's
        // confirmation first (`db.applyChanges` answers that as an outcome,
        // 200; the code is mapped for any call that refuses with it).
        // Or a transaction opened by hand is already open on the
        // connection (`TRANSACTION_OPEN`, SQL Server and DuckDB).
        //
        // The library (phase 5d-1): a name another row of the scope has
        // (`NAME_TAKEN`), the last project (`LAST_PROJECT`), or a write to a
        // workspace opened read-only (`STORAGE_READ_ONLY`; the web's never
        // is).
        "NO_ROWS_AFFECTED"
        | "WORKSPACE_CLOSED"
        | "CONFIRM_REQUIRED"
        | "TRANSACTION_OPEN"
        | "NAME_TAKEN"
        | "LAST_PROJECT"
        | "STORAGE_READ_ONLY"
        // The assistant (phase 6): the chat can't take another turn
        // (`CHAT_FULL`), or one is already running on it.
        | "CHAT_FULL"
        | "TURN_IN_PROGRESS"
        | "AI_PROVIDER_CHANGED" => StatusCode::CONFLICT,
        // The user holds as many connections as the web server allows
        // (`WEB_CONNECTION_LIMITS`), or has as many calls in flight
        // (`MAX_IN_FLIGHT_BYTES_PER_USER`, `MAX_EDIT_CALLS_PER_USER`).
        // Also past `WEB_AI_LIMITS`' turns in flight, and the provider's own
        // rate limit (`RATE_LIMITED`).
        seaquel_core::TOO_MANY_CONNECTIONS
        | crate::routes::rpc::TOO_MANY_REQUESTS
        | "RATE_LIMITED" => StatusCode::TOO_MANY_REQUESTS,
        // The user's metadata file reached its size cap
        // (`SEAQUEL_USER_DB_MAX_BYTES`, phase 5d-2): nothing was written.
        "STORAGE_FULL" => StatusCode::INSUFFICIENT_STORAGE,
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
        assert_eq!(status_for("CONFIRM_REQUIRED"), StatusCode::CONFLICT);
        assert_eq!(status_for("TRANSACTION_OPEN"), StatusCode::CONFLICT);
        assert_eq!(status_for("NOT_EDITABLE"), StatusCode::BAD_REQUEST);
        assert_eq!(
            status_for("TOO_MANY_CONNECTIONS"),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status_for("TOO_MANY_REQUESTS"),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(status_for("AUTH_ERROR"), StatusCode::BAD_REQUEST);
        // The web workspace has no secret store: a client's
        // problem, never a server fault.
        assert_eq!(status_for("NO_SECRET_STORE"), StatusCode::BAD_REQUEST);
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
        for code in ["NAME_TAKEN", "LAST_PROJECT", "STORAGE_READ_ONLY"] {
            assert_eq!(status_for(code), StatusCode::CONFLICT, "{code}");
        }
        for code in [
            "PROJECT_NOT_FOUND",
            "SAVED_QUERY_NOT_FOUND",
            "LABEL_NOT_FOUND",
            "DASHBOARD_NOT_FOUND",
            "DASHBOARD_VERSION_NOT_FOUND",
            "WORKFLOW_NOT_FOUND",
            "CHAT_NOT_FOUND",
            "THEME_NOT_FOUND",
            "AI_PROVIDER_NOT_FOUND",
        ] {
            assert_eq!(status_for(code), StatusCode::NOT_FOUND, "{code}");
        }
        assert_eq!(status_for("STORAGE_FULL"), StatusCode::INSUFFICIENT_STORAGE);
        // The assistant (phase 6).
        for (code, status) in [
            ("NO_PROVIDER", StatusCode::BAD_REQUEST),
            ("NO_MODEL", StatusCode::BAD_REQUEST),
            ("NO_API_KEY", StatusCode::BAD_REQUEST),
            ("AI_DISABLED", StatusCode::BAD_REQUEST),
            ("CONNECTION_MISMATCH", StatusCode::BAD_REQUEST),
            ("TOOL_LIMIT", StatusCode::BAD_REQUEST),
            ("AI_EGRESS_BLOCKED", StatusCode::SERVICE_UNAVAILABLE),
            ("PROVIDER_ERROR", StatusCode::BAD_GATEWAY),
            ("RATE_LIMITED", StatusCode::TOO_MANY_REQUESTS),
            ("TOO_MANY_REQUESTS", StatusCode::TOO_MANY_REQUESTS),
            ("CHAT_FULL", StatusCode::CONFLICT),
            ("TURN_IN_PROGRESS", StatusCode::CONFLICT),
            ("AI_PROVIDER_CHANGED", StatusCode::CONFLICT),
            ("NOT_FOUND", StatusCode::NOT_FOUND),
            ("TIMEOUT", StatusCode::GATEWAY_TIMEOUT),
            // A user's message past the web's cap (F1/F2/F5 review P1).
            ("MESSAGE_TOO_LONG", StatusCode::PAYLOAD_TOO_LARGE),
        ] {
            assert_eq!(status_for(code), status, "{code}");
        }
        for code in ["STORAGE_ERROR", "STORAGE_CORRUPT", "SOMETHING_ELSE"] {
            assert_eq!(status_for(code), StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
}
