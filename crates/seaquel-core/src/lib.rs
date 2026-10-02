//! Seaquel Core.
//!
//! Interfaces (the Tauri app, `seaquel-server`, and later the CLI, TUI and MCP
//! server) do database work only through [`Core`]. It owns the engine
//! registry, the open connections, and the cancellation tokens of running
//! streams. It grows into the full Core from
//! `docs/plans/2026-09-24-rust-core-plugin-architecture-design.md` over the
//! following phases.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use futures::StreamExt;
use log::{debug, info};
/// Why a transaction failed, and which statement failed (see
/// [`Workspace::transaction`]).
pub use seaquel_engine::TransactionError;
use seaquel_engine::{
    not_supported, BatchStatement, BoxStream, CancellationToken, ConnectConfig, ConnectResult,
    DatabaseStatistics, DbError, Dialect, Driver, Engine, EngineRegistry, ExecuteResult,
    ExplainResult, OpenOptions, QueryResult, ReadOnlyOptions, SchemaColumn, SchemaIndex,
    SchemaTable,
};
use seaquel_sql::read_only::read_only_error;
use seaquel_sql::SqlEngine;
pub use seaquel_types::{StreamEvent, Value};

// `browser` is the wasm32 build for the web page (phase 8): Core with
// `storage` (the metadata file in memory) and `workspace` (connecting,
// runs, edits), and nothing native. Every engine feature is refused,
// `engine-duckdb` included: that one is the native driver, and the page's
// module registers the DuckDB engine's browser driver itself. So are the
// secret store, SSH, git, licensing and the imports. Build it with
// `--no-default-features --features browser,storage,workspace`.
#[cfg(all(
    feature = "browser",
    any(
        feature = "engine-postgres",
        feature = "engine-mysql",
        feature = "engine-sqlite",
        feature = "engine-mssql",
        feature = "engine-duckdb",
        feature = "secrets",
        feature = "ssh",
        feature = "git",
        feature = "license-desktop",
        feature = "license-server",
        feature = "imports",
    )
))]
compile_error!(
    "seaquel-core's `browser` feature can't be combined with an engine or native infrastructure \
     feature (only `storage` and `workspace`); build it with --no-default-features --features \
     browser,storage,workspace"
);

mod changes;
// The demo's connection (phase 8 Decision 19): the browser build only, and
// this crate's own tests.
#[cfg(all(feature = "storage", any(feature = "browser", test)))]
mod demo;
#[cfg(feature = "workspace")]
mod edits;
#[cfg(feature = "imports")]
mod imports;
#[cfg(feature = "storage")]
mod library;
#[cfg(feature = "storage")]
mod projection;
#[cfg(feature = "workspace")]
mod run;
#[cfg(all(feature = "git", feature = "storage"))]
mod shared;
#[cfg(feature = "storage")]
mod state;
#[cfg(feature = "storage")]
mod upgrade;
mod workspace;
/// `StorageChanged` and the change sequence (phase 5d, Decisions 16–17).
pub use changes::{
    is_origin, ChangeSeq, Seqd, StorageChange, StoredKind, WriteOrigin, MAX_EVENT_IDS,
    MAX_EVENT_IDS_BYTES, MAX_EVENT_ID_BYTES,
};
#[cfg(all(feature = "storage", any(feature = "browser", test)))]
pub use demo::DEMO_CONNECTION_ID;
pub use seaquel_runtime::Executor;
/// What a GUI sends to connect: the form and the secrets it supplies.
pub use seaquel_types::connect::{ConnectionForm, SuppliedSecrets};
#[cfg(feature = "storage")]
pub use upgrade::{
    MAX_VACUUM_ATTEMPTS, STRING_SECRETS_CHECKPOINT_KEY, STRING_SECRETS_NOTICE_KEY,
    STRING_SECRETS_UPGRADED_KEY, STRING_SECRETS_VACUUM_KEY,
};
#[cfg(any(feature = "workspace", feature = "storage"))]
pub use workspace::SAVED_CONNECTION_NOT_FOUND;
#[cfg(feature = "workspace")]
pub use workspace::{
    ConnectRequest, ConnectTarget, HostKeyPolicy, NO_SECRET_STORE, SECRET_UNREADABLE,
    WORKSPACE_CLOSED,
};
pub use workspace::{
    CoreError, Workspace, WorkspaceEvent, WorkspaceId, WorkspaceSpec, CONNECTION_CLOSED,
    DESKTOP_STORAGE_FILE, TUNNEL_CLOSED, WORKSPACE_EVICTED,
};

/// The metadata storage (`seaquel-storage`), for interfaces and
/// `seaquel-rpc`, which may not depend on it directly.
#[cfg(feature = "storage")]
pub use seaquel_storage as storage;

/// The secret stores (`seaquel-secrets`), for interfaces and `seaquel-rpc`,
/// which may not depend on them directly.
#[cfg(feature = "secrets")]
pub use seaquel_secrets as secrets;

/// Pure SQL text work (`seaquel-sql`: `{{param}}` substitution, the
/// read-only check), for interfaces, which may not depend on it directly.
/// The MCP server's saved queries use it.
pub use seaquel_sql as sql;

/// The workspace domain (`seaquel-workspace`), for interfaces, which may not
/// depend on it directly. Named `domain` because `workspace` is Core's own
/// [`Workspace`] module. Always there (it's pure): the `db` wire names its
/// run types in every build, while connecting and running need the
/// `workspace` feature.
pub use seaquel_workspace as domain;
/// What an edit call may carry, set per interface with
/// [`CoreBuilder::edit_limits`].
pub use seaquel_workspace::edits::EditLimits;
/// What a library call may carry, set per interface with
/// [`CoreBuilder::library_limits`].
pub use seaquel_workspace::library::LibraryLimits;
/// What a run may carry, set per interface with [`CoreBuilder::run_limits`].
pub use seaquel_workspace::run::RunLimits;
/// What a state call may carry, set per interface with
/// [`CoreBuilder::state_limits`].
pub use seaquel_workspace::state::StateLimits;

/// Git for shared projects (`seaquel-git`), and the per-repo lock.
#[cfg(feature = "git")]
pub mod git;

/// The shared projection (phase 5e): link, unlink, scan, import projects,
/// sync, the repo list, and git calls under the repo lock.
#[cfg(all(feature = "git", feature = "storage"))]
pub use shared::{
    ImportedProjects, PreviewProject, PreviewTemplate, ProjectFailure, RepoPatch, RepoPreview,
    SkippedDir, SkippedProject, SyncReport, SyncTarget, UnlinkPreview, UnlinkReport, FILE_CHANGED,
    FILE_ERROR, PROJECT_ALREADY_LINKED, PROJECT_NOT_LINKED, REPO_CONFLICTED, REPO_IN_USE,
    REPO_NOT_FOUND,
};

/// Imports from TablePlus and DBeaver (phase 5e, Decision 47).
#[cfg(feature = "imports")]
pub use imports::{ImportKeyOutcome, ImportOutcome, ImportPaths, IMPORT_SOURCE_UNREADABLE};

/// Whether this Core may read and write the user's files: shared repos and
/// other tools' connection files (phase 5e, Decision 31). There is no
/// default: a Core built without [`CoreBuilder::local_files`] answers
/// `NOT_SUPPORTED` to every `shared` and `imports` call and publishes
/// nothing from the library calls, whatever features Cargo unified. The
/// desktop and the CLI pass [`LocalFiles::Allowed`]; the web server passes
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalFiles {
    Allowed,
}

/// Licensing (`seaquel-license`): the desktop activation client and the web
/// server's license gate.
#[cfg(any(feature = "license-desktop", feature = "license-server"))]
pub mod license;

