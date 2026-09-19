//! HTTP tenant-plane server for Seaquel.
//!
//! Exposes `build_router()` as the single entry point so integration tests can
//! drive the router directly without binding a TCP port.

use axum::{
    routing::{get, post},
    Router,
};
use seaquel_db::ConnectionManager;
use std::sync::Arc;

mod error;
mod routes;

/// Application state shared across request handlers.
#[derive(Clone)]
pub struct AppState {
    pub connection_manager: Arc<ConnectionManager>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            connection_manager: Arc::new(ConnectionManager::new()),
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
        .route("/api/db/query", post(routes::db::query::query))
        .route("/api/db/execute", post(routes::db::execute::execute))
        .route("/api/db/stream", get(routes::db::stream::stream))
        .route(
            "/api/db/transaction",
            post(routes::db::transaction::transaction),
        )
        .route("/api/db/test", post(routes::db::test::test))
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
