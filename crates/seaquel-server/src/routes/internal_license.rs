//! `/internal/license/*`: the web build's licensing, for the local Node
//! server only.
//!
//! Node's hooks and routes call these through `license-client.ts` in the
//! order they always ran their steps; Better Auth, session purging and the
//! redirects stay in Node. Neither `server.js` nor any SvelteKit proxy
//! forwards `/internal/*`, and these routes refuse (403):
//!
//! - any peer that isn't loopback, whatever address the server was bound
//!   to;
//! - any request without the per-boot secret in [`SECRET_HEADER`]. Loopback
//!   alone isn't enough: a query running inside this process (DuckDB's
//!   httpfs reading `http://127.0.0.1:8788/internal/...`) also arrives from
//!   loopback. `server.js` generates the secret at startup and hands it to
//!   both processes (`SEAQUEL_INTERNAL_SECRET`); a server started without
//!   one refuses every `/internal` call.
//!
//! | Route | Does |
//! | --- | --- |
//! | `GET gate?user=` | the TTL ladder, the user's membership, install facts |
//! | `GET install` | `hasTenant` and `bundlePresent`, read now |
//! | `POST register-install` `{licenseKey}` | first-owner registration and the cache write |
//! | `POST signup-check` `{licenseKey, email}` | `verifyMembershipLicense`'s `VerifyResult` |
//! | `POST bind-member` `{licenseKey, userId, email, role, firstOwner}` | upstream bind and the local row |
//! | `POST unbind-member` `{userId}` | upstream unbind and the local row |
//! | `GET members` | `MemberView[]` |
//! | `GET airgap/status?user=` | mode, TTL timestamps, the user's row, the bundle |
//! | `POST airgap/upload?user=` (raw envelope) | `{status, body, revokedUserIds}` |
//! | `POST airgap/clear?user=` | `{ok: true}` |
//!
//! `user` is the signed-in user Node's session names, left out when there
//! is none. Errors are `{code, message}` with the status from
//! `ServerErrorCode::http_status` (`NOT_READY` 503 until Node has applied
//! its auth.db migrations). License keys never reach a log line.

use std::net::SocketAddr;

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, DefaultBodyLimit, Query, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use seaquel_core::license::server::{BindMemberRequest, ServerError, ServerErrorCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::AppState;

/// The largest bundle upload. Node's adapter caps request bodies first.
const BODY_LIMIT: usize = 64 * 1024 * 1024;

/// The header Node sends the per-boot secret in.
pub const SECRET_HEADER: &str = "x-seaquel-internal";

pub fn router(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/gate", get(gate))
        .route("/install", get(install))
        .route("/register-install", post(register_install))
        .route("/signup-check", post(signup_check))
        .route("/bind-member", post(bind_member))
        .route("/unbind-member", post(unbind_member))
        .route("/members", get(members))
        .route("/airgap/status", get(airgap_status))
        .route(
            "/airgap/upload",
            post(airgap_upload).layer(DefaultBodyLimit::max(BODY_LIMIT)),
        )
        .route("/airgap/clear", post(airgap_clear))
        .layer(middleware::from_fn_with_state(state, guard))
}

/// Equal in time proportional to `b`'s length whatever `a` holds, so the
/// comparison doesn't leak how much of a guess was right.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = (a.len() != b.len()) as u8;
    for (i, y) in b.iter().enumerate() {
        diff |= a.get(i).copied().unwrap_or(0) ^ y;
    }
    diff == 0
}

/// Whether `addr` is this machine (including `::ffff:127.0.0.1`).
pub fn is_loopback_peer(addr: &SocketAddr) -> bool {
    addr.ip().to_canonical().is_loopback()
}

/// Refuse every peer that isn't loopback, any request whose peer is
/// unknown (a server started without connect info), and any request
/// without the per-boot secret.
async fn guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let refuse = || {
        error_body(
            StatusCode::FORBIDDEN,
            "FORBIDDEN",
            "/internal/* only answers the local Node server",
        )
    };
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    if !peer.is_some_and(|addr| is_loopback_peer(&addr)) {
        log::warn!(activity = "license.internal", peer = peer.map(|a| a.to_string()).unwrap_or_default().as_str(); "Refused /internal/license from a non-loopback peer");
        return refuse();
    }
    let Some(secret) = state.internal_secret.as_deref() else {
        log::warn!(activity = "license.internal"; "Refused /internal/license: this server has no SEAQUEL_INTERNAL_SECRET (server.js sets it)");
        return refuse();
    };
    let sent = req
        .headers()
        .get(SECRET_HEADER)
        .map(|v| v.as_bytes())
        .unwrap_or_default();
    if !constant_time_eq(sent, secret.as_bytes()) {
        log::warn!(activity = "license.internal"; "Refused /internal/license without the right secret");
        return refuse();
    }
    next.run(req).await
}