/// SSH tunnels (`seaquel-ssh`), owned by Core: [`Core::ssh_open`] and
/// [`Core::ssh_close`].
#[cfg(feature = "ssh")]
pub mod ssh;

/// A running stream's key: the workspace that started it (`None` for
/// Core's own [`Core::query_stream`]) and its query id. Stream ids are
/// scoped per workspace, so one workspace can't cancel another's query.
type StreamKey = (Option<WorkspaceId>, String);

type StreamTokens = Mutex<HashMap<StreamKey, StreamEntry>>;

/// How many early cancels ([`Core::cancel_stream_as`] on a workspace stream
/// that isn't registered yet) Core remembers per workspace, and how many of
/// a workspace's finished stream ids; the oldest is forgotten first. Per
/// workspace, so one workspace's cancels can't push out another's.
const EARLY_CANCELS: usize = 256;

/// One workspace's stream ids that aren't running: cancelled before they
/// started, and recently finished (a cancel for one of those came too late
/// and is dropped instead of remembered). Stream ids are the client's and
/// must be unique per stream.
#[derive(Default)]
struct EarlyCancels {
    cancelled: VecDeque<String>,
    finished: VecDeque<String>,
}

/// Push `id` onto a queue capped at [`EARLY_CANCELS`], unless it's there.
fn push_capped(queue: &mut VecDeque<String>, id: &str) {
    if queue.iter().any(|q| q == id) {
        return;
    }
    if queue.len() == EARLY_CANCELS {
        queue.pop_front();
    }
    queue.push_back(id.to_string());
}

/// Remove `id` from `queue`; whether it was there.
fn take(queue: &mut VecDeque<String>, id: &str) -> bool {
    match queue.iter().position(|q| q == id) {
        Some(i) => {
            queue.remove(i);
            true
        }
        None => false,
    }
}

type EarlyCancelMap = Mutex<HashMap<WorkspaceId, EarlyCancels>>;

/// A running stream's entry in [`Core::streams`].
struct StreamEntry {
    /// Tells apart two streams that were given the same query id.
    id: u64,
    /// So `disconnect` can cancel the connection's streams.
    connection_id: String,
    token: CancellationToken,
    /// Set by `disconnect` before it cancels `token`, so the stream reports
    /// the closed connection instead of ending silently like a client cancel.
    closed: Arc<AtomicBool>,
}

/// An open connection: its driver, the engine that opened it (for the
/// engine's dialect), and the workspace that owns it.
#[derive(Clone)]
struct Connection {
    engine: Arc<dyn Engine>,
    driver: Arc<dyn Driver>,
    /// The SQL rules its text is scanned with: the database type's
    /// ([`Workspace::connect`] records MariaDB as MariaDB, though it opens
    /// with the `mysql` driver), or the driver id's for [`Core::connect`].
    /// `None` for an engine id with no rules: its read-only queries and
    /// runs are refused.
    sql_engine: Option<SqlEngine>,
    /// The workspace that opened it ([`Workspace::connect`]), or `None` for
    /// [`Core::connect`]. A workspace reaches only its own connections.
    owner: Option<WorkspaceId>,
}

pub struct Core {
    engines: EngineRegistry,
    /// `Arc` (not `Box`) so a handle can be cloned out of the map and the lock
    /// released before awaiting the driver. A long-running stream must never
    /// hold this lock, or it would block every other caller — disconnect,
    /// new queries, other connections — until it finished.
    connections: RwLock<HashMap<String, Connection>>,
    /// Cancellation tokens of running streams, keyed by their workspace and
    /// the client's query id.
    streams: StreamTokens,
    /// Workspace stream keys cancelled before they were registered (the
    /// desktop's cancel and start are separate IPC calls that can arrive in
    /// either order). Locked only while `streams` is held.
    cancelled_early: EarlyCancelMap,
    next_stream: AtomicU64,
    /// Open SSH tunnels; dropping Core closes them.
    #[cfg(feature = "ssh")]
    tunnels: ssh::TunnelManager,
    /// `None` until a builder sets one: every connect and test is refused.
    connect_policy: Option<ConnectPolicy>,
    limits: ConnectionLimits,
    /// What a run may carry ([`CoreBuilder::run_limits`]).
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    run_limits: RunLimits,
    /// What an edit call may carry ([`CoreBuilder::edit_limits`]).
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    edit_limits: EditLimits,
    /// What a library call may carry ([`CoreBuilder::library_limits`]).
    #[cfg_attr(not(feature = "storage"), allow(dead_code))]
    library_limits: LibraryLimits,
    /// What a state call may carry ([`CoreBuilder::state_limits`]).
    #[cfg_attr(not(feature = "storage"), allow(dead_code))]
    state_limits: StateLimits,
    /// The clock and spawner ([`CoreBuilder::executor`]). `None`: the
    /// editor's runs (`Workspace::run`/`page`) are `NOT_SUPPORTED`.
    #[cfg_attr(not(feature = "workspace"), allow(dead_code))]
    executor: Option<Arc<dyn Executor>>,
    /// [`CoreBuilder::local_files`]; `None` refuses the user's files.
    local_files: Option<LocalFiles>,
    /// One async mutex per repo (phase 5e, Decision 38), by its
    /// canonical path: sync, publish, pull, commit and conflict resolution
    /// take it.
    #[cfg(feature = "git")]
    repo_locks: Mutex<HashMap<std::path::PathBuf, Arc<futures::lock::Mutex<()>>>>,
    /// Tests only: fails or holds a file write by path.
    #[cfg(feature = "git")]
    file_hook: Option<seaquel_git::tree::WriteHook>,
    /// Tests only: runs after a sync's optimistic plan, before its write.
    #[cfg(feature = "git")]
    #[cfg_attr(not(feature = "storage"), allow(dead_code))]
    sync_plan_hook: Option<SyncPlanHook>,
    /// Where other tools' files are ([`CoreBuilder::import_paths`]).
    #[cfg(feature = "imports")]
    import_paths: Option<ImportPaths>,
}

/// How much one workspace may open ([`CoreBuilder::connection_limits`]).
/// The default is no limit: the desktop app, the CLI and MCP server keep
/// the engines' own pool sizes. The web server sets both, so one user can't
/// hold an unbounded number of database connections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionLimits {
    /// The most connections one workspace may have open at once, counting
    /// connects and tests still in flight. Past it, [`Workspace::connect`]
    /// and [`Workspace::test`] fail with [`TOO_MANY_CONNECTIONS`] before
    /// anything is read or opened.
    pub per_workspace: Option<usize>,
    /// The most database connections one open connection's pool holds
    /// (passed to the engine as [`OpenOptions::max_pool_size`]). Never
    /// from the wire.
    pub max_pool_size: Option<u32>,
}

/// [`Workspace::connect`] or `test` when the workspace is at
/// [`ConnectionLimits::per_workspace`].
pub const TOO_MANY_CONNECTIONS: &str = "TOO_MANY_CONNECTIONS";

/// A check on a finished connect config (see [`ConnectPolicy::Checked`]).
pub type ConfigCheck = Arc<dyn Fn(&ConnectConfig) -> Result<(), DbError> + Send + Sync>;

/// What a Core may connect to. There is no default: until a builder calls
/// [`CoreBuilder::connect_policy`], [`Core::connect`], [`Core::test`],
/// [`Workspace::connect`] and [`Workspace::test`] all answer
/// `NOT_SUPPORTED`, whatever Cargo features the build unified. So an
/// interface that forgets to choose can't connect anywhere.
#[derive(Clone)]
pub enum ConnectPolicy {
    /// Any engine this Core has, any config, SSH tunnels included. The
    /// desktop app, the CLI and MCP server, and tests.
    Unrestricted,
    /// The web server's (Task 5). `allow_ssh: false` refuses a connection
    /// that needs an SSH tunnel before anything is opened (no SSH session,
    /// no key file read). `check` then runs on the finished config, right
    /// before the driver opens it; an `Err` is returned as it is.
    Checked { check: ConfigCheck, allow_ssh: bool },
}

