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
use seaquel_core::{ConnectPolicy, ConnectionLimits, Core, EditLimits, RunLimits};
use std::sync::Arc;

mod error;
mod routes;
pub mod startup;
pub mod web_config;
pub mod workspaces;

pub use routes::internal_license::{is_loopback_peer, SECRET_HEADER};
pub use routes::rpc::{
    MAX_EDIT_CALLS_PER_USER, MAX_IN_FLIGHT_BYTES_PER_USER, SMALL_CALL_BYTES, TOO_MANY_REQUESTS,
    USER_HEADER,
};
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

/// What a web user may connect to: [`web_config::check_connect_config`] on
/// the config Core builds (after the saved row or form and the supplied
/// secrets are resolved, right before the driver opens it), and no SSH
/// tunnels (refused before anything is opened: no SSH session, no key file
/// read on the server).
pub fn web_connect_policy() -> ConnectPolicy {
    ConnectPolicy::checked(web_config::check_connect_config, false)
}

/// What one web user may hold: 16 open connections (connects and tests
/// still in flight count), each a pool of at most 6 database connections,
/// so at most 96 per user on the databases they reach. The desktop keeps
/// the engines' defaults (no cap, pools of 10).
///
/// Every stream, query and introspection call on a connection takes one of
/// its pool's connections, so once 6 statements are streaming on it, the
/// next call (the schema tree, a table's metadata) waits for one of them to
/// finish. SQL Server holds one session plus up to 5 read-only connections
/// (it has 4 at most anyway).
pub const WEB_CONNECTION_LIMITS: ConnectionLimits = ConnectionLimits {
    per_workspace: Some(16),
    max_pool_size: Some(6),
};

/// What one run on the web may carry (owner, 2026-10-02: web only). Frames
/// are 8 MiB, and planning one statement that long took ~560 MB and ~0.5 s
/// of a worker (phase 5b probe, I1); at 2 MiB the worst is ~140 MB and
/// ~150 ms. 10,000 statements bound the results and round trips of one run.
/// 1,000 parameter values and 1 MiB of them bound what planning looks up
/// (phase 5b review, C1); the dialog sends one per `{{name}}` in the text.
pub const WEB_RUN_LIMITS: RunLimits = RunLimits {
    max_text_bytes: Some(2 * 1024 * 1024),
    max_statements: Some(10_000),
    max_param_values: Some(1_000),
    max_param_bytes: Some(1024 * 1024),
};

/// What one edit call on the web may carry (phase 5c, Decision 17): 10,000
/// changes per apply or plan touching at most 100 distinct tables (each is a
/// metadata read), 2 MiB of typed SQL and 16 MiB of values
/// among them; a data tab page with 100 filters (and 100 sort columns), 1,000
/// `IN` items and 64 KiB per filter value. `/rpc`'s 64 MiB body limit stays
/// the outer bound. Every check is linear in the request and runs before
/// anything is planned, read or run.
pub const WEB_EDIT_LIMITS: EditLimits = EditLimits {
    max_changes: Some(10_000),
    max_tables: Some(100),
    max_sql_bytes: Some(2 * 1024 * 1024),
    max_value_bytes: Some(16 * 1024 * 1024),
    max_filters: Some(100),
    max_in_values: Some(1_000),
    max_filter_value_bytes: Some(64 * 1024),
};

/// The server's Core: the compiled-in engines in [`WEB_ENGINES`] and no
/// others, under [`web_connect_policy`], [`WEB_CONNECTION_LIMITS`],
/// [`WEB_RUN_LIMITS`] and [`WEB_EDIT_LIMITS`]. Core refuses any other driver on
/// `db.connect` and `db.test` with `ENGINE_NOT_AVAILABLE`, whatever features
/// Cargo unified into this build.
pub fn web_core() -> Core {
    seaquel_core::with_plugins(|id| WEB_ENGINES.contains(&id))
        .connect_policy(web_connect_policy())
        .connection_limits(WEB_CONNECTION_LIMITS)
        .run_limits(WEB_RUN_LIMITS)
        .edit_limits(WEB_EDIT_LIMITS)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build()
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
        // Workspace calls (storage and db; secrets answer NOT_SUPPORTED)
        // for the user in `X-Seaquel-User`. Only Node's `/api/rpc` calls it.
        .route(
            "/rpc",
            post(routes::rpc::rpc).layer(DefaultBodyLimit::max(routes::rpc::BODY_LIMIT)),
        )
        // The user's query streams and connection events, multiplexed on one
        // WebSocket. Only Node's `/api/rpc/stream` upgrade reaches it.
        .route("/rpc/stream", get(routes::rpc_stream::stream))
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
