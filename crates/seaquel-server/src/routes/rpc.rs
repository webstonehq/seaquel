//! `POST /rpc`: one workspace call (`seaquel_rpc::Request`) for the user in
//! `X-Seaquel-User`: storage, and `db` (connect, test, disconnect, query,
//! execute, transaction, engine, cancel) on the user's own connections.
//! Core refuses a connection or stream the workspace doesn't own with
//! `CONNECTION_NOT_FOUND`, and connects under [`crate::web_connect_policy`].
//! `db.queryStream` is served by `/rpc/stream`.
//!
//! Only Node's `/api/rpc` route calls this. It sets the header from the
//! session and drops any copy the browser sent, so the header is trusted
//! here; that trust is why the server must stay on loopback (`main.rs`).
//!
//! The body goes to `parse_request` as the bytes that came in, never through
//! a `serde_json::Value`: stored JSON columns keep their exact text, and
//! `method` must come before `params`.
//!
//! Errors are `RpcError` JSON (`{code, message}`) with a status from the
//! code (`crate::error::status_for`): `INVALID_ARGUMENT` 400 (a missing or
//! unsafe header, a bad body), `NOT_SUPPORTED` 501 (secrets: the web
//! workspace has no store; SSH tunnels), `CONNECTION_NOT_FOUND` 404, the
//! refused engines and options 400, and the storage codes 500
//! (`STORAGE_ERROR`, `STORAGE_CORRUPT`, `LEGACY_STORAGE`, `NO_DATA_DIR`).
//!
//! A failure is logged by its code and the request's group and method only:
//! the message can quote SQL and values (a database's syntax error, a parse
//! error naming what it read). The copy sent to the browser keeps its code,
//! but the data root in its message becomes `DATA_DIR`, so it never shows
//! where the server keeps its files.

use axum::{
    body::Bytes,
    extract::State,
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
    Json,
};
use seaquel_rpc::{dispatch_workspace, parse_request, RpcError};

use std::path::Path;
use std::sync::Arc;

use crate::workspaces::{GetError, OpenWorkspace};
use crate::AppState;

/// The header Node sets to the session's user id.
pub const USER_HEADER: &str = "x-seaquel-user";

/// The largest body `/rpc` takes. Node's adapter caps request bodies first
/// (`BODY_SIZE_LIMIT`, 512 KB unless a self-hoster raises it); this only
/// keeps Rust from being the tighter limit.
pub const BODY_LIMIT: usize = 64 * 1024 * 1024;

pub async fn rpc(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let mut method = None;
    match call(&state, &headers, &body, &mut method).await {
        Ok(json) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        Err(e) => {
            // Code and method only: the message can quote SQL and values.
            let (group, method) = method.unwrap_or(("-", "-"));
            log::warn!(activity = "rpc.error", code = e.code.as_str(), group = group, method = method; "/rpc failed");
            error_response(redact(e, state.workspaces.root()))
        }
    }
}

/// The placeholder for the data root in messages sent to the browser.
pub const DATA_DIR_PLACEHOLDER: &str = "DATA_DIR";

/// `e` with the absolute spellings of `root` in its message (as given,
/// made absolute, and canonical, which differ under symlinks like macOS's
/// `/var`) replaced by [`DATA_DIR_PLACEHOLDER`]. The code is kept. A
/// relative spelling is left alone: it says nothing about the server's
/// layout, and replacing a short word like `data` would mangle other text.
pub fn redact(e: RpcError, root: &Path) -> RpcError {
    let mut spellings: Vec<String> = Vec::new();
    for form in [
        std::fs::canonicalize(root).ok(),
        std::path::absolute(root).ok(),
        Some(root.to_path_buf()),
    ]
    .into_iter()
    .flatten()
    {
        if !form.is_absolute() {
            continue;
        }
        let text = form.display().to_string();
        let text = text.trim_end_matches(['/', '\\']).to_string();
        // `/` trims to nothing; replacing it would mangle every path.
        if !text.is_empty() && !spellings.contains(&text) {
            spellings.push(text);
        }
    }
    // Longest first, so `/private/var/x` goes before `/var/x`.
    spellings.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut message = e.message;
    for s in &spellings {
        message = message.replace(s.as_str(), DATA_DIR_PLACEHOLDER);
    }
    RpcError::new(e.code, message)
}

