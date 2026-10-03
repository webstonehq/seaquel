//! The `db` group of the workspace RPC: connect, test, disconnect, queries,
//! engine calls and cancel, on the workspace's own connections (phase 5a),
//! plus [`CoreEvent`], what the stream transports push to a GUI.
//!
//! Wire shape, like the other groups:
//!
//! ```json
//! {"method":"db","params":{"method":"query","params":{"connectionId":"c","sql":"SELECT 1","params":[]}}}
//! {"method":"db","result":{"method":"query","result":{"columns":["?column?"],"rows":[[1]]}}}
//! ```
//!
//! Request fields are camelCase. Query parameters are the tagged `Value`
//! wire format (`encodeParams` in `src/lib/values.ts`): plain JSON, or
//! `{"$sq": kind, "v": …}`. Results reuse the engine types as they are
//! (`QueryResult`, `ExecuteResult`, `EngineResponse`, and `StreamEvent` in
//! [`CoreEvent::Stream`]). **`ExecuteResult` (`rows_affected`,
//! `last_insert_id`) and `StreamBatch` (`is_final`) stay snake_case on
//! purpose**: they're the shapes the old `db_*` commands and `/api/db/*`
//! sent, and the TypeScript that reads them (`CoreProvider`) kept them.
//!
//! Every call goes through the [`Workspace`], so a connection or stream the
//! workspace doesn't own is `CONNECTION_NOT_FOUND`, the same as one that
//! doesn't exist.
//!
//! `queryStream`, `run`, `page` and `tablePage` aren't request/response
//! calls: [`dispatch_stream`] serves them as streams of [`CoreEvent`]s (the
//! desktop's `core_stream`, the web's `/rpc/stream`), and
//! [`crate::dispatch_workspace`] refuses them. `run` is the editor's run
//! (`Workspace::run`, phase 5b), `page` re-pages one of its statements, and
//! `tablePage` is a data tab's page (`Workspace::table_page`, phase 5c);
//! their events are [`CoreEvent::Run`]s. Their types live in
//! `seaquel_core::domain::run` and `…::edits` and exist in every build;
//! without the `workspace` feature (the browser build) they answer
//! `NOT_SUPPORTED`.
//!
//! The grid's edits (phase 5c) are unary: `planEdits` (the pending-changes
//! queue's display fields for some edit intents), `applyChanges` (the queue
//! applied: one change, one transaction for DML only, or in order; its
//! refusals and failures are the outcome's `failed`, not an error) and
//! `duckdbExtension` (the DuckDB extensions tab's actions). Without the
//! `workspace` feature they answer `NOT_SUPPORTED` too.
//!
//! Whether `connect` and `test` may connect at all, and to what, is Core's
//! `ConnectPolicy`: a Core built without one answers `NOT_SUPPORTED`. Without
//! this crate's `workspace` feature (the browser build) both answer
//! `NOT_SUPPORTED` too.

use futures::StreamExt;
use seaquel_core::domain::ai::AiEvent;
use seaquel_core::domain::edits::{
    ApplyChangesParams, ApplyOutcome, ExtensionAction, PlanEditsParams, PlannedChange,
    TablePageParams,
};
use seaquel_core::domain::run::{PageParams, RunEvent, RunParams};
use seaquel_core::{
    ChangeSeq, Core, QueryOptions, StoredKind, Workspace, WorkspaceEvent, WriteOrigin,
};
use seaquel_engine::BoxStream;
use seaquel_types::connect::{ConnectionForm, SuppliedSecrets};
use seaquel_types::{BatchStatement, ExecuteResult, QueryResult, StreamEvent, Value};
use serde::{Deserialize, Serialize};

use crate::workspace::RpcError;
use crate::{dispatch_on, EngineRequest, EngineResponse};

pub use seaquel_core::{
    CONNECTION_CLOSED, CONNECTION_REPLACED, TUNNEL_CLOSED, WINDOW_CLOSED, WORKSPACE_EVICTED,
};

// ── Requests ──

