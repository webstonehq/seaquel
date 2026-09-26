//! HTTP tenant-plane server for Seaquel.
//!
//! Exposes `build_router()` as the single entry point so integration tests can
//! drive the router directly without binding a TCP port.

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use seaquel_core::Core;
use std::sync::Arc;

mod error;
mod routes;
pub mod startup;
pub mod workspaces;

pub use routes::rpc::USER_HEADER;
pub use workspaces::Workspaces;

/// Application state shared across request handlers.
#[derive(Clone)]
pub struct AppState {
    pub core: Arc<Core>,
    /// Each user's workspace for `POST /rpc`, under `DATA_DIR/users/<id>`.
    pub workspaces: Arc<Workspaces>,
}

impl AppState {
    /// The default engines, with workspaces under `$DATA_DIR` (or the
    /// current directory).
    pub fn new() -> Self {
        Self::with_core(Arc::new(seaquel_core::with_default_plugins().build()))
    }

    /// `core`, with workspaces under `$DATA_DIR` (or the current directory).
    pub fn with_core(core: Arc<Core>) -> Self {
        Self {
            core,
            workspaces: Arc::new(Workspaces::from_env()),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the top-level Axum router. Callers can pass either an explicitly
/// constructed `AppState` (useful in tests) or `AppState::default()`.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(routes::health::health))
        .route("/api/db/connect", post(routes::db::connect::connect))
        .route(
            "/api/db/disconnect",
            post(routes::db::disconnect::disconnect),
        )
        .route("/api/db/engine", post(routes::db::engine::engine))
        .route("/api/db/query", post(routes::db::query::query))
        .route("/api/db/execute", post(routes::db::execute::execute))
        .route("/api/db/stream", get(routes::db::stream::stream))
        .route(
            "/api/db/transaction",
            post(routes::db::transaction::transaction),
        )
        .route("/api/db/test", post(routes::db::test::test))
        // Workspace calls (storage; secrets answer NOT_SUPPORTED) for the
        // user in `X-Seaquel-User`. Only Node's `/api/rpc` calls it.
        .route(
            "/rpc",
            post(routes::rpc::rpc).layer(DefaultBodyLimit::max(routes::rpc::BODY_LIMIT)),
        )
        // Static frontend. `fallback(get(...))` means:
        //   - Known API routes above take precedence.
        //   - Any GET for an unknown path gets the SvelteKit SPA shell
        //     (either the real asset or `index.html` for client-side routing).
        //   - Non-GET to unknown paths gets 405 from the MethodRouter rather
        //     than the SPA shell — that's the right behavior for stray API
        //     calls to bad URLs.
        .fallback(get(routes::static_assets::serve))
        .with_state(state)
}
