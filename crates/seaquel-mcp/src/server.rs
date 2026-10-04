//! The server: the exposed connections, the lazily opened Core connections,
//! the per-call timeout, and the rmcp tool router.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use seaquel_core::ai::tools::mcp as args;
use seaquel_core::ai::tools::Call;
use seaquel_core::storage::connections;
use seaquel_core::{ConnectRequest, Core, HostKeyPolicy, Workspace};
use seaquel_types::storage::PersistedConnection;
use serde_json::Value as Json;
use tokio::sync::OnceCell;

use crate::duckdb_helper;
use crate::error::{ToolError, TIMEOUT};
use crate::exposed::{self, Exposed, Found, Selection, Sharing, AMBIGUOUS_CONNECTION};
use crate::tools;
use seaquel_core::secrets::SecretWait;

/// How long one tool call may take by default: connecting, then the query
/// or introspection call, less any time spent waiting on the secret store
/// ([`ServerOptions::with_secret_wait`]). Past it the call fails with
/// `TIMEOUT` and its query is cancelled.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Server settings. Build from `ServerOptions::default()`.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ServerOptions {
    /// The per-call timeout ([`DEFAULT_CALL_TIMEOUT`]).
    pub call_timeout: Duration,
    /// The version `initialize` reports (the desktop app's, from the CLI).
    pub version: String,
    /// Counts the time secret reads were pending, which the call timeout
    /// leaves out. `None`: secret reads count like the rest of the call.
    pub secret_wait: Option<Arc<SecretWait>>,
}

impl Default for ServerOptions {
    fn default() -> Self {
        Self {
            call_timeout: DEFAULT_CALL_TIMEOUT,
            version: env!("CARGO_PKG_VERSION").to_string(),
            secret_wait: None,
        }
    }
}

impl ServerOptions {
    #[must_use]
    pub fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// Leave the time secret reads are pending out of the call timeout. Pass
    /// the `SecretWait` whose [`SecretWait::watch`] wraps the workspace's
    /// secret store.
    #[must_use]
    pub fn with_secret_wait(mut self, wait: Arc<SecretWait>) -> Self {
        self.secret_wait = Some(wait);
        self
    }
}

/// The saved connections' rows and the global AI settings, read once, to
/// give several exposed connections' sharing flags.
pub(crate) struct SharingSnapshot {
    rows: Vec<PersistedConnection>,
    global: Sharing,
}

impl SharingSnapshot {
    /// `c`'s flags, or `CONNECTION_NOT_FOUND` when the app deleted it.
    pub(crate) fn get(&self, c: &Exposed) -> Result<Sharing, ToolError> {
        let row = self.rows.iter().find(|r| r.id == c.id).ok_or_else(|| {
            ToolError::new(
                exposed::CONNECTION_NOT_FOUND,
                format!("The connection {:?} was deleted in the app", c.name),
            )
        })?;
        Ok(exposed::sharing(row, self.global))
    }
}

pub(crate) struct Inner {
    pub(crate) core: Arc<Core>,
    pub(crate) workspace: Arc<Workspace>,
    pub(crate) exposed: Vec<Exposed>,
    pub(crate) options: ServerOptions,
    /// Core's connection id per exposed saved connection, opened on first
    /// use. A failed open leaves the cell empty, so the next call retries.
    open: Mutex<HashMap<String, Arc<OnceCell<String>>>>,
}

/// Seaquel's MCP server. Cheap to clone: clones share the open connections.
///
/// ```ignore
/// let server = McpServer::start(core, workspace, &selection, ServerOptions::default()).await?;
/// server.clone().serve(rmcp::transport::stdio()).await?.waiting().await?;
/// server.close().await;
/// ```
#[derive(Clone)]
pub struct McpServer {
    pub(crate) inner: Arc<Inner>,
    tool_router: ToolRouter<McpServer>,
}