impl ConnectPolicy {
    /// [`ConnectPolicy::Checked`] from a plain function or closure.
    pub fn checked(
        check: impl Fn(&ConnectConfig) -> Result<(), DbError> + Send + Sync + 'static,
        allow_ssh: bool,
    ) -> Self {
        Self::Checked {
            check: Arc::new(check),
            allow_ssh,
        }
    }
}

impl std::fmt::Debug for ConnectPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unrestricted => f.write_str("Unrestricted"),
            Self::Checked { allow_ssh, .. } => f
                .debug_struct("Checked")
                .field("allow_ssh", allow_ssh)
                .finish_non_exhaustive(),
        }
    }
}

/// The code for a connect or test on a Core without a [`ConnectPolicy`],
/// and for an SSH tunnel under `Checked { allow_ssh: false }`.
const CONNECT_REFUSED: &str = "NOT_SUPPORTED";

#[derive(Default)]
pub struct CoreBuilder {
    engines: EngineRegistry,
    #[cfg(feature = "ssh")]
    ssh: ssh::TunnelOptions,
    connect_policy: Option<ConnectPolicy>,
    limits: ConnectionLimits,
    run_limits: RunLimits,
    edit_limits: EditLimits,
    library_limits: LibraryLimits,
    state_limits: StateLimits,
    executor: Option<Arc<dyn Executor>>,
    local_files: Option<LocalFiles>,
    #[cfg(feature = "git")]
    file_hook: Option<seaquel_git::tree::WriteHook>,
    #[cfg(feature = "git")]
    sync_plan_hook: Option<SyncPlanHook>,
    #[cfg(feature = "imports")]
    import_paths: Option<ImportPaths>,
}

/// Tests only ([`CoreBuilder::sync_plan_hook`]).
pub type SyncPlanHook = Arc<dyn Fn() -> futures::future::BoxFuture<'static, ()> + Send + Sync>;

impl CoreBuilder {
    /// Tests only: `hook` runs after a sync planned outside the write lock,
    /// before it takes the lock to check and write the plan (the race test
    /// edits a row there).
    #[cfg(feature = "git")]
    #[doc(hidden)]
    #[must_use]
    pub fn sync_plan_hook(mut self, hook: SyncPlanHook) -> Self {
        self.sync_plan_hook = Some(hook);
        self
    }

    /// Lets this Core read and write the user's files (see [`LocalFiles`]).
    /// The desktop and the CLI only.
    #[must_use]
    pub fn local_files(mut self, local_files: LocalFiles) -> Self {
        self.local_files = Some(local_files);
        self
    }

    /// Where the imports look for TablePlus's and DBeaver's files
    /// ([`ImportPaths::from_env`] on the desktop; a temp home in tests).
    /// Without it, an import of the default location answers `found:
    /// false`.
    #[cfg(feature = "imports")]
    #[must_use]
    pub fn import_paths(mut self, paths: ImportPaths) -> Self {
        self.import_paths = Some(paths);
        self
    }

    /// Tests only: `hook` runs before every file write of the shared
    /// projection, with the absolute path; an `Err` fails that write as an
    /// I/O error would (the projection replay's failing paths). It may
    /// block, which holds the write (the lock-order tests).
    #[cfg(feature = "git")]
    #[doc(hidden)]
    #[must_use]
    pub fn file_write_hook(mut self, hook: seaquel_git::tree::WriteHook) -> Self {
        self.file_hook = Some(hook);
        self
    }

    pub fn engine(mut self, engine: Arc<dyn Engine>) -> Self {
        self.engines.register(engine);
        self
    }

    /// What this Core may connect to. Required for any connect or test:
    /// without it they're refused (see [`ConnectPolicy`]).
    #[must_use]
    pub fn connect_policy(mut self, policy: ConnectPolicy) -> Self {
        self.connect_policy = Some(policy);
        self
    }

    /// Limits on what each workspace opens. Without it, none.
    #[must_use]
    pub fn connection_limits(mut self, limits: ConnectionLimits) -> Self {
        self.limits = limits;
        self
    }

    /// What a run may carry: its text's size, its statement count and its
    /// parameter values. Without it, no limit (the desktop, the CLI, MCP);
    /// the web server sets all four.
    #[must_use]
    pub fn run_limits(mut self, limits: RunLimits) -> Self {
        self.run_limits = limits;
        self
    }

    /// What an edit call (`Workspace::plan_edits`, `apply_changes`,
    /// `table_page`) may carry: its changes, their SQL and values, and a
    /// table page's filters. Without it, no limit (the desktop, the CLI,
    /// MCP); the web server sets all six.
    #[must_use]
    pub fn edit_limits(mut self, limits: EditLimits) -> Self {
        self.edit_limits = limits;
        self
    }

    /// What a library call (`Workspace::create_connection`, …) may carry:
    /// name, field and query sizes, list lengths, and how many connections,
    /// projects and saved queries a workspace may hold. Without it, no
    /// limit (the desktop, the CLI, MCP); the web server sets all seven
    /// (phase 5d, Decision 15).
    #[must_use]
    pub fn library_limits(mut self, limits: LibraryLimits) -> Self {
        self.library_limits = limits;
        self
    }

    /// What a state call (dashboards, workflows, chats, settings, themes,
    /// window view state, …) may carry and what a workspace may hold.
    /// Without it, [`StateLimits::DESKTOP`]: no limit but the window counts
    /// (the desktop, the CLI, MCP). The web server sets every one (phase
    /// 5d-2, Decision 27).
    #[must_use]
    pub fn state_limits(mut self, limits: StateLimits) -> Self {
        self.state_limits = limits;
        self
    }

    /// The runtime Core takes time from (statement timings and history
    /// timestamps in `Workspace::run`). There is no default: without one,
    /// `Workspace::run` and `Workspace::page` answer `NOT_SUPPORTED`, as a
    /// Core without a [`ConnectPolicy`] refuses to connect. Interfaces pass
    /// `seaquel_runtime::TokioExecutor`.
    #[must_use]
    pub fn executor(mut self, executor: Arc<dyn Executor>) -> Self {
        self.executor = Some(executor);
        self
    }

    pub fn build(self) -> Core {
        Core {
            executor: self.executor,
            connect_policy: self.connect_policy,
            limits: self.limits,
            run_limits: self.run_limits,
            edit_limits: self.edit_limits,
            library_limits: self.library_limits,
            state_limits: self.state_limits,
            engines: self.engines,
            connections: RwLock::default(),
            streams: Mutex::default(),
            cancelled_early: Mutex::default(),
            next_stream: AtomicU64::new(0),
            #[cfg(feature = "ssh")]
            tunnels: ssh::TunnelManager::new(self.ssh),
            local_files: self.local_files,
            #[cfg(feature = "git")]
            repo_locks: Mutex::default(),
            #[cfg(feature = "git")]
            file_hook: self.file_hook,
            #[cfg(feature = "git")]
            sync_plan_hook: self.sync_plan_hook,
            #[cfg(feature = "imports")]
            import_paths: self.import_paths,
        }
    }
}

/// A builder with every plugin this build's Cargo features enable.
pub fn with_default_plugins() -> CoreBuilder {
    with_plugins(|_| true)
}

