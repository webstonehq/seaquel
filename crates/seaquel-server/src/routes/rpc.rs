//! `POST /rpc`: one workspace call (`seaquel_rpc::Request`) for the user in
//! `X-Seaquel-User`: storage (the history, shared repos, the license and
//! the vault), the library (connections, projects, labels, saved queries,
//! dashboards, workflows, chats), settings, each tab's view state (`ui`),
//! and `db` (connect, test, disconnect, query,
//! execute, transaction, engine, cancel, and the edits service's
//! planEdits, applyChanges and duckdbExtension) on the user's own
//! connections.
//! Core refuses a connection or stream the workspace doesn't own with
//! `CONNECTION_NOT_FOUND`, and connects under [`crate::web_connect_policy`].
//! The `ai` group (phase 6): `respond`, `generate`, `models` and `test`;
//! the key the page decrypted from the vault comes with each call
//! (`apiKey`) and is never logged. `db.queryStream`, `db.run`, `db.page`,
//! `db.tablePage` and `ai.chat` are served by `/rpc/stream`.
//!
//! Only Node's `/api/rpc` route calls this. It sets the header from the
//! session and drops any copy the browser sent, so the header is trusted
//! here; that trust is why the server must stay on loopback (`main.rs`).
//!
//! `X-Seaquel-Origin` (phase 5d, Decision 18) names the browser tab that
//! sent the call; the `StorageChanged` event of a write carries it, so that
//! tab can skip its own change. Node forwards it only when it matches
//! `^[A-Za-z0-9_-]{1,64}$`, and it's checked again here: one that's missing,
//! repeated or malformed is ignored (the call runs with no origin), never
//! refused, and never logged. It isn't a security boundary: a lying origin
//! can only hide a change from that user's own tab. The `ui` group
//! (phase 5d-2) also takes it as the calling tab's window id: a view-state
//! call naming any other window, or sent without the header, is refused by
//! Core with `INVALID_ARGUMENT`, so one tab can't read or overwrite
//! another's tabs.
//!
//! The body goes to `parse_request` as the bytes that came in, never through
//! a `serde_json::Value`: stored JSON columns keep their exact text, and
//! `method` must come before `params`.
//!
//! Errors are `RpcError` JSON (`{code, message}`) with a status from the
//! code (`crate::error::status_for`): `INVALID_ARGUMENT` 400 (a missing or
//! unsafe header, a bad body), `NOT_SUPPORTED` 501 (secrets: the web
//! workspace has no store; SSH tunnels), `CONNECTION_NOT_FOUND` 404, the
//! refused engines and options 400, an edit Core won't build
//! (`NOT_EDITABLE`) 400, the library's `NAME_TAKEN` and `LAST_PROJECT` 409
//! and `PROJECT_NOT_FOUND`, `SAVED_QUERY_NOT_FOUND`, `LABEL_NOT_FOUND`,
//! `DASHBOARD_NOT_FOUND`, `DASHBOARD_VERSION_NOT_FOUND`,
//! `WORKFLOW_NOT_FOUND`, `CHAT_NOT_FOUND`,
//! `THEME_NOT_FOUND` and `AI_PROVIDER_NOT_FOUND` 404, the assistant's codes
//! (`AI_EGRESS_BLOCKED` 503, `PROVIDER_ERROR` 502, `RATE_LIMITED` and
//! `TOO_MANY_REQUESTS` 429, `CHAT_FULL` and `TURN_IN_PROGRESS` 409,
//! `NOT_FOUND` 404, Core's refusals 400), `STORAGE_FULL` 507
//! (the user's file reached its cap), and the storage codes 500
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
use seaquel_rpc::{dispatch_workspace, parse_request, RpcError, WriteOrigin};

use std::path::Path;
use std::sync::Arc;

use crate::workspaces::{GetError, OpenWorkspace};
use crate::AppState;

/// The header Node sets to the session's user id.
pub const USER_HEADER: &str = "x-seaquel-user";

/// The header Node forwards with the calling tab's origin id, after its own
/// format check (see the module docs).
pub const ORIGIN_HEADER: &str = "x-seaquel-origin";

/// The largest body `/rpc` takes. Node caps `/api/rpc` bodies first, at
/// 20 MiB (`RPC_BODY_LIMIT` in `src/lib/server/body-limit.ts`, above
/// `WEB_EDIT_LIMITS`' 16 MiB of values and 2 MiB of SQL); this is the outer
/// bound.
pub const BODY_LIMIT: usize = 64 * 1024 * 1024;

/// How many bytes of bodies one user's `/rpc` calls in flight may hold
/// together (probe review, M4); a call past it is refused with
/// [`TOO_MANY_REQUESTS`] (429) before it's parsed. A lone call always runs,
/// so a body up to [`BODY_LIMIT`] still fits. The GUI's everyday calls
/// (loading its state, many at once) are tiny next to it.
pub const MAX_IN_FLIGHT_BYTES_PER_USER: usize = 40 * 1024 * 1024;