/// The most ids one `db.alive` takes. A web user holds at most 16
/// connections; a page shows at most its saved ones.
pub const MAX_ALIVE_IDS: usize = 1_000;

/// A `db` call. `Debug` never shows a secret or a connection string
/// (see [`ConnectParams`]).
#[allow(clippy::large_enum_variant)] // See `Request`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum DbRequest {
    /// Open a connection owned by the workspace; the result is its id.
    Connect(ConnectParams),
    /// Open and close a connection without keeping it ("Test connection").
    Test(ConnectParams),
    Disconnect {
        connection_id: String,
    },
    /// Record that an open connection was opened for a saved connection
    /// (phase 6 Task 7): the page's `add` connects a form before Core has
    /// made the row. Once per connection; `null`.
    BindSaved {
        connection_id: String,
        saved_connection_id: String,
    },
    Query {
        connection_id: String,
        sql: String,
        /// Absent is none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "unknown[]"))]
        params: Option<Vec<Value>>,
    },
    Execute {
        connection_id: String,
        sql: String,
        /// Absent is none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "unknown[]"))]
        params: Option<Vec<Value>>,
    },
    Transaction {
        connection_id: String,
        statements: Vec<BatchStatement>,
    },
    /// A dialect or introspection call (`EngineRequest`, whose own fields
    /// keep their Rust names).
    Engine {
        connection_id: String,
        request: EngineRequest,
    },
    /// Which of `connectionIds` this workspace still holds (phase 6 probe
    /// F4 review I1): a page whose event channel was down (a sleeping
    /// laptop past the web's window grace) checks the connections it shows
    /// and reconnects the ones Core closed. Answers only ids of this
    /// workspace's open connections, in the order asked; at most
    /// [`MAX_ALIVE_IDS`] ids (`INVALID_ARGUMENT`).
    Alive {
        connection_ids: Vec<String>,
    },
    /// Cancel the workspace's stream `streamId`. Another workspace's stream
    /// with the same id isn't touched; an unknown or finished id is ignored.
    Cancel {
        stream_id: String,
    },
    /// Only through [`dispatch_stream`].
    QueryStream(QueryStreamParams),
    /// The editor's run: the text, a target, parameter values and the page
    /// size. Only through [`dispatch_stream`]. `Debug` shows no text, value
    /// or history context.
    Run(RunParams),
    /// One statement of a run again, from the `source` its `statementStart`
    /// carried: another page, or the whole result at page size 0. Only
    /// through [`dispatch_stream`]. `Debug` shows no SQL or values.
    Page(PageParams),
    /// The queue entries' SQL, binds, query type and summary for some edit
    /// intents; runs nothing. `Debug` shows counts only.
    PlanEdits(PlanEditsParams),
    /// Apply the pending-changes queue. `Debug` shows counts only.
    ApplyChanges(ApplyChangesParams),
    /// One page of a data tab: Core builds the SELECT from the typed query.
    /// Only through [`dispatch_stream`]. `Debug` shows the table, counts and
    /// the page, never filter values.
    TablePage(TablePageParams),
    /// An action of the DuckDB extensions tab on a DuckDB connection.
    DuckdbExtension {
        connection_id: String,
        action: ExtensionAction,
    },
}