/// A builder with the plugins this build's Cargo features enable whose
/// engine id `allow` accepts. An interface that must never offer some
/// engine (the web server and SQLite or DuckDB, which read and write the
/// server's files) registers through this, so the rule holds even when
/// Cargo's feature unification compiles those engines in (a workspace test
/// build): Core refuses a driver it has no engine for.
pub fn with_plugins(allow: impl Fn(&str) -> bool) -> CoreBuilder {
    let mut builder = Core::builder();
    for engine in compiled_engines() {
        if allow(engine.id()) {
            builder = builder.engine(engine);
        }
    }
    builder
}

/// The engines this build's Cargo features compile in.
fn compiled_engines() -> Vec<Arc<dyn Engine>> {
    vec![
        #[cfg(feature = "engine-postgres")]
        seaquel_engine_postgres::engine(),
        #[cfg(feature = "engine-mysql")]
        seaquel_engine_mysql::engine(),
        #[cfg(feature = "engine-sqlite")]
        seaquel_engine_sqlite::engine(),
        #[cfg(feature = "engine-mssql")]
        seaquel_engine_mssql::engine(),
        #[cfg(feature = "engine-duckdb")]
        seaquel_engine_duckdb::engine(),
    ]
}

/// How [`Core::query_stream`] runs a query. Build it from
/// `QueryOptions::default()` and the `with_*` methods, so adding an option
/// later doesn't touch every caller.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct QueryOptions {
    /// Run through the AI's token check (`seaquel_sql::read_only`) and then
    /// [`Driver::query_read_only`], which the database enforces: the AI's
    /// `run_query` tool and dashboard widgets. Off by default (the editor).
    pub read_only: bool,
    /// Only with [`QueryOptions::read_only`]: return at most this many rows
    /// and mark the final batch `truncated` when the query had more, instead
    /// of failing with `RESULT_TOO_LARGE` past the driver's row cap (which
    /// still bounds it). `None` (the default) keeps that failure.
    ///
    /// Set without `read_only` it is a caller's mistake, and the stream ends
    /// with an `INVALID_OPTIONS` error before anything runs: the editor's
    /// streaming path has no row limit, and silently ignoring it would hand
    /// a caller that asked for a sample the whole table.
    pub max_rows: Option<usize>,
    /// Only with [`QueryOptions::read_only`]: a limit the database itself
    /// enforces on the statement ([`ReadOnlyOptions::timeout`]), so a query
    /// the caller gives up on doesn't keep running on the server. Past it
    /// the stream ends with a `TIMEOUT` error. It is a backstop for the
    /// caller's own deadline (dropping or cancelling the stream), which it
    /// doesn't replace. `None` (the default) sets none; set without
    /// `read_only` it ends the stream with `INVALID_OPTIONS`, like
    /// `max_rows`.
    pub timeout: Option<Duration>,
    /// Only with [`QueryOptions::read_only`]: a byte budget for the rows
    /// the driver keeps ([`ReadOnlyOptions::max_bytes`]). Once the decoded
    /// cells add up to it the driver stops fetching and the final batch is
    /// marked `truncated`, as at `max_rows`. One cell can't be split, so a
    /// single row can still bring a cell as large as the database allows.
    /// `None` (the default) sets none; set without `read_only` it ends the
    /// stream with `INVALID_OPTIONS`, like `max_rows`.
    pub max_bytes: Option<usize>,
}

impl QueryOptions {
    #[must_use]
    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// See [`QueryOptions::max_rows`]; valid only with `read_only`.
    #[must_use]
    pub fn with_max_rows(mut self, max_rows: Option<usize>) -> Self {
        self.max_rows = max_rows;
        self
    }

    /// See [`QueryOptions::timeout`]; valid only with `read_only`.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    /// See [`QueryOptions::max_bytes`]; valid only with `read_only`.
    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: Option<usize>) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// `INVALID_OPTIONS` for options that can't be combined.
    fn check(self) -> Result<(), DbError> {
        let invalid = |option: &str| DbError {
            message: format!("{option} is only supported on read-only queries"),
            code: "INVALID_OPTIONS".to_string(),
        };
        if self.max_rows.is_some() && !self.read_only {
            return Err(invalid("max_rows"));
        }
        if self.timeout.is_some() && !self.read_only {
            return Err(invalid("timeout"));
        }
        if self.max_bytes.is_some() && !self.read_only {
            return Err(invalid("max_bytes"));
        }
        Ok(())
    }

    /// What the driver's read-only path gets.
    fn read_only_options(self) -> ReadOnlyOptions {
        ReadOnlyOptions::default()
            .with_max_rows(self.max_rows)
            .with_timeout(self.timeout)
            .with_max_bytes(self.max_bytes)
    }
}

/// The `seaquel_sql` engine whose token rules apply to a connection opened by
/// the engine with this id, for [`Core::connect`], which knows only the
/// driver. No fallback: an id without rules is refused, not checked under
/// some other engine's rules. `mysql` also serves MariaDB, whose `/*M!`
/// comments the MySQL rules refuse; [`Workspace::connect`] records MariaDB
/// from the connection's database type instead.
fn sql_engine(engine_id: &str) -> Option<SqlEngine> {
    match engine_id {
        "postgres" => Some(SqlEngine::Postgres),
        "mysql" => Some(SqlEngine::Mysql),
        "sqlite" => Some(SqlEngine::Sqlite),
        "mssql" => Some(SqlEngine::Mssql),
        "duckdb" => Some(SqlEngine::Duckdb),
        _ => None,
    }
}

/// Fix 14's token check for a read-only query on a connection with these
/// SQL rules, as [`DbError::read_only`] with the exact text the TS shows.
fn check_read_only(sql: &str, engine: Option<SqlEngine>) -> Result<(), DbError> {
    let refusal = match engine {
        Some(engine) => read_only_error(sql, engine),
        None => Some(seaquel_sql::read_only::READ_ONLY_MESSAGE),
    };
    refusal.map_or(Ok(()), |message| Err(DbError::read_only(message)))
}

/// [`seaquel_engine::EXPLAIN_ONE_STATEMENT`] as `READ_ONLY` when `sql` holds
/// more than one statement, split on `;` under the engine's quoting. Only
/// called after [`check_read_only`], which refuses an engine without rules.
fn check_one_statement(sql: &str, engine: Option<SqlEngine>) -> Result<(), DbError> {
    let Some(engine) = engine else {
        return Err(DbError::read_only(
            seaquel_sql::read_only::READ_ONLY_MESSAGE,
        ));
    };
    if seaquel_sql::scan::split_statements(sql, engine).len() > 1 {
        return Err(DbError::read_only(seaquel_engine::EXPLAIN_ONE_STATEMENT));
    }
    Ok(())
}

/// The terminal event of a stream whose connection `disconnect` closed.
fn connection_closed() -> StreamEvent {
    StreamEvent::Error {
        message: "Connection was closed while the query was running".to_string(),
        code: "CONNECTION_CLOSED".to_string(),
    }
}

fn sql_keyword(sql: &str) -> String {
    sql.split_whitespace().next().unwrap_or("?").to_uppercase()
}

impl Core {
    pub fn builder() -> CoreBuilder {
        CoreBuilder::default()
    }