/// Serve one call. `method` is set to the request's group and method once
/// it parses, for the error log.
async fn call(
    state: &AppState,
    headers: &HeaderMap,
    body: &[u8],
    method: &mut Option<(&'static str, &'static str)>,
) -> Result<Vec<u8>, RpcError> {
    let user = user_id(headers)?;
    // Parse before opening anything, so a bad body never creates a file.
    let request = parse_request(body)?;
    *method = Some((request.group(), request.method()));
    let open = open_workspace(state, user).await?;
    let response = dispatch_workspace(&state.core, open.workspace(), request).await?;
    serde_json::to_vec(&response).map_err(|e| {
        RpcError::new(
            "INTERNAL_ERROR",
            format!("couldn't encode the response: {e}"),
        )
    })
}

/// `user`'s workspace from the LRU, opening it if needed.
pub(crate) async fn open_workspace(
    state: &AppState,
    user: &str,
) -> Result<Arc<OpenWorkspace>, RpcError> {
    state
        .workspaces
        .get(&state.core, user)
        .await
        .map_err(|e| match e {
            GetError::InvalidUser(e) => RpcError::invalid_argument(e.message()),
            GetError::Open(e) => RpcError::from(e),
        })
}

pub(crate) fn user_id(headers: &HeaderMap) -> Result<&str, RpcError> {
    let mut values = headers.get_all(USER_HEADER).iter();
    let value = values
        .next()
        .ok_or_else(|| RpcError::invalid_argument("the X-Seaquel-User header is missing"))?;
    if values.next().is_some() {
        return Err(RpcError::invalid_argument(
            "the X-Seaquel-User header is repeated",
        ));
    }
    let id = value.to_str().map_err(|_| {
        RpcError::invalid_argument("the X-Seaquel-User header isn't a safe user id")
    })?;
    crate::workspaces::validate_user_id(id).map_err(|e| RpcError::invalid_argument(e.message()))?;
    Ok(id)
}

pub(crate) fn error_response(e: RpcError) -> Response {
    (crate::error::status_for(&e.code), Json(e)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_replaces_the_data_root_and_keeps_the_code() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let canonical = std::fs::canonicalize(root).unwrap();
        let e = RpcError::new(
            "STORAGE_CORRUPT",
            format!(
                "{}/users/u1/meta.db isn't a SQLite database (also {}/users/u1/meta.db)",
                root.display(),
                canonical.display()
            ),
        );
        let out = redact(e, root);
        assert_eq!(out.code, "STORAGE_CORRUPT");
        assert_eq!(
            out.message,
            "DATA_DIR/users/u1/meta.db isn't a SQLite database (also DATA_DIR/users/u1/meta.db)"
        );
    }

    #[test]
    fn redact_handles_a_relative_root() {
        let abs = std::path::absolute("data").unwrap();
        let e = RpcError::new(
            "STORAGE_ERROR",
            format!(
                "metadata: data/users/u1/meta.db and {}/users/u1/meta.db",
                abs.display()
            ),
        );
        assert_eq!(
            redact(e, Path::new("data")).message,
            "metadata: data/users/u1/meta.db and DATA_DIR/users/u1/meta.db"
        );
    }

    #[test]
    fn redact_leaves_other_messages_alone() {
        let e = RpcError::new("INVALID_ARGUMENT", "invalid request: expected value");
        assert_eq!(
            redact(e, Path::new("/data")).message,
            "invalid request: expected value"
        );
    }
}