impl DbRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            DbRequest::Connect(_) => "connect",
            DbRequest::Test(_) => "test",
            DbRequest::Disconnect { .. } => "disconnect",
            DbRequest::BindSaved { .. } => "bindSaved",
            DbRequest::Query { .. } => "query",
            DbRequest::Execute { .. } => "execute",
            DbRequest::Transaction { .. } => "transaction",
            DbRequest::Engine { .. } => "engine",
            DbRequest::Alive { .. } => "alive",
            DbRequest::Cancel { .. } => "cancel",
            DbRequest::QueryStream(_) => "queryStream",
            DbRequest::Run(_) => "run",
            DbRequest::Page(_) => "page",
            DbRequest::PlanEdits(_) => "planEdits",
            DbRequest::ApplyChanges(_) => "applyChanges",
            DbRequest::TablePage(_) => "tablePage",
            DbRequest::DuckdbExtension { .. } => "duckdbExtension",
        }
    }

    /// The `streamId` of a stream request (`queryStream`, `run`, `page`,
    /// `tablePage`): what its events carry and `cancel` takes. `None` for
    /// the rest.
    pub fn stream_id(&self) -> Option<&str> {
        match self {
            DbRequest::QueryStream(params) => Some(&params.stream_id),
            DbRequest::Run(params) => Some(&params.stream_id),
            DbRequest::Page(params) => Some(&params.stream_id),
            DbRequest::TablePage(params) => Some(&params.stream_id),
            _ => None,
        }
    }

    /// Whether its events are [`CoreEvent::Run`]s (`run`, `page`,
    /// `tablePage`) rather than [`CoreEvent::Stream`]s.
    pub fn is_run(&self) -> bool {
        Self::is_run_method(self.method())
    }

    /// Whether `method` names a request whose events are
    /// [`CoreEvent::Run`]s, for a transport that has only the name (a
    /// request it couldn't parse still gets its refusal as a run event).
    pub fn is_run_method(method: &str) -> bool {
        matches!(method, "run" | "page" | "tablePage")
    }
}

/// What `connect` and `test` take: Core's `ConnectRequest` without
/// `restricted` (the MCP server's, never the GUI's).
///
/// `Debug` shows no secret: `SuppliedSecrets` and `ConnectionForm` (its
/// connection string) redact themselves.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConnectParams {
    pub target: ConnectTargetParams,
    /// Typed into the form, or (web) decrypted from the vault. Each one
    /// supplied wins over the secret store.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<SuppliedSecrets>", optional))]
    pub secrets: SuppliedSecrets,
    /// The SSH host key fingerprint (`SHA256:…`) the user approved in the
    /// trust prompt after an `UNKNOWN_HOST_KEY`. Absent: known hosts only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub trust_host_key: Option<String>,
    /// SQLite: create the file if it doesn't exist.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub create_if_missing: bool,
    /// The saved connection this connection is for (phase 6, Decision 6):
    /// an assistant turn runs its tools only on a connection opened for
    /// the chat's saved connection (`CONNECTION_MISMATCH`). A saved target
    /// records its own id without it; naming another is `INVALID_ARGUMENT`.
    /// A form connect (the edit tab's reconnect) names the row it edits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub saved_connection_id: Option<String>,
}

/// What to connect: `{"type":"saved","id":…}` (a stored connection, secrets
/// from the caller first, then the store) or `{"type":"form","form":{…}}`
/// (the form the user filled in, never read from storage).
#[allow(clippy::large_enum_variant)] // See `Request`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ConnectTargetParams {
    Saved { id: String },
    Form { form: ConnectionForm },
}

/// A streaming query. `readOnly` runs it through the read-only check and
/// the database's read-only mode (the AI, dashboards); `maxRows`,
/// `maxBytes` and `timeoutMs` are only valid with it (else the stream ends
/// with `INVALID_OPTIONS`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueryStreamParams {
    pub connection_id: String,
    /// The caller's id for this stream; the events carry it and `cancel`
    /// takes it. **It must be unique per stream** (a fresh UUID each time),
    /// not just among running ones: Core remembers recent ids per workspace
    /// so that a `cancel` sent before the start still cancels, and one sent
    /// after the stream finished is dropped. Reusing an id can cancel a new
    /// stream with an old cancel.
    pub stream_id: String,
    pub sql: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "unknown[]"))]
    pub params: Vec<Value>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub max_rows: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub max_bytes: Option<u64>,
    /// Milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub timeout_ms: Option<u64>,
}

impl QueryStreamParams {
    fn options(&self) -> QueryOptions {
        let size = |n: Option<u64>| n.map(|n| usize::try_from(n).unwrap_or(usize::MAX));
        QueryOptions::default()
            .with_read_only(self.read_only)
            .with_max_rows(size(self.max_rows))
            .with_max_bytes(size(self.max_bytes))
            .with_timeout(self.timeout_ms.map(std::time::Duration::from_millis))
    }
}