    /// Open one user's workspace: its metadata storage at
    /// `<data_dir>/<storage_file>` and its secret store. Each call opens a
    /// new, independent workspace; the caller keeps it.
    ///
    /// Storage failures keep their codes (`LEGACY_STORAGE`,
    /// `STORAGE_CORRUPT`, `NO_DATA_DIR`, `STORAGE_ERROR`, and for a
    /// read-only spec `STORAGE_NEEDS_UPGRADE` and `STORAGE_NOT_FOUND`).
    // In the browser (phase 8) nothing is `Send`: the page has one thread,
    // and storage's in-memory SQLite and the executor are local to it.
    #[cfg_attr(target_arch = "wasm32", allow(clippy::arc_with_non_send_sync))]
    pub async fn open_workspace(&self, spec: WorkspaceSpec) -> Result<Arc<Workspace>, CoreError> {
        info!(activity = "workspace.open"; "Opening workspace");
        let workspace = Workspace::open(spec, self.executor.as_ref()).await?;
        // Phase 5d Decision 12a: move secrets left in stored connection
        // strings to the keychain, then strip them (once; a read-only open
        // never runs it).
        #[cfg(feature = "storage")]
        workspace
            .upgrade_string_secrets(self.executor.as_deref())
            .await;
        // Phase 5d-1 probe fix: rows an older release wrote or renamed
        // after the `backfill_name_keys` step (a downgrade, then this
        // release again) get their `name_key` back. Only a read when there
        // are none; a failure is logged and the lookups still fold the
        // NULL-key rows themselves.
        #[cfg(feature = "storage")]
        if let Err(e) = storage::refill_name_keys(workspace.storage()).await {
            log::warn!(activity = "workspace.open", code = e.code(); "Refilling name keys failed");
        }
        // 5d-2 Task 7 review: workflows and versions an older release wrote
        // without their list metadata get it (the lists compute it for such
        // a row meanwhile).
        #[cfg(feature = "storage")]
        if let Err(e) = storage::refill_list_meta(workspace.storage()).await {
            log::warn!(activity = "workspace.open", code = e.code(); "Refilling list metadata failed");
        }
        Ok(Arc::new(workspace))
    }

    /// The [`ConnectPolicy`], or `NOT_SUPPORTED` without one.
    fn connect_policy(&self) -> Result<&ConnectPolicy, DbError> {
        self.connect_policy.as_ref().ok_or_else(|| DbError {
            code: CONNECT_REFUSED.to_string(),
            message: "Connecting isn't enabled here (no connect policy is set)".to_string(),
        })
    }

    /// `Ok` if the policy lets a connection through an SSH tunnel be
    /// opened. [`Workspace::connect`] and `test` ask before opening one.
    #[cfg(any(feature = "workspace", feature = "ssh"))]
    pub(crate) fn check_ssh_allowed(&self) -> Result<(), DbError> {
        match self.connect_policy()? {
            ConnectPolicy::Checked {
                allow_ssh: false, ..
            } => Err(DbError {
                code: CONNECT_REFUSED.to_string(),
                message: "SSH tunnels aren't available here".to_string(),
            }),
            _ => Ok(()),
        }
    }

    /// The policy's verdict on a finished `config`.
    fn check_config(&self, config: &ConnectConfig) -> Result<(), DbError> {
        match self.connect_policy()? {
            ConnectPolicy::Unrestricted => Ok(()),
            ConnectPolicy::Checked { check, .. } => check(config),
        }
    }

    /// What every engine open gets: the pool size from the limits.
    fn open_options(&self) -> OpenOptions {
        OpenOptions {
            max_pool_size: self.limits.max_pool_size,
        }
    }

    /// [`CoreBuilder::local_files`]: `None` when this Core may not touch
    /// the user's files.
    pub fn local_files(&self) -> Option<LocalFiles> {
        self.local_files
    }

    /// `NOT_SUPPORTED` unless this Core may touch the user's files.
    #[cfg(any(feature = "git", feature = "imports"))]
    pub(crate) fn require_local_files(&self) -> Result<(), CoreError> {
        match self.local_files {
            Some(LocalFiles::Allowed) => Ok(()),
            None => Err(CoreError::new(
                "NOT_SUPPORTED",
                "Shared projects and imports aren't available here.",
            )),
        }
    }

    /// The limits [`CoreBuilder::connection_limits`] set.
    pub fn connection_limits(&self) -> ConnectionLimits {
        self.limits
    }

    /// The limits [`CoreBuilder::run_limits`] set.
    pub fn run_limits(&self) -> RunLimits {
        self.run_limits
    }

    /// The limits [`CoreBuilder::edit_limits`] set.
    pub fn edit_limits(&self) -> EditLimits {
        self.edit_limits
    }

    /// The limits [`CoreBuilder::library_limits`] set.
    pub fn library_limits(&self) -> LibraryLimits {
        self.library_limits
    }

    /// The limits [`CoreBuilder::state_limits`] set.
    pub fn state_limits(&self) -> StateLimits {
        self.state_limits
    }