impl McpServer {
    /// Resolve `selection` against the workspace's saved connections and
    /// projects (see [`exposed::resolve`]). Nothing is connected yet.
    ///
    /// Fails with `CONNECTION_NOT_FOUND`, `AMBIGUOUS_CONNECTION`,
    /// `PROJECT_NOT_FOUND`, `AMBIGUOUS_PROJECT` or a storage code.
    pub async fn start(
        core: Arc<Core>,
        workspace: Arc<Workspace>,
        selection: &Selection,
        options: ServerOptions,
    ) -> Result<Self, ToolError> {
        let exposed = exposed::resolve(workspace.storage(), selection).await?;
        log::info!("Exposing {} connection(s)", exposed.len());
        Ok(Self {
            inner: Arc::new(Inner {
                core,
                workspace,
                exposed,
                options,
                open: Mutex::default(),
            }),
            tool_router: Self::tool_router(),
        })
    }

    /// The tools as `tools/list` lists them, sorted by name. Their frozen
    /// copy is `seaquel-ai`'s `tests/fixtures/tool-schemas.json` (its `mcp`
    /// profile), which `tests/tool_schemas.rs` checks.
    pub fn tool_list() -> Vec<rmcp::model::Tool> {
        Self::tool_router().list_all()
    }

    /// The exposed connections, in storage order.
    pub fn exposed(&self) -> &[Exposed] {
        &self.inner.exposed
    }

    /// Disconnect every connection this server opened (which closes their
    /// SSH tunnels) and close the workspace. Call it once the transport has
    /// closed.
    pub async fn close(&self) {
        let ids: Vec<String> = {
            let open = self
                .inner
                .open
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            open.values()
                .filter_map(|cell| cell.get().cloned())
                .collect()
        };
        let count = ids.len();
        // At once: each can take up to 2 s (a DuckDB helper closing its
        // file), and the CLI's exit waits for them all.
        let closing = ids
            .iter()
            .map(|id| self.inner.workspace.disconnect(&self.inner.core, id));
        for result in futures::future::join_all(closing).await {
            if let Err(e) = result {
                log::warn!("Closing a connection failed: {e}");
            }
        }
        self.inner
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        self.inner.workspace.close().await;
        log::info!("Closed {count} connection(s) and the workspace");
    }

    /// How many Core connections this server has open.
    pub fn open_connection_count(&self) -> usize {
        self.inner
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|cell| cell.initialized())
            .count()
    }
}