// ── Stream kinds ──

/// Which of the three stream kinds a request is (phase 6): its events are
/// [`CoreEvent::Stream`]s (`db.queryStream`), [`CoreEvent::Run`]s
/// (`db.run`, `db.page`, `db.tablePage`) or [`CoreEvent::Ai`]s
/// (`ai.chat`). The transports' stream lists are this one function
/// ([`crate::Request::stream_kind`]), so a new stream kind is one line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Stream,
    Run,
    Ai,
}

impl StreamKind {
    /// The kind of the request `group.method`, `None` for one that isn't a
    /// stream. For a transport that has only the names (a frame it couldn't
    /// parse still gets its refusal as an event of the right kind).
    pub fn of(group: &str, method: &str) -> Option<Self> {
        match (group, method) {
            ("db", "queryStream") => Some(StreamKind::Stream),
            ("db", m) if DbRequest::is_run_method(m) => Some(StreamKind::Run),
            ("ai", "chat") => Some(StreamKind::Ai),
            _ => None,
        }
    }
}

// ── Responses ──

/// A `db` call's result, as `{"method": …, "result": …}`. Calls that return
/// nothing have `"result": null`.
// Not `Deserialize`: results only go out (the edit outcomes can't be read).
#[derive(Debug, Serialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum DbResponse {
    Connect(Connected),
    Test(()),
    Disconnect(()),
    BindSaved(()),
    Query(QueryResult),
    Execute(ExecuteResult),
    Transaction(()),
    Engine(EngineResponse),
    /// The ids asked that are still open.
    Alive(Vec<String>),
    Cancel(()),
    PlanEdits(Vec<PlannedChange>),
    ApplyChanges(ApplyOutcome),
    /// `list`'s rows; `null` for the other actions.
    DuckdbExtension(Option<QueryResult>),
}

/// `connect`'s result: Core's id for the new connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Connected {
    pub connection_id: String,
}

// ── Events ──

/// What a stream transport pushes to a GUI: `{"type":"stream",…}` for a
/// query stream's events, `{"type":"run",…}` for a run's or a page's,
/// `{"type":"connectionClosed",…}` when a connection went away without the
/// GUI asking, and `{"type":"storageChanged",…}` after every stored write.
///
/// `Debug` shows no rows or SQL for a run event (`RunEvent`'s is by hand)
/// and no origin for a `storageChanged` one; transports still never log
/// events.
#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum CoreEvent {
    /// One event of stream `streamId`: batches, then one `done` or `error`
    /// (nothing more after a cancel).
    Stream {
        stream_id: String,
        event: StreamEvent,
    },
    /// `code` is [`WORKSPACE_EVICTED`] (the web server closed the
    /// workspace's connections), [`WINDOW_CLOSED`] (the web server closed
    /// the connections of a tab that stayed closed) or
    /// [`CONNECTION_REPLACED`] (the tab that opened it connected the same
    /// saved connection again). [`CONNECTION_CLOSED`] (lost) and
    /// [`TUNNEL_CLOSED`] are reserved: Core doesn't detect those yet. A
    /// page ignores one for a connection it doesn't hold.
    ConnectionClosed {
        connection_id: String,
        code: String,
        message: String,
    },
    /// One event of run, page or table page `streamId`: per statement a
    /// `statementStart`, its `batch`es (`StreamBatch` flattened, snake_case,
    /// as in `stream`) and `statementDone`/`statementError`, then one `done`
    /// or `error` (nothing more after a cancel).
    Run { stream_id: String, event: RunEvent },
    /// One event of assistant turn `streamId` (phase 6, Decision 11):
    /// `started`, `text`, tool calls, `approvalRequired` and `clientTool`,
    /// then one `done` or `error` (nothing more after a cancel).
    Ai { stream_id: String, event: AiEvent },
    /// A stored write committed (phase 5d, Decision 16): what changed, never
    /// a value. `scope` is the project, connection or chat the `ids` belong
    /// to; `ids` `null` means reload the kind within the scope. `origin` is
    /// the writer's window or tab (`null` for Core's own writes): a window
    /// ignores its own. `seq` orders it against write and list results
    /// (Decision 17). Every one of the workspace's windows or tabs gets it.
    StorageChanged {
        kind: StoredKind,
        scope: Option<String>,
        ids: Option<Vec<String>>,
        origin: Option<String>,
        seq: ChangeSeq,
    },
}

