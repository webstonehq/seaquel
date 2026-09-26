//! HTTP tenant-plane server for Seaquel.
//!
//! Exposes `build_router()` as the single entry point so integration tests can
//! drive the router directly without binding a TCP port.

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use seaquel_core::license::server::{LicenseServer, ServerConfig};
use seaquel_core::Core;
use std::sync::Arc;

mod error;
mod routes;
pub mod startup;
pub mod web_config;
pub mod workspaces;

pub use routes::internal_license::{is_loopback_peer, SECRET_HEADER};
pub use routes::rpc::USER_HEADER;
pub use workspaces::Workspaces;

/// Application state shared across request handlers.
#[derive(Clone)]
pub struct AppState {
    pub core: Arc<Core>,
    /// Each user's workspace for `POST /rpc`, under `DATA_DIR/users/<id>`.
    pub workspaces: Arc<Workspaces>,
    /// Licensing over `DATA_DIR/auth.db`, for `/internal/license/*`.
    pub license: Arc<LicenseServer>,
    /// The per-boot secret `/internal/*` requires (`SEAQUEL_INTERNAL_SECRET`,
    /// from `server.js`). `None` refuses every `/internal` call.
    pub internal_secret: Option<Arc<str>>,
}

/// The engines the web build serves (Decision 11b): PostgreSQL, MySQL (which
/// also serves MariaDB) and SQL Server. SQLite and DuckDB are never offered
/// on web: their "connection string" is a path on the server, so a signed-in
/// user could open `auth.db` or another user's `meta.db`, and DuckDB's
/// `read_*`, `sqlite_scan`, `ATTACH` and `COPY TO` read and write any file
/// the server can.
pub const WEB_ENGINES: &[&str] = &["postgres", "mysql", "mssql"];

/// The server's Core: the compiled-in engines in [`WEB_ENGINES`] and no
/// others. Core refuses any other driver on `/api/db/connect` and
/// `/api/db/test` with `ENGINE_NOT_AVAILABLE`, whatever features Cargo
/// unified into this build.
pub fn web_core() -> Core {
    seaquel_core::with_plugins(|id| WEB_ENGINES.contains(&id)).build()
}

impl AppState {
    /// [`web_core`], with workspaces under `$DATA_DIR` (or the current
    /// directory). This is what the binary serves.
    pub fn new() -> Self {
        Self::with_core(Arc::new(web_core()))
    }

    /// `core`, with workspaces and `auth.db` under `$DATA_DIR` (or the
    /// current directory), and the license settings from the environment
    /// (`SEAQUEL_CONTROL_URL`, the TTLs, `SEAQUEL_BUNDLE_TRUSTED_PUBKEY`).
    ///
    /// For tests that need their own engines. The server itself uses
    /// [`AppState::new`], whose Core holds only [`WEB_ENGINES`].
    pub fn with_core(core: Arc<Core>) -> Self {
        let workspaces = Arc::new(Workspaces::from_env());
        let license = Arc::new(LicenseServer::new(ServerConfig::from_env(
            workspaces.root().join(AUTH_DB_FILE),
        )));
        Self {
            core,
            workspaces,
            license,
            internal_secret: None,
        }
    }

    /// Require `secret` on `/internal/*` (an empty one counts as none).
    #[must_use]
    pub fn with_internal_secret(mut self, secret: Option<String>) -> Self {
        self.internal_secret = secret.filter(|s| !s.is_empty()).map(Arc::from);
        self
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Node's Better Auth database, in the data root. Rust reads and writes its
/// license tables; Node creates it and owns the rest.
pub const AUTH_DB_FILE: &str = "auth.db";

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
        // Licensing for Node's hooks and routes, loopback peers only (the
        // router must be served with connect info; see `main.rs`).
        .nest(
            "/internal/license",
            routes::internal_license::router(state.clone()),
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