    /// Ids of the engines in this build, sorted.
    pub fn engine_ids(&self) -> Vec<&'static str> {
        self.engines.ids()
    }

    /// Open a connection that no workspace owns. Interfaces connect through
    /// [`Workspace::connect`]; this stays for the engine tests.
    pub async fn connect(&self, config: &ConnectConfig) -> Result<ConnectResult, DbError> {
        self.connect_as(config, None, None).await
    }

    /// Open a connection owned by `owner`, scanned with `sql_engine`
    /// (`None`: the driver id's rules).
    pub(crate) async fn connect_as(
        &self,
        config: &ConnectConfig,
        owner: Option<WorkspaceId>,
        sql_engine: Option<SqlEngine>,
    ) -> Result<ConnectResult, DbError> {
        let driver_name = config.driver.as_str();
        info!(activity = "db.connect", driver = driver_name; "Connecting");
        // An engine this Core lacks is refused as such first; nothing is
        // opened either way.
        let engine = self
            .engines
            .get(driver_name)
            .ok_or_else(|| DbError::engine_not_available(driver_name))?;
        self.check_config(config)?;
        let driver = engine.open_with(config, self.open_options()).await?;
        let connection_id = format!("{}-{}", driver_name, uuid::Uuid::new_v4());
        self.connections
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                connection_id.clone(),
                Connection {
                    sql_engine: sql_engine.or_else(|| crate::sql_engine(driver_name)),
                    engine,
                    driver,
                    owner,
                },
            );

        info!(activity = "db.connect", driver = driver_name, connection_id = connection_id.as_str(); "Connected");
        Ok(ConnectResult { connection_id })
    }

    /// Open and close a connection without registering it ("Test connection").
    pub async fn test(&self, config: &ConnectConfig) -> Result<(), DbError> {
        debug!(activity = "db.test", driver = config.driver.as_str(); "Testing connection");
        let driver_name = config.driver.as_str();
        let engine = self
            .engines
            .get(driver_name)
            .ok_or_else(|| DbError::engine_not_available(driver_name))?;
        self.check_config(config)?;
        let driver = engine.open_with(config, self.open_options()).await?;
        driver.close().await
    }

    /// Close any connection, whoever owns it. Idempotent: an unknown id
    /// succeeds. A connection [`Workspace::connect`] opened through an SSH
    /// tunnel has that tunnel closed too. Interfaces use
    /// [`Workspace::disconnect`]; this stays for the engine tests.
    ///
    /// Cancels the connection's running streams first, since the driver's
    /// `close()` waits for them to hand back their pooled connections. Each
    /// ends with a `CONNECTION_CLOSED` error so clients stop waiting. A
    /// cancelled stream only lets go of its connection when it is next polled
    /// or dropped, so a caller holding a stream it never polls again delays
    /// this until it drops that stream.
    ///
    /// Only the newest stream under each query id is tracked, so if a client
    /// reused a query id, an older stream with it is not cancelled here (it
    /// still stops when dropped).
    pub async fn disconnect(&self, connection_id: &str) -> Result<(), DbError> {
        self.disconnect_as(connection_id, None).await
    }

    /// [`Core::disconnect`] for `owner`: with `Some`, a connection it doesn't
    /// own (or none at all) is `CONNECTION_NOT_FOUND` and stays open.
    pub(crate) async fn disconnect_as(
        &self,
        connection_id: &str,
        owner: Option<WorkspaceId>,
    ) -> Result<(), DbError> {
        info!(activity = "db.disconnect", connection_id = connection_id; "Disconnecting");
        let connection = {
            let mut connections = self
                .connections
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            match (connections.get(connection_id), owner) {
                (Some(c), Some(owner)) if c.owner != Some(owner) => {
                    return Err(DbError::connection_not_found(connection_id))
                }
                (None, Some(_)) => return Err(DbError::connection_not_found(connection_id)),
                _ => connections.remove(connection_id),
            }
        };
        // Out of the ownership map before anything is awaited: if this future
        // is dropped, the guard still closes the tunnel.
        #[cfg(feature = "ssh")]
        let tunnel = self.take_tunnel_of(connection_id);
        let closed = match connection {
            Some(Connection { driver, .. }) => {
                self.cancel_streams_of(connection_id);
                // Waits for in-flight queries to return their pooled
                // connections. Cancelled streams return theirs as soon as
                // they are polled.
                driver.close().await
            }
            None => Ok(()),
        };
        // The SSH tunnel `Workspace::connect` opened for it, after the
        // driver (closing it cuts whatever still runs through it), and even
        // when closing the driver failed.
        #[cfg(feature = "ssh")]
        if let Some(tunnel) = tunnel {
            tunnel.close().await;
        }
        closed
    }

    pub fn connection_count(&self) -> usize {
        self.connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// A clone of the connection's handles, so the lock is released before
    /// anything is awaited.
    fn connection(&self, connection_id: &str) -> Result<Connection, DbError> {
        self.connection_as(connection_id, None)
    }

    /// The connection, if `owner` may use it: any with `None` (Core's own
    /// methods), else only one it owns. Not owned and not open are the same
    /// `CONNECTION_NOT_FOUND`, so the answer doesn't tell whether the id
    /// exists.
    fn connection_as(
        &self,
        connection_id: &str,
        owner: Option<WorkspaceId>,
    ) -> Result<Connection, DbError> {
        self.connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(connection_id)
            .filter(|c| owner.is_none() || c.owner == owner)
            .cloned()
            .ok_or_else(|| DbError::connection_not_found(connection_id))
    }

    /// The ids of the connections `owner` owns.
    pub(crate) fn connections_of(&self, owner: WorkspaceId) -> Vec<String> {
        self.connections
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, c)| c.owner == Some(owner))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// A handle for any connection's calls, whoever owns it. Interfaces use
    /// [`Workspace::engine`]; `seaquel-rpc`'s engine dispatch and the engine
    /// tests use this.
    pub fn connection_handle(&self, connection_id: &str) -> ConnectionHandle<'_> {
        ConnectionHandle {
            core: self,
            connection_id: connection_id.to_string(),
            owner: None,
        }
    }

    /// A handle checked against `owner` on every call.
    pub(crate) fn connection_handle_as(
        &self,
        connection_id: &str,
        owner: Option<WorkspaceId>,
    ) -> ConnectionHandle<'_> {
        ConnectionHandle {
            core: self,
            connection_id: connection_id.to_string(),
            owner,
        }
    }

    /// The engine that opened a connection.
    pub fn engine(&self, connection_id: &str) -> Result<Arc<dyn Engine>, DbError> {
        Ok(self.connection(connection_id)?.engine)
    }

    /// Run `f` with the connection's SQL dialect. `NOT_SUPPORTED` when the
    /// engine's dialect still lives in TypeScript.
    ///
    /// A closure rather than a returned `&dyn Dialect`: the dialect borrows
    /// from the engine, which is only reachable through a cloned `Arc` once
    /// the connections lock is released. Dialects are pure, so `f` is sync.
    pub fn with_dialect<R>(
        &self,
        connection_id: &str,
        f: impl FnOnce(&dyn Dialect) -> R,
    ) -> Result<R, DbError> {
        self.connection_handle(connection_id).with_dialect(f)
    }

    pub async fn query(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<QueryResult, DbError> {
        self.connection_handle(connection_id)
            .query(sql, params)
            .await
    }

    pub async fn execute(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
    ) -> Result<ExecuteResult, DbError> {
        self.connection_handle(connection_id)
            .execute(sql, params)
            .await
    }

    pub async fn transaction(
        &self,
        connection_id: &str,
        statements: Vec<BatchStatement>,
    ) -> Result<Vec<u64>, TransactionError> {
        self.connection_handle(connection_id)
            .transaction(statements)
            .await
    }

    // ── Introspection ──

    pub async fn list_schemas(&self, connection_id: &str) -> Result<Vec<String>, DbError> {
        self.connection_handle(connection_id).list_schemas().await
    }

    pub async fn schema_tables(&self, connection_id: &str) -> Result<Vec<SchemaTable>, DbError> {
        self.connection_handle(connection_id).schema_tables().await
    }

    pub async fn table_metadata(
        &self,
        connection_id: &str,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        self.connection_handle(connection_id)
            .table_metadata(schema, table)
            .await
    }

    pub async fn statistics(&self, connection_id: &str) -> Result<DatabaseStatistics, DbError> {
        self.connection_handle(connection_id).statistics().await
    }

    pub async fn explain(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        self.connection_handle(connection_id)
            .explain(sql, params, analyze)
            .await
    }

    /// See [`ConnectionHandle::explain_read_only`].
    pub async fn explain_read_only(
        &self,
        connection_id: &str,
        sql: &str,
        params: Vec<Value>,
        timeout: Option<Duration>,
    ) -> Result<ExplainResult, DbError> {
        self.connection_handle(connection_id)
            .explain_read_only(sql, params, timeout)
            .await
    }

    /// Run a query and deliver its results as client events: zero or more
    /// `Batch` events, then exactly one `Done` or `Error`.
    ///
    /// After [`Core::cancel_stream`] with the same `query_id`, or when the
    /// returned stream is dropped, the driver stops fetching and the stream
    /// ends with no terminal event. After [`Core::disconnect`] of its
    /// connection it stops too, but ends with a `CONNECTION_CLOSED` error: the
    /// client didn't ask for that and would otherwise wait for a terminal
    /// event forever.
    ///
    /// With [`QueryOptions::read_only`], the SQL first goes through the AI's
    /// token check for the connection's engine; a refusal ends the stream
    /// with `READ_ONLY` and never reaches the driver. Then the driver's
    /// [`Driver::query_read_only`] runs, under the same cancellation, and
    /// its result is one final `Batch` and `Done`. With
    /// [`QueryOptions::max_rows`] that batch holds at most that many rows
    /// and is `truncated` when the query had more; without `read_only`,
    /// `max_rows` ends the stream with `INVALID_OPTIONS`.
    ///
    /// The stream isn't owned by a workspace: interfaces use
    /// [`Workspace::query_stream`], and this stays for the engine tests.
    pub fn query_stream(
        &self,
        query_id: String,
        connection_id: String,
        sql: String,
        params: Vec<Value>,
        options: QueryOptions,
    ) -> BoxStream<'_, StreamEvent> {
        self.query_stream_as(None, query_id, connection_id, sql, params, options)
    }

    /// [`Core::query_stream`] for `owner`: with `Some`, a connection it
    /// doesn't own ends the stream with `CONNECTION_NOT_FOUND` before
    /// anything is registered, and the stream is registered under `owner`.
    pub(crate) fn query_stream_as(
        &self,
        owner: Option<WorkspaceId>,
        query_id: String,
        connection_id: String,
        sql: String,
        params: Vec<Value>,
        options: QueryOptions,
    ) -> BoxStream<'_, StreamEvent> {
        let keyword = sql_keyword(&sql);
        debug!(activity = "db.query_stream", query_id = query_id.as_str(), connection_id = connection_id.as_str(), keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(), read_only = options.read_only, max_rows = options.max_rows, max_bytes = options.max_bytes, timeout_ms = options.timeout.map(|t| t.as_millis() as u64); "Query stream");

        if owner.is_some() {
            if let Err(e) = self.connection_as(&connection_id, owner) {
                return Box::pin(futures::stream::once(std::future::ready(
                    StreamEvent::from(e),
                )));
            }
        }
        // Registered now, not on first poll, so a cancel that arrives before
        // the stream starts still counts.
        let (token, closed, guard) = self.register_stream((owner, query_id), connection_id.clone());
        Box::pin(async_stream::stream! {
            let _guard = guard;
            // Cancelled before the first poll, e.g. by `disconnect`.
            if token.is_cancelled() {
                if closed.load(Ordering::SeqCst) {
                    yield connection_closed();
                }
                return;
            }
            if let Err(e) = options.check() {
                yield StreamEvent::from(e);
                return;
            }
            let connection = match self.connection_as(&connection_id, owner) {
                Ok(connection) => connection,
                Err(e) => {
                    yield StreamEvent::from(e);
                    return;
                }
            };
            if options.read_only {
                // The check runs before the driver is touched.
                if let Err(e) = check_read_only(&sql, connection.sql_engine) {
                    yield StreamEvent::from(e);
                    return;
                }
                // Dropping the driver's future is how a read-only query is
                // cancelled: `take_until` drops it on cancel or disconnect,
                // and dropping this stream drops it too.
                // Dropped at the end of this block, before any closing
                // event: that's what stops the statement.
                let outcome = {
                    let mut result = std::pin::pin!(futures::stream::once(
                        connection
                            .driver
                            .query_read_only_with(&sql, params, options.read_only_options())
                    )
                    .take_until(token.cancelled()));
                    result.next().await
                };
                match outcome {
                    _ if token.is_cancelled() => {
                        if closed.load(Ordering::SeqCst) {
                            yield connection_closed();
                        }
                    }
                    Some(Ok(result)) => {
                        yield StreamEvent::Batch(seaquel_engine::StreamBatch {
                            columns: Some(result.columns),
                            rows: result.rows,
                            is_final: true,
                            truncated: result.truncated,
                        });
                        yield StreamEvent::Done;
                    }
                    Some(Err(e)) => yield StreamEvent::from(e),
                    // `take_until` ends without an item only on cancel.
                    None => {}
                }
                return;
            }
            let driver = connection.driver;
            {
                // `take_until` ends the stream on cancel even while the driver
                // is awaiting a query it can't interrupt (the default,
                // non-streaming `query_stream`, e.g. MSSQL). The driver's
                // stream is dropped at the end of this block, before any
                // closing event, which is what stops the statement on the
                // server: the sqlx engines cancel it from a connection of
                // their own, and DuckDB (on `spawn_blocking`) interrupts it.
                // `Workspace::run`'s stream and page loops (run.rs) do the
                // same; keep them in step.
                let mut batches = std::pin::pin!(driver
                    .query_stream(sql, params, token.clone())
                    .take_until(token.cancelled()));
                while let Some(item) = batches.next().await {
                    if token.is_cancelled() {
                        break;
                    }
                    match item {
                        Ok(batch) => yield StreamEvent::Batch(batch),
                        Err(e) => {
                            yield StreamEvent::from(e);
                            return;
                        }
                    }
                }
            }
            if !token.is_cancelled() {
                yield StreamEvent::Done;
            } else if closed.load(Ordering::SeqCst) {
                yield connection_closed();
            }
        })
    }

    /// Cancel a running stream. Unknown or finished query ids are ignored.
    ///
    /// If a query id is reused while an earlier stream with it still runs,
    /// only the newest one can be cancelled by id; the older ones still stop
    /// when dropped.
    ///
    /// It reaches only streams started with [`Core::query_stream`], not a
    /// workspace's ([`Workspace::cancel`]).
    pub fn cancel_stream(&self, query_id: &str) {
        self.cancel_stream_as(None, query_id, None);
    }

    /// Cancel `owner`'s stream with this id. For a workspace (`Some`), an id
    /// that isn't registered yet is remembered, and a stream registered
    /// under it later starts cancelled (see [`EARLY_CANCELS`]), unless
    /// `closed` (the workspace's `close_all` flag) is set. It's read under
    /// the streams lock, which `cancel_streams_owned_by` also holds while it
    /// drops the workspace's early cancels, and `close_all` sets it before
    /// that: so a cancel racing `close_all` either lands before the purge
    /// (and is dropped by it) or sees the flag, and none comes back.
    pub(crate) fn cancel_stream_as(
        &self,
        owner: Option<WorkspaceId>,
        query_id: &str,
        closed: Option<&AtomicBool>,
    ) {
        debug!(activity = "db.cancel_stream", query_id = query_id; "Cancel stream");
        let key = (owner, query_id.to_string());
        let token = {
            let streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
            let token = streams.get(&key).map(|entry| entry.token.clone());
            let remember = !closed.is_some_and(|c| c.load(Ordering::SeqCst));
            if let (None, Some(owner), true) = (&token, owner, remember) {
                let mut early = self
                    .cancelled_early
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                let early = early.entry(owner).or_default();
                // A stream that already ran under this id: the cancel came
                // too late, and there's nothing to remember.
                if !early.finished.iter().any(|id| id == query_id) {
                    push_capped(&mut early.cancelled, query_id);
                }
            }
            token
        };
        // Cancelled outside the lock: `cancel()` runs wakers.
        if let Some(token) = token {
            token.cancel();
        }
    }

    /// Cancel every running stream on one connection.
    fn cancel_streams_of(&self, connection_id: &str) {
        let tokens: Vec<CancellationToken> = self
            .streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|entry| entry.connection_id == connection_id)
            .map(|entry| {
                // Before the cancel, so the woken stream sees it.
                entry.closed.store(true, Ordering::SeqCst);
                entry.token.clone()
            })
            .collect();
        for token in tokens {
            token.cancel();
        }
    }

    /// Cancel every stream `owner` started. They end silently, like a
    /// client cancel.
    pub(crate) fn cancel_streams_owned_by(&self, owner: WorkspaceId) {
        let tokens: Vec<CancellationToken> = {
            let streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
            // The workspace is going away: forget its early cancels too,
            // under the streams lock, like `cancel_stream_as` adds them.
            self.cancelled_early
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&owner);
            streams
                .iter()
                .filter(|(key, _)| key.0 == Some(owner))
                .map(|(_, entry)| entry.token.clone())
                .collect()
        };
        for token in tokens {
            token.cancel();
        }
    }

    /// How many early cancels `owner` has remembered (tests).
    pub(crate) fn early_cancel_count_of(&self, owner: WorkspaceId) -> usize {
        self.cancelled_early
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&owner)
            .map_or(0, |early| early.cancelled.len())
    }

    /// The number of streams `owner` has running.
    pub(crate) fn stream_count_of(&self, owner: WorkspaceId) -> usize {
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .filter(|key| key.0 == Some(owner))
            .count()
    }

    pub fn running_stream_count(&self) -> usize {
        self.streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn register_stream(
        &self,
        key: StreamKey,
        connection_id: String,
    ) -> (CancellationToken, Arc<AtomicBool>, StreamGuard<'_>) {
        let token = CancellationToken::new();
        let closed = Arc::new(AtomicBool::new(false));
        let id = self.next_stream.fetch_add(1, Ordering::Relaxed);
        let entry = StreamEntry {
            id,
            connection_id,
            token: token.clone(),
            closed: closed.clone(),
        };
        let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(owner) = key.0 {
            let mut early = self
                .cancelled_early
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let early = early.entry(owner).or_default();
            // A reused id runs again; it's no longer "finished".
            take(&mut early.finished, &key.1);
            if take(&mut early.cancelled, &key.1) {
                // Nothing waits on the new token yet, so no waker runs here.
                token.cancel();
            }
        }
        streams.insert(key.clone(), entry);
        drop(streams);
        let guard = StreamGuard {
            streams: &self.streams,
            early: &self.cancelled_early,
            key,
            id,
        };
        (token, closed, guard)
    }
}