impl std::fmt::Debug for CoreEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreEvent::Stream { stream_id, event } => f
                .debug_struct("Stream")
                .field("stream_id", stream_id)
                .field("event", event)
                .finish(),
            CoreEvent::ConnectionClosed {
                connection_id,
                code,
                message,
            } => f
                .debug_struct("ConnectionClosed")
                .field("connection_id", connection_id)
                .field("code", code)
                .field("message", message)
                .finish(),
            CoreEvent::Run { stream_id, event } => f
                .debug_struct("Run")
                .field("stream_id", stream_id)
                .field("event", event)
                .finish(),
            CoreEvent::Ai { stream_id, event } => f
                .debug_struct("Ai")
                .field("stream_id", stream_id)
                .field("event", event)
                .finish(),
            CoreEvent::StorageChanged {
                kind,
                scope,
                ids,
                origin,
                seq,
            } => f
                .debug_struct("StorageChanged")
                .field("kind", kind)
                .field("scope", scope)
                .field("ids", ids)
                .field("origin", &origin.as_ref().map(|_| "<set>"))
                .field("seq", seq)
                .finish(),
        }
    }
}

impl CoreEvent {
    /// The stream or run it belongs to; `None` for `connectionClosed`.
    pub fn stream_id(&self) -> Option<&str> {
        match self {
            CoreEvent::Stream { stream_id, .. }
            | CoreEvent::Run { stream_id, .. }
            | CoreEvent::Ai { stream_id, .. } => Some(stream_id),
            CoreEvent::ConnectionClosed { .. } | CoreEvent::StorageChanged { .. } => None,
        }
    }

    /// A stream's or run's `done` or `error`: nothing more comes for its
    /// `streamId`.
    pub fn is_terminal(&self) -> bool {
        match self {
            CoreEvent::Stream { event, .. } => {
                matches!(event, StreamEvent::Done | StreamEvent::Error { .. })
            }
            CoreEvent::Run { event, .. } => event.is_terminal(),
            CoreEvent::Ai { event, .. } => event.is_terminal(),
            CoreEvent::ConnectionClosed { .. } | CoreEvent::StorageChanged { .. } => false,
        }
    }

    /// A terminal `error` for stream `stream_id`, shaped for the request it
    /// answers ([`StreamKind`]): a `stream`, `run` or `ai` event. The
    /// transports use it for a request they can't serve, and the web for a
    /// stream that ended without a terminal event.
    pub fn error(
        stream_id: &str,
        kind: StreamKind,
        code: &str,
        message: impl Into<String>,
    ) -> Self {
        let stream_id = stream_id.to_string();
        match kind {
            StreamKind::Run => CoreEvent::Run {
                stream_id,
                event: RunEvent::error(code, message),
            },
            StreamKind::Ai => CoreEvent::Ai {
                stream_id,
                event: AiEvent::error(code, message),
            },
            StreamKind::Stream => CoreEvent::Stream {
                stream_id,
                event: StreamEvent::Error {
                    message: message.into(),
                    code: code.to_string(),
                },
            },
        }
    }

    /// The wire event for a workspace event, if it has one.
    fn from_workspace(event: WorkspaceEvent) -> Option<Self> {
        match event {
            WorkspaceEvent::ConnectionClosed {
                connection_id,
                code,
                message,
            } => Some(CoreEvent::ConnectionClosed {
                connection_id,
                code,
                message,
            }),
            WorkspaceEvent::StorageChanged(change) => Some(CoreEvent::StorageChanged {
                kind: change.kind,
                scope: change.scope,
                ids: change.ids,
                origin: change.origin,
                seq: change.seq,
            }),
            #[allow(unreachable_patterns)] // `WorkspaceEvent` is non_exhaustive.
            _ => None,
        }
    }
}