impl Inner {
    /// The exposed connection `wanted` names: by id, else by name. Exact and
    /// case-sensitive, and only among the exposed set.
    pub(crate) fn resolve(&self, wanted: &str) -> Result<&Exposed, ToolError> {
        match exposed::lookup(&self.exposed, wanted, |c| &c.id, |c| &c.name) {
            Found::One(c) => Ok(c),
            Found::None => Err(ToolError::new(
                exposed::CONNECTION_NOT_FOUND,
                if self.exposed.is_empty() {
                    format!(
                        "No connection named {wanted:?} is available: this server exposes no \
                         connections. {}",
                        tools::NO_CONNECTIONS_HINT
                    )
                } else {
                    format!(
                        "No connection named {wanted:?} is available. Use a name or id from \
                         list_connections: {}",
                        self.exposed
                            .iter()
                            .map(|c| format!("{:?}", c.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                },
            )),
            Found::Many(many) => Err(ToolError::new(
                AMBIGUOUS_CONNECTION,
                format!(
                    "{} connections are named {wanted:?}; pass one of their ids instead: {}",
                    many.len(),
                    many.iter()
                        .map(|c| format!("{:?} (project {:?})", c.id, c.project_name))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        }
    }

    /// The connection's sharing flags now: its row and the global AI
    /// settings are re-read on every call, so a change in the app applies
    /// to the next call.
    pub(crate) async fn sharing(&self, c: &Exposed) -> Result<Sharing, ToolError> {
        self.sharing_snapshot().await?.get(c)
    }

    /// Every connection's sharing inputs, read once (for the tools that look
    /// at several connections).
    pub(crate) async fn sharing_snapshot(&self) -> Result<SharingSnapshot, ToolError> {
        let st = self.workspace.storage();
        Ok(SharingSnapshot {
            rows: connections::load_all(st).await?,
            global: exposed::global_sharing(st).await,
        })
    }

    /// Core's connection id for `c`, connecting it on first use: known SSH
    /// hosts only, and `restricted`, so a DuckDB instance can't reach files
    /// beyond its own database or install or load extensions, and its
    /// configuration is locked so SQL can't undo that.
    pub(crate) async fn connection(&self, c: &Exposed) -> Result<String, ToolError> {
        let cell = self
            .open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(c.id.clone())
            .or_default()
            .clone();
        let id = cell
            .get_or_try_init(|| async {
                log::info!("Connecting {:?}", c.name);
                let request = ConnectRequest::saved(&c.id)
                    .with_host_key(HostKeyPolicy::KnownOnly)
                    .with_restricted(true);
                self.workspace
                    .connect(&self.core, request)
                    .await
                    .map_err(|e| {
                        let e = ToolError::from(e);
                        duckdb_helper::connect_error(&self.core, c, &self.options.version, e)
                    })
            })
            .await?;
        Ok(id.clone())
    }

    /// Run `work` under the per-call timeout, which leaves out the time a
    /// secret read was pending (see `SecretWait`).
    ///
    /// On timeout `work` is dropped, and that is what cancels it: the
    /// registry's query stream ([`Workspace::query_stream`]) owned by `work`
    /// stops its driver when dropped, and a dropped `connect` closes the
    /// tunnel it opened. The call then fails with `TIMEOUT`.
    pub(crate) async fn timed<T>(
        &self,
        work: impl Future<Output = Result<T, ToolError>>,
    ) -> Result<T, ToolError> {
        let limit = self.options.call_timeout;
        let wait = self.options.secret_wait.as_deref();
        let start = Instant::now();
        let waited_before = wait.map_or(Duration::ZERO, SecretWait::waited);
        let mut deadline = start + limit;
        tokio::pin!(work);
        loop {
            tokio::select! {
                result = &mut work => return result,
                () = tokio::time::sleep_until(deadline.into()) => {}
            }
            let Some(wait) = wait else { break };
            let now = Instant::now();
            if let Some(pending) = wait.pending_for() {
                if pending >= wait.limit() {
                    return Err(ToolError::new(
                        TIMEOUT,
                        format!(
                            "The call waited more than {} s to read the connection's saved \
                             password and was cancelled. On a Mac, a keychain prompt from \
                             seaquel-cli may be waiting for an answer: allow it, then call again.",
                            wait.limit().as_secs()
                        ),
                    ));
                }
                // Look again when the read may have ended or passed its limit.
                deadline = now + (wait.limit() - pending).min(SECRET_WAIT_POLL);
                continue;
            }
            let excluded = wait.waited().saturating_sub(waited_before);
            deadline = start + limit + excluded;
            if deadline <= now {
                break;
            }
        }
        Err(ToolError::new(
            TIMEOUT,
            format!(
                "The call took longer than {} s and was cancelled",
                limit.as_secs_f64()
            ),
        ))
    }
}

/// How often a timed-out call looks again while a secret read is pending.
const SECRET_WAIT_POLL: Duration = Duration::from_millis(250);

/// A tool's JSON result as one compact text block.
fn ok(value: Json) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(value.to_string())])
}

fn reply(result: Result<Json, ToolError>) -> CallToolResult {
    match result {
        Ok(value) => ok(value),
        Err(e) => {
            log::debug!("Tool error {}", e.code);
            e.into_result()
        }
    }
}

#[tool_router]
impl McpServer {
    /// List the database connections this server exposes, with their
    /// engine, project and whether schema and data may be shared.
    #[tool(
        name = "list_connections",
        annotations(
            title = "List connections",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_connections(&self) -> CallToolResult {
        reply(tools::list_connections(&self.inner).await)
    }

    /// List the schemas of a connection.
    #[tool(
        name = "list_schemas",
        annotations(title = "List schemas", read_only_hint = true, open_world_hint = false)
    )]
    async fn list_schemas(
        &self,
        Parameters(a): Parameters<args::ConnectionArgs>,
    ) -> CallToolResult {
        reply(tools::on_connection(&self.inner, Call::from(a)).await)
    }

    /// List the tables and views of a connection, optionally of one schema.
    #[tool(
        name = "list_tables",
        annotations(title = "List tables", read_only_hint = true, open_world_hint = false)
    )]
    async fn list_tables(&self, Parameters(a): Parameters<args::ListTablesArgs>) -> CallToolResult {
        reply(tools::on_connection(&self.inner, Call::from(a)).await)
    }