/// Removes a stream's cancellation token when the stream finishes or is
/// dropped. It only removes its own entry: if a newer stream reused the query
/// id, that one stays cancellable.
struct StreamGuard<'a> {
    streams: &'a StreamTokens,
    early: &'a EarlyCancelMap,
    key: StreamKey,
    id: u64,
}

impl Drop for StreamGuard<'_> {
    fn drop(&mut self) {
        let mut streams = self.streams.lock().unwrap_or_else(PoisonError::into_inner);
        if streams
            .get(&self.key)
            .is_some_and(|entry| entry.id == self.id)
        {
            streams.remove(&self.key);
            // Under the streams lock, like `cancel_stream_as`, so a cancel
            // sees either the running stream or the finished id.
            if let Some(owner) = self.key.0 {
                // Not after `close_all` forgot the workspace.
                let mut early = self.early.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(early) = early.get_mut(&owner) {
                    push_capped(&mut early.finished, &self.key.1);
                }
            }
        }
    }
}

/// One connection's calls: the dialect, queries and introspection. From
/// [`Workspace::engine`] it is checked against the workspace on every call,
/// so a call on a connection the workspace doesn't own (or that has since
/// closed) is `CONNECTION_NOT_FOUND`. From [`Core::connection_handle`] it
/// reaches any connection.
#[derive(Clone)]
pub struct ConnectionHandle<'a> {
    core: &'a Core,
    connection_id: String,
    owner: Option<WorkspaceId>,
}