// ── Dispatch ──

/// Serve a `db` call (not a stream) through `ws`.
pub(crate) async fn db(
    core: &Core,
    ws: &Workspace,
    req: DbRequest,
    origin: &WriteOrigin,
) -> Result<DbResponse, RpcError> {
    Ok(match req {
        DbRequest::Connect(params) => DbResponse::Connect(Connected {
            connection_id: connect(core, ws, params, origin).await?,
        }),
        DbRequest::Test(params) => DbResponse::Test(test(core, ws, params).await?),
        DbRequest::Disconnect { connection_id } => {
            DbResponse::Disconnect(ws.disconnect(core, &connection_id).await?)
        }
        DbRequest::BindSaved {
            connection_id,
            saved_connection_id,
        } => DbResponse::BindSaved(
            ws.bind_saved_connection(core, &connection_id, &saved_connection_id)
                .await?,
        ),
        DbRequest::Query {
            connection_id,
            sql,
            params,
        } => DbResponse::Query(
            ws.query(core, &connection_id, &sql, params.unwrap_or_default())
                .await?,
        ),
        DbRequest::Execute {
            connection_id,
            sql,
            params,
        } => DbResponse::Execute(
            ws.execute(core, &connection_id, &sql, params.unwrap_or_default())
                .await?,
        ),
        DbRequest::Transaction {
            connection_id,
            statements,
        } => DbResponse::Transaction(
            // The wire keeps the error alone; the index is for Core's callers.
            ws.transaction(core, &connection_id, statements)
                .await
                .map(|_| ())
                .map_err(|e| e.error)?,
        ),
        DbRequest::Engine {
            connection_id,
            request,
        } => {
            let handle = ws.engine(core, &connection_id)?;
            DbResponse::Engine(dispatch_on(&handle, request).await?)
        }
        DbRequest::Alive { connection_ids } => {
            if connection_ids.len() > MAX_ALIVE_IDS {
                return Err(RpcError::invalid_argument(format!(
                    "db.alive takes at most {MAX_ALIVE_IDS} connection ids"
                )));
            }
            DbResponse::Alive(ws.alive(core, &connection_ids))
        }
        DbRequest::Cancel { stream_id } => {
            ws.cancel(core, &stream_id);
            DbResponse::Cancel(())
        }
        DbRequest::PlanEdits(params) => DbResponse::PlanEdits(plan_edits(core, ws, params).await?),
        DbRequest::ApplyChanges(params) => {
            DbResponse::ApplyChanges(apply_changes(core, ws, params, origin).await?)
        }
        DbRequest::DuckdbExtension {
            connection_id,
            action,
        } => DbResponse::DuckdbExtension(duckdb_extension(core, ws, &connection_id, action).await?),
        req @ (DbRequest::QueryStream(_)
        | DbRequest::Run(_)
        | DbRequest::Page(_)
        | DbRequest::TablePage(_)) => {
            return Err(RpcError::invalid_argument(format!(
                "db.{} is served by the stream transport (core_stream, /rpc/stream), \
                 not as a single call",
                req.method()
            )))
        }
    })
}

#[cfg(feature = "workspace")]
fn connect_request(params: ConnectParams) -> seaquel_core::ConnectRequest {
    use seaquel_core::{ConnectRequest, HostKeyPolicy};

    let req = match params.target {
        ConnectTargetParams::Saved { id } => ConnectRequest::saved(id),
        ConnectTargetParams::Form { form } => ConnectRequest::form(form),
    };
    let host_key = match params.trust_host_key.filter(|fp| !fp.is_empty()) {
        Some(fingerprint) => HostKeyPolicy::Trust(fingerprint),
        None => HostKeyPolicy::KnownOnly,
    };
    let req = req
        .with_secrets(params.secrets)
        .with_host_key(host_key)
        .with_create_if_missing(params.create_if_missing);
    req.with_saved_connection_id(params.saved_connection_id)
}