/// A body under this always gets past [`MAX_IN_FLIGHT_BYTES_PER_USER`]
/// (it still counts toward it), so a user's own large applies never make
/// their storage saves and history appends fail with 429.
pub const SMALL_CALL_BYTES: usize = 64 * 1024;

/// How many slow calls one user runs at once, whatever their size: the
/// edit calls (`db.applyChanges`, `db.planEdits`, `db.duckdbExtension`),
/// each holding many times its body in memory or a database connection,
/// and (phase 6) the unary model calls (`ai.generate`, `ai.models`,
/// `ai.test`), each holding a request to the provider for up to 10 minutes.
/// One pool for both. The next is refused with [`TOO_MANY_REQUESTS`] (429)
/// before it runs. A turn (`ai.chat`) is a stream, under Core's own cap
/// (`WEB_AI_LIMITS`).
pub const MAX_EDIT_CALLS_PER_USER: usize = 4;

/// The code for a call refused by [`MAX_IN_FLIGHT_BYTES_PER_USER`] or
/// [`MAX_EDIT_CALLS_PER_USER`]. Nothing ran.
pub const TOO_MANY_REQUESTS: &str = "TOO_MANY_REQUESTS";

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
    RpcError { message, ..e }
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
    let mut slot = state
        .workspaces
        .begin_call(user, body.len(), MAX_IN_FLIGHT_BYTES_PER_USER)
        .ok_or_else(|| {
            RpcError::new(
                TOO_MANY_REQUESTS,
                "Too many large requests are running at once. Wait for them to finish \
                 and try again.",
            )
        })?;
    // Parse before opening anything, so a bad body never creates a file.
    let request = parse_request(body)?;
    *method = Some((request.group(), request.method()));
    if is_slow_call(&request) && !slot.begin_edit(MAX_EDIT_CALLS_PER_USER) {
        return Err(RpcError::new(
            TOO_MANY_REQUESTS,
            format!(
                "At most {MAX_EDIT_CALLS_PER_USER} edit or AI requests can run at once. \
                 Wait for one to finish and try again."
            ),
        ));
    }
    let open = open_workspace(state, user).await?;
    let response = dispatch_workspace(
        &state.core,
        open.workspace(),
        request,
        write_origin(headers),
    )
    .await?;
    serde_json::to_vec(&response).map_err(|e| {
        RpcError::new(
            "INTERNAL_ERROR",
            format!("couldn't encode the response: {e}"),
        )
    })
}

/// The calling tab's origin from [`ORIGIN_HEADER`]: none when the header
/// is missing, repeated, not ASCII, or not 1–64 of `[A-Za-z0-9_-]`
/// (`WriteOrigin::new` checks the format).
pub(crate) fn write_origin(headers: &HeaderMap) -> WriteOrigin {
    let mut values = headers.get_all(ORIGIN_HEADER).iter();
    match (values.next(), values.next()) {
        (Some(value), None) => WriteOrigin::new(value.to_str().ok()),
        _ => WriteOrigin::none(),
    }
}

/// A call [`MAX_EDIT_CALLS_PER_USER`] counts: the edit calls and the unary
/// model calls. Library, settings and `ui` calls don't count (phase 5d,
/// Decisions 15 and 27): they're single-row writes and reads, bounded by
/// the web limits; nor does `ai.respond`, which only hands a turn its
/// answer. Like every call they count toward
/// [`MAX_IN_FLIGHT_BYTES_PER_USER`] and the body limits.
fn is_slow_call(request: &seaquel_rpc::Request) -> bool {
    matches!(
        (request.group(), request.method()),
        ("db", "applyChanges" | "planEdits" | "duckdbExtension")
            | ("ai", "generate" | "models" | "test")
    )
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
    fn a_bad_origin_header_is_ignored() {
        let origin = |values: &[&[u8]]| {
            let mut headers = HeaderMap::new();
            for v in values {
                headers.append(
                    ORIGIN_HEADER,
                    axum::http::HeaderValue::from_bytes(v).unwrap(),
                );
            }
            write_origin(&headers).as_deref().map(str::to_string)
        };
        assert_eq!(origin(&[b"tab-1_A"]), Some("tab-1_A".into()));
        assert_eq!(origin(&[]), None);
        assert_eq!(origin(&[b""]), None);
        assert_eq!(origin(&[b"a b"]), None);
        assert_eq!(origin(&[b"a/b"]), None);
        assert_eq!(origin(&["caf\u{e9}".as_bytes()]), None);
        assert_eq!(origin(&["x".repeat(65).as_bytes()]), None);
        assert_eq!(origin(&[b"a", b"b"]), None, "repeated");
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