impl ConnectionHandle<'_> {
    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }

    fn connection(&self) -> Result<Connection, DbError> {
        self.core.connection_as(&self.connection_id, self.owner)
    }

    fn driver(&self) -> Result<Arc<dyn Driver>, DbError> {
        Ok(self.connection()?.driver)
    }

    /// The engine that opened the connection.
    pub fn engine(&self) -> Result<Arc<dyn Engine>, DbError> {
        Ok(self.connection()?.engine)
    }

    /// See [`Core::with_dialect`].
    pub fn with_dialect<R>(&self, f: impl FnOnce(&dyn Dialect) -> R) -> Result<R, DbError> {
        let engine = self.engine()?;
        let dialect = engine
            .dialect()
            .ok_or_else(|| not_supported("The Rust SQL dialect"))?;
        Ok(f(dialect))
    }

    pub async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.query", connection_id = self.connection_id.as_str(), keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Query");
        self.driver()?.query(sql, params).await
    }

    pub async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.execute", connection_id = self.connection_id.as_str(), keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(); "Execute");
        self.driver()?.execute(sql, params).await
    }

    /// A failure names its statement where the driver knows it
    /// ([`TransactionError`]); an unknown connection names none. Success
    /// returns each statement's affected rows.
    pub async fn transaction(
        &self,
        statements: Vec<BatchStatement>,
    ) -> Result<Vec<u64>, TransactionError> {
        debug!(activity = "db.transaction", connection_id = self.connection_id.as_str(), statements = statements.len(); "Executing transaction");
        self.driver()?.transaction(statements).await
    }

    pub async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        debug!(activity = "db.list_schemas", connection_id = self.connection_id.as_str(); "List schemas");
        self.driver()?.list_schemas().await
    }

    pub async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        debug!(activity = "db.schema_tables", connection_id = self.connection_id.as_str(); "Schema tables");
        self.driver()?.schema_tables().await
    }

    pub async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        debug!(activity = "db.table_metadata", connection_id = self.connection_id.as_str(), schema = schema, table = table; "Table metadata");
        self.driver()?.table_metadata(schema, table).await
    }

    pub async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
        debug!(activity = "db.statistics", connection_id = self.connection_id.as_str(); "Statistics");
        self.driver()?.statistics().await
    }

    pub async fn explain(
        &self,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.explain", connection_id = self.connection_id.as_str(), keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(), analyze = analyze; "Explain");
        self.driver()?.explain(sql, params, analyze).await
    }

    /// A plain EXPLAIN (no ANALYZE) of one statement that must not change
    /// anything: the MCP server's `explain_query`. The SQL first goes
    /// through the same token check as a read-only query
    /// ([`QueryOptions::read_only`]), then must be a single statement (split
    /// on `;` under the engine's quoting; SQL Server, which needs no `;`
    /// between statements, is checked again from its plan), and then runs
    /// through [`Driver::explain_read_only`]: in the read-only transaction
    /// or session of the engine's read-only queries where planning can run
    /// user code (Postgres, MySQL/MariaDB), and as a plain EXPLAIN where it
    /// only compiles. Refusals are `READ_ONLY`.
    ///
    /// `timeout` is as [`QueryOptions::timeout`]; past it the call fails
    /// with `TIMEOUT`. Dropping the returned future cancels the EXPLAIN.
    pub async fn explain_read_only(
        &self,
        sql: &str,
        params: Vec<Value>,
        timeout: Option<Duration>,
    ) -> Result<ExplainResult, DbError> {
        let keyword = sql_keyword(sql);
        debug!(activity = "db.explain_read_only", connection_id = self.connection_id.as_str(), keyword = keyword.as_str(), sql_len = sql.len(), params = params.len(), timeout_ms = timeout.map(|t| t.as_millis() as u64); "Read-only explain");
        let connection = self.connection()?;
        check_read_only(sql, connection.sql_engine)?;
        check_one_statement(sql, connection.sql_engine)?;
        connection
            .driver
            .explain_read_only(sql, params, timeout)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_id_has_token_rules() {
        for (id, engine) in [
            ("postgres", SqlEngine::Postgres),
            ("mysql", SqlEngine::Mysql),
            ("sqlite", SqlEngine::Sqlite),
            ("mssql", SqlEngine::Mssql),
            ("duckdb", SqlEngine::Duckdb),
        ] {
            assert_eq!(sql_engine(id), Some(engine), "{id}");
        }
        // Every engine this build can open is covered.
        for id in with_default_plugins().build().engine_ids() {
            assert!(sql_engine(id).is_some(), "{id} has no token rules");
        }
    }

    #[test]
    fn with_plugins_registers_only_the_engines_it_allows() {
        let web = |id: &str| id != "sqlite" && id != "duckdb";
        let all = with_default_plugins().build().engine_ids();
        let some = with_plugins(web).build().engine_ids();
        let expected: Vec<_> = all.into_iter().filter(|id| web(id)).collect();
        assert_eq!(some, expected);
        assert!(with_plugins(|_| false).build().engine_ids().is_empty());
    }

    #[test]
    fn an_unknown_engine_id_is_refused_without_a_fallback() {
        for id in ["", "oracle", "mariadb", "Postgres"] {
            assert_eq!(sql_engine(id), None, "{id}");
            let err = check_read_only("SELECT 1", sql_engine(id)).unwrap_err();
            assert_eq!(err.code, "READ_ONLY");
            assert_eq!(err.message, seaquel_sql::read_only::READ_ONLY_MESSAGE);
        }
        assert!(check_read_only("SELECT 1", sql_engine("postgres")).is_ok());
    }
}