/// The window `origin` is recorded on the connection (phase 6 probe F4):
/// its new connection for a saved connection replaces its older ones, and
/// the web server closes a closed window's connections.
#[cfg(feature = "workspace")]
async fn connect(
    core: &Core,
    ws: &Workspace,
    params: ConnectParams,
    origin: &WriteOrigin,
) -> Result<String, RpcError> {
    Ok(ws
        .connect(core, connect_request(params).with_origin(origin.clone()))
        .await?)
}

#[cfg(feature = "workspace")]
async fn test(core: &Core, ws: &Workspace, params: ConnectParams) -> Result<(), RpcError> {
    Ok(ws.test(core, connect_request(params)).await?)
}

#[cfg(not(feature = "workspace"))]
async fn connect(
    _: &Core,
    _: &Workspace,
    _: ConnectParams,
    _: &WriteOrigin,
) -> Result<String, RpcError> {
    Err(RpcError::not_supported("Connecting"))
}

#[cfg(not(feature = "workspace"))]
async fn test(_: &Core, _: &Workspace, _: ConnectParams) -> Result<(), RpcError> {
    Err(RpcError::not_supported("Connecting"))
}

#[cfg(feature = "workspace")]
async fn plan_edits(
    core: &Core,
    ws: &Workspace,
    params: PlanEditsParams,
) -> Result<Vec<PlannedChange>, RpcError> {
    Ok(ws.plan_edits(core, params).await?)
}

#[cfg(feature = "workspace")]
async fn apply_changes(
    core: &Core,
    ws: &Workspace,
    params: ApplyChangesParams,
    origin: &WriteOrigin,
) -> Result<ApplyOutcome, RpcError> {
    Ok(ws.apply_changes_from(core, params, origin).await?)
}

#[cfg(feature = "workspace")]
async fn duckdb_extension(
    core: &Core,
    ws: &Workspace,
    connection_id: &str,
    action: ExtensionAction,
) -> Result<Option<QueryResult>, RpcError> {
    Ok(ws.duckdb_extension(core, connection_id, action).await?)
}

#[cfg(not(feature = "workspace"))]
async fn plan_edits(
    _: &Core,
    _: &Workspace,
    _: PlanEditsParams,
) -> Result<Vec<PlannedChange>, RpcError> {
    Err(RpcError::not_supported("Editing"))
}

#[cfg(not(feature = "workspace"))]
async fn apply_changes(
    _: &Core,
    _: &Workspace,
    _: ApplyChangesParams,
    _: &WriteOrigin,
) -> Result<ApplyOutcome, RpcError> {
    Err(RpcError::not_supported("Editing"))
}

#[cfg(not(feature = "workspace"))]
async fn duckdb_extension(
    _: &Core,
    _: &Workspace,
    _: &str,
    _: ExtensionAction,
) -> Result<Option<QueryResult>, RpcError> {
    Err(RpcError::not_supported("DuckDB extensions"))
}