fn error_body(status: StatusCode, code: &str, message: &str) -> Response {
    #[derive(Serialize)]
    struct Body<'a> {
        code: &'a str,
        message: &'a str,
    }
    (status, Json(Body { code, message })).into_response()
}

fn failure(e: ServerError) -> Response {
    // Never a key in these messages (see seaquel_license::server).
    log::warn!(activity = "license.internal", code = e.code.as_str(); "/internal/license failed: {}", e.message);
    let status =
        StatusCode::from_u16(e.code.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    error_body(status, e.code.as_str(), &e.message)
}

fn ok_json(value: &impl Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => (
            [(header::CONTENT_TYPE, "application/json")],
            Body::from(bytes),
        )
            .into_response(),
        Err(e) => error_body(
            StatusCode::INTERNAL_SERVER_ERROR,
            "INTERNAL_ERROR",
            &format!("couldn't encode the answer: {e}"),
        ),
    }
}

fn answer<T: Serialize>(result: Result<T, ServerError>) -> Response {
    match result {
        Ok(v) => ok_json(&v),
        Err(e) => failure(e),
    }
}

fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, ServerError> {
    serde_json::from_slice(body).map_err(|e| {
        ServerError::new(
            ServerErrorCode::InvalidArgument,
            format!("invalid request body: {e}"),
        )
    })
}

#[derive(Deserialize, Default)]
pub struct UserParam {
    #[serde(default)]
    user: Option<String>,
}

impl UserParam {
    fn user(&self) -> Option<&str> {
        self.user.as_deref().filter(|u| !u.is_empty())
    }
}

/// A body holding a license key. No `Debug`: nothing here is logged.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyBody {
    license_key: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckBody {
    license_key: String,
    email: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserBody {
    user_id: String,
}

async fn gate(State(state): State<AppState>, Query(q): Query<UserParam>) -> Response {
    answer(state.license.gate(q.user()).await)
}

async fn install(State(state): State<AppState>) -> Response {
    answer(state.license.install_status().await)
}

async fn register_install(State(state): State<AppState>, body: Bytes) -> Response {
    let run = async {
        let b: KeyBody = parse(&body)?;
        state.license.register_install(&b.license_key).await?;
        Ok(serde_json::json!({ "ok": true }))
    };
    answer(run.await)
}

async fn signup_check(State(state): State<AppState>, body: Bytes) -> Response {
    let run = async {
        let b: CheckBody = parse(&body)?;
        state.license.signup_check(&b.license_key, &b.email).await
    };
    answer(run.await)
}

async fn bind_member(State(state): State<AppState>, body: Bytes) -> Response {
    let run = async {
        let b: BindMemberRequest = parse(&body)?;
        state.license.bind_member(&b).await
    };
    answer(run.await)
}

async fn unbind_member(State(state): State<AppState>, body: Bytes) -> Response {
    let run = async {
        let b: UserBody = parse(&body)?;
        state.license.unbind_member(&b.user_id).await
    };
    answer(run.await)
}

async fn members(State(state): State<AppState>) -> Response {
    answer(state.license.list_members().await)
}

async fn airgap_status(State(state): State<AppState>, Query(q): Query<UserParam>) -> Response {
    answer(state.license.airgap_status(q.user()).await)
}

async fn airgap_upload(
    State(state): State<AppState>,
    Query(q): Query<UserParam>,
    body: Bytes,
) -> Response {
    answer(state.license.airgap_upload(&body, q.user()).await)
}

async fn airgap_clear(State(state): State<AppState>, Query(q): Query<UserParam>) -> Response {
    let run = async {
        state.license.airgap_clear(q.user()).await?;
        Ok(serde_json::json!({ "ok": true }))
    };
    answer(run.await)
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn constant_time_eq_compares_bytes_and_lengths() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abd", b"abc"));
        assert!(!constant_time_eq(b"ab", b"abc"));
        assert!(!constant_time_eq(b"abcd", b"abc"));
        assert!(!constant_time_eq(b"", b"abc"));
        assert!(constant_time_eq(b"", b""));
    }
}