    /// Describe a table: its columns (name, type, nullable, default, primary
    /// key), indexes and foreign keys.
    #[tool(
        name = "describe_table",
        annotations(
            title = "Describe table",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn describe_table(
        &self,
        Parameters(a): Parameters<args::DescribeTableArgs>,
    ) -> CallToolResult {
        reply(tools::on_connection(&self.inner, Call::from(a)).await)
    }

    /// Run one read-only SQL query (SELECT and the like) and return its rows.
    /// Writes are refused. At most `max_rows` rows come back (default 100, at
    /// most 1000); `truncated` says the query had more.
    #[tool(
        name = "run_query",
        annotations(
            title = "Run read-only query",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn run_query(&self, Parameters(a): Parameters<args::RunQueryArgs>) -> CallToolResult {
        reply(tools::on_connection(&self.inner, Call::from(a)).await)
    }

    /// Show the database's query plan for a read-only query, without running
    /// it.
    #[tool(
        name = "explain_query",
        annotations(
            title = "Explain query",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn explain_query(&self, Parameters(a): Parameters<args::ExplainArgs>) -> CallToolResult {
        reply(tools::on_connection(&self.inner, Call::from(a)).await)
    }

    /// List the saved queries of the exposed connections' projects, with
    /// their `{{parameters}}`.
    #[tool(
        name = "list_saved_queries",
        annotations(
            title = "List saved queries",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_saved_queries(
        &self,
        Parameters(a): Parameters<args::ListSavedQueriesArgs>,
    ) -> CallToolResult {
        reply(
            tools::list_saved_queries(&self.inner, a.connection.as_deref(), a.project.as_deref())
                .await,
        )
    }

    /// Run a saved query on a connection of its project, with values for its
    /// `{{parameters}}`. Read-only, like run_query.
    #[tool(
        name = "run_saved_query",
        annotations(
            title = "Run saved query",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    async fn run_saved_query(
        &self,
        Parameters(a): Parameters<args::RunSavedQueryArgs>,
    ) -> CallToolResult {
        reply(tools::on_connection(&self.inner, Call::from(a)).await)
    }
}

/// What `initialize` tells the host about the server.
pub const INSTRUCTIONS: &str = "Seaquel exposes the database connections the user chose when \
starting this server. Everything is read-only: run_query and run_saved_query refuse writes, \
DDL and anything else that could change data. Start with list_connections; pass a connection's \
name or id to the other tools. A connection may not share its schema or its data with AI \
tools; those tools then fail with SCHEMA_SHARING_OFF or DATA_SHARING_OFF, which the user can \
change in the Seaquel app. Results hold at most 1000 rows and about 4 MB, and a cell's text is \
cut at 64 KB (such a cell is an object with \"truncated\": true).";

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "seaquel",
                self.inner.options.version.clone(),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}