/// Start a stream request on `ws`, its events tagged with its `streamId`:
///
/// - `db.queryStream`: [`CoreEvent::Stream`]s, ending after `done` or
///   `error`;
/// - `db.run`, `db.page` and `db.tablePage`: [`CoreEvent::Run`]s, ending
///   after the run's `done` or `error`;
/// - `ai.chat` (phase 6): [`CoreEvent::Ai`]s, ending after the turn's
///   `done` or `error`. **Stop a turn with `db.cancel` and keep polling
///   it to its end**: the turn then stores the reply with what streamed.
///   Dropping the stream drops the turn where it is, without the reply's
///   write.
///
/// Each ends with nothing more after a cancel. A connection `ws` doesn't
/// own gives one `CONNECTION_NOT_FOUND` error event. The stream borrows `ws`
/// (a run appends its history row through it).
///
/// `origin` is the window or tab that started it: a run's history row's
/// `StorageChanged` event carries it.
///
/// Anything else is `INVALID_ARGUMENT`; `run`, `page` and `tablePage`
/// without the `workspace` feature, and `ai.chat` without `ai`, are
/// `NOT_SUPPORTED`. The desktop's `core_stream` and
/// the web's `/rpc/stream` call it with a request parsed by
/// [`crate::parse_request`]. Logs the method name only.
pub fn dispatch_stream<'a>(
    core: &'a Core,
    ws: &'a Workspace,
    req: crate::Request,
    origin: WriteOrigin,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    let (group, method) = (req.group(), req.method());
    log::debug!(activity = "rpc.stream", group = group, method = method; "Workspace stream");
    let params = match req {
        crate::Request::Db(DbRequest::QueryStream(params)) => params,
        crate::Request::Db(DbRequest::Run(params)) => return run(core, ws, params, origin),
        crate::Request::Db(DbRequest::Page(params)) => return page(core, ws, params),
        crate::Request::Db(DbRequest::TablePage(params)) => return table_page(core, ws, params),
        crate::Request::Ai(crate::AiRequest::Chat(params)) => {
            return crate::ai::chat(core, ws, params, origin)
        }
        _ => {
            log::debug!(activity = "rpc.stream", group = group, method = method, code = crate::INVALID_ARGUMENT; "Workspace stream refused");
            return Err(RpcError::invalid_argument(format!(
                "{group}.{method} isn't a stream; only db.queryStream, db.run, db.page, \
                 db.tablePage and ai.chat are"
            )));
        }
    };
    let options = params.options();
    let stream_id = params.stream_id.clone();
    let events = ws.query_stream(
        core,
        params.stream_id,
        params.connection_id,
        params.sql,
        params.params,
        options,
    );
    Ok(Box::pin(events.map(move |event| CoreEvent::Stream {
        stream_id: stream_id.clone(),
        event,
    })))
}

#[cfg(feature = "workspace")]
fn tag_run<'a>(stream_id: String, events: BoxStream<'a, RunEvent>) -> BoxStream<'a, CoreEvent> {
    Box::pin(events.map(move |event| CoreEvent::Run {
        stream_id: stream_id.clone(),
        event,
    }))
}

#[cfg(feature = "workspace")]
fn run<'a>(
    core: &'a Core,
    ws: &'a Workspace,
    params: RunParams,
    origin: WriteOrigin,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    let stream_id = params.stream_id.clone();
    Ok(tag_run(stream_id, ws.run_from(core, params, origin)))
}

#[cfg(feature = "workspace")]
fn page<'a>(
    core: &'a Core,
    ws: &'a Workspace,
    params: PageParams,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    let stream_id = params.stream_id.clone();
    Ok(tag_run(stream_id, ws.page(core, params)))
}

#[cfg(feature = "workspace")]
fn table_page<'a>(
    core: &'a Core,
    ws: &'a Workspace,
    params: TablePageParams,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    let stream_id = params.stream_id.clone();
    Ok(tag_run(stream_id, ws.table_page(core, params)))
}

#[cfg(not(feature = "workspace"))]
fn table_page<'a>(
    _: &'a Core,
    _: &'a Workspace,
    _: TablePageParams,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    Err(RpcError::not_supported("Paging tables"))
}

#[cfg(not(feature = "workspace"))]
fn run<'a>(
    _: &'a Core,
    _: &'a Workspace,
    _: RunParams,
    _: WriteOrigin,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    Err(RpcError::not_supported("Running queries"))
}

#[cfg(not(feature = "workspace"))]
fn page<'a>(
    _: &'a Core,
    _: &'a Workspace,
    _: PageParams,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    Err(RpcError::not_supported("Running queries"))
}

/// `ws`'s [`CoreEvent::ConnectionClosed`] and [`CoreEvent::StorageChanged`]
/// events from now on (see `Workspace::events`). The desktop's `core_events` and each web
/// `/rpc/stream` socket hold one; dropping it unsubscribes.
pub fn workspace_events(ws: &Workspace) -> BoxStream<'static, CoreEvent> {
    Box::pin(
        ws.events()
            .filter_map(|event| std::future::ready(CoreEvent::from_workspace(event))),
    )
}
