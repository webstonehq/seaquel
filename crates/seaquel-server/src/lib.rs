//! HTTP tenant-plane server for Seaquel.
//!
//! Exposes `build_router()` as the single entry point so integration tests can
//! drive the router directly without binding a TCP port.

use axum::{
    extract::DefaultBodyLimit,
    routing::{get, post},
    Router,
};
use seaquel_core::ai::native::{NativeHttp, NativeHttpOptions};
use seaquel_core::ai::AiEgress;
use seaquel_core::license::server::{LicenseServer, ServerConfig};
use seaquel_core::{
    ConnectPolicy, ConnectionLimits, Core, EditLimits, LibraryLimits, RunLimits, StateLimits,
};
use std::path::PathBuf;
use std::sync::Arc;

mod error;
mod routes;
pub mod startup;
pub mod web_config;
pub mod workspaces;

pub use error::status_for;
pub use routes::internal_license::{is_loopback_peer, SECRET_HEADER};
pub use routes::rpc::{
    MAX_EDIT_CALLS_PER_USER, MAX_IN_FLIGHT_BYTES_PER_USER, SMALL_CALL_BYTES, TOO_MANY_REQUESTS,
    USER_HEADER,
};
pub use routes::rpc_stream::{MAX_PENDING_REFUSALS, TOO_MANY_PENDING};
pub use workspaces::{Workspaces, EVENTS_LAGGED, LISTENER_EVENT_BOUND, LISTENER_EVENT_BYTE_BOUND};

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

/// What one user may store in the library on the web (phase 5d, Decision
/// 15): names, label names, folders and tags of 1 KiB, other fields of
/// 64 KiB, a saved query's text of 2 MiB (as [`WEB_RUN_LIMITS`]), 1,000
/// items per list, and 10,000 connections, 1,000 projects and 50,000 saved
/// queries per user. Sizes are refused before anything is read. A saved
/// query's versions keep at most 16 MiB together (8 versions of a 2 MiB
/// query; phase 5d-1 probe fix), pruned oldest first.
pub const WEB_LIBRARY_LIMITS: LibraryLimits = LibraryLimits {
    max_name_bytes: Some(1024),
    max_field_bytes: Some(64 * 1024),
    max_query_bytes: Some(2 * 1024 * 1024),
    max_list_items: Some(1_000),
    max_connections: Some(10_000),
    max_projects: Some(1_000),
    max_saved_queries: Some(50_000),
    max_version_bytes: Some(16 * 1024 * 1024),
};

/// What one user may store in state on the web (phase 5d-2, Decision
/// 27): a window's view state of 8 MiB (one tab's text 2 MiB, 500 tabs),
/// 50 windows and 20 view states per project (windows unused for 30 days
/// pruned, `main` not spared), a saved workflow of 16 MiB and 1,000 of
/// them, a dashboard's widgets, viewport and filter of 4 MiB, 1,000
/// dashboards and 16 MiB of versions each, an AI message of 1 MiB, 5,000
/// messages and 64 MiB of content per chat (Q17) and 10,000 chats, and a
/// setting, the AI settings record or a user theme of 256 KiB, with 200
/// user themes and 50 AI providers. Names are bounded by
/// [`WEB_LIBRARY_LIMITS`]' `max_name_bytes`. Sizes are refused before
/// anything is read, counts inside the write.
pub const WEB_STATE_LIMITS: StateLimits = StateLimits {
    max_view_state_bytes: Some(8 * 1024 * 1024),
    max_tab_text_bytes: Some(2 * 1024 * 1024),
    max_tabs: Some(500),
    max_windows: 50,
    max_window_states_per_project: 20,
    spare_main_window: false,
    max_workflow_bytes: Some(16 * 1024 * 1024),
    max_workflows: Some(1_000),
    max_dashboard_bytes: Some(4 * 1024 * 1024),
    max_dashboards: Some(1_000),
    max_dashboard_version_bytes: Some(16 * 1024 * 1024),
    max_message_bytes: Some(1024 * 1024),
    max_messages_per_chat: Some(5_000),
    max_chat_bytes: Some(64 * 1024 * 1024),
    max_chats: Some(10_000),
    max_setting_bytes: Some(256 * 1024),
    max_user_themes: Some(200),
    max_ai_providers: Some(50),
};

/// What the assistant may do per web user (phase 6, Decision 14): four
/// turns in flight at once (a fifth is `TOO_MANY_REQUESTS`, 429; each holds
/// a model stream and, while a tool runs, a database connection), and a
/// user's message of at most 1 MiB, the chat budget's message cap
/// ([`WEB_STATE_LIMITS`]' `max_message_bytes`).
pub const WEB_AI_LIMITS: seaquel_core::ai::AiLimits = seaquel_core::ai::AiLimits {
    max_turns_in_flight: Some(4),
    max_message_bytes: Some(1024 * 1024),
    ..seaquel_core::ai::AiLimits::DEFAULT
};

/// The server's Core: the compiled-in engines in [`WEB_ENGINES`] and no
/// others, under [`web_connect_policy`], [`WEB_CONNECTION_LIMITS`],
/// [`WEB_RUN_LIMITS`], [`WEB_EDIT_LIMITS`], [`WEB_LIBRARY_LIMITS`],
/// [`WEB_STATE_LIMITS`] and [`WEB_AI_LIMITS`]. Core refuses any other
/// driver on `db.connect` and `db.test` with `ENGINE_NOT_AVAILABLE`,
/// whatever features Cargo unified into this build.
///
/// Model calls (phase 6, Q1) go through seaquel-http's native client under
/// `egress` (`SEAQUEL_AI_EGRESS`, [`startup::ai_egress_from_env`]), the
/// same rule Core checks, trusting `extra_ca_file` (`NODE_EXTRA_CA_CERTS`)
/// on top of the built-in roots and using the environment's proxies.
pub fn web_core(egress: AiEgress, extra_ca_file: Option<PathBuf>) -> Core {
    let mut http = NativeHttpOptions::new(egress.into());
    http.extra_ca_file = extra_ca_file;
    seaquel_core::with_plugins(|id| WEB_ENGINES.contains(&id))
        .connect_policy(web_connect_policy())
        .connection_limits(WEB_CONNECTION_LIMITS)
        .run_limits(WEB_RUN_LIMITS)
        .edit_limits(WEB_EDIT_LIMITS)
        .library_limits(WEB_LIBRARY_LIMITS)
        .state_limits(WEB_STATE_LIMITS)
        .ai_limits(WEB_AI_LIMITS)
        .ai_http(Arc::new(NativeHttp::new(http)))
        .ai_egress(egress)
        .executor(Arc::new(seaquel_runtime::TokioExecutor))
        .build()
}

/// [`web_core`] from the environment: `SEAQUEL_AI_EGRESS` (an error for a
/// value it can't read, so the binary refuses to start) and
/// `NODE_EXTRA_CA_CERTS`.
pub fn web_core_from_env() -> Result<Core, String> {
    let egress = startup::ai_egress_from_env()?;
    let extra_ca_file = std::env::var_os(startup::EXTRA_CA_CERTS_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    Ok(web_core(egress, extra_ca_file))
}

impl AppState {
    /// [`web_core_from_env`], with workspaces under `$DATA_DIR` (or the
    /// current directory). Panics on a `SEAQUEL_AI_EGRESS` it can't read;
    /// the binary checks it first and exits with the message.
    pub fn new() -> Self {
        let core = web_core_from_env().unwrap_or_else(|e| panic!("{e}"));
        Self::with_core(Arc::new(core))
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
