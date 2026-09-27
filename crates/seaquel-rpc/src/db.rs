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
//! `queryStream` isn't a request/response call: [`dispatch_stream`] serves
//! it as a stream of [`CoreEvent`]s (the desktop's `core_stream`, the web's
//! `/rpc/stream`), and [`crate::dispatch_workspace`] refuses it.
//!
//! Whether `connect` and `test` may connect at all, and to what, is Core's
//! `ConnectPolicy`: a Core built without one answers `NOT_SUPPORTED`. Without
//! this crate's `workspace` feature (the browser build) both answer
//! `NOT_SUPPORTED` too.

use futures::StreamExt;
use seaquel_core::{Core, QueryOptions, Workspace, WorkspaceEvent};
use seaquel_engine::BoxStream;
use seaquel_types::connect::{ConnectionForm, SuppliedSecrets};
use seaquel_types::{BatchStatement, ExecuteResult, QueryResult, StreamEvent, Value};
use serde::{Deserialize, Serialize};

use crate::workspace::RpcError;
use crate::{dispatch_on, EngineRequest, EngineResponse};

pub use seaquel_core::{CONNECTION_CLOSED, TUNNEL_CLOSED, WORKSPACE_EVICTED};

// ── Requests ──

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
    /// Cancel the workspace's stream `streamId`. Another workspace's stream
    /// with the same id isn't touched; an unknown or finished id is ignored.
    Cancel {
        stream_id: String,
    },
    /// Only through [`dispatch_stream`].
    QueryStream(QueryStreamParams),
}

impl DbRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            DbRequest::Connect(_) => "connect",
            DbRequest::Test(_) => "test",
            DbRequest::Disconnect { .. } => "disconnect",
            DbRequest::Query { .. } => "query",
            DbRequest::Execute { .. } => "execute",
            DbRequest::Transaction { .. } => "transaction",
            DbRequest::Engine { .. } => "engine",
            DbRequest::Cancel { .. } => "cancel",
            DbRequest::QueryStream(_) => "queryStream",
        }
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

// ── Responses ──

/// A `db` call's result, as `{"method": …, "result": …}`. Calls that return
/// nothing have `"result": null`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum DbResponse {
    Connect(Connected),
    Test(()),
    Disconnect(()),
    Query(QueryResult),
    Execute(ExecuteResult),
    Transaction(()),
    Engine(EngineResponse),
    Cancel(()),
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
/// query stream's events, `{"type":"connectionClosed",…}` when a connection
/// went away without the GUI asking.
#[derive(Debug, Clone, Serialize)]
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
    /// workspace's connections). [`CONNECTION_CLOSED`] (lost) and
    /// [`TUNNEL_CLOSED`] are reserved: Core doesn't detect those yet.
    ConnectionClosed {
        connection_id: String,
        code: String,
        message: String,
    },
}

impl CoreEvent {
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
            #[allow(unreachable_patterns)] // `WorkspaceEvent` is non_exhaustive.
            _ => None,
        }
    }
}

// ── Dispatch ──

/// Serve a `db` call (not `queryStream`) through `ws`.
pub(crate) async fn db(
    core: &Core,
    ws: &Workspace,
    req: DbRequest,
) -> Result<DbResponse, RpcError> {
    Ok(match req {
        DbRequest::Connect(params) => DbResponse::Connect(Connected {
            connection_id: connect(core, ws, params).await?,
        }),
        DbRequest::Test(params) => DbResponse::Test(test(core, ws, params).await?),
        DbRequest::Disconnect { connection_id } => {
            DbResponse::Disconnect(ws.disconnect(core, &connection_id).await?)
        }
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
        } => DbResponse::Transaction(ws.transaction(core, &connection_id, statements).await?),
        DbRequest::Engine {
            connection_id,
            request,
        } => {
            let handle = ws.engine(core, &connection_id)?;
            DbResponse::Engine(dispatch_on(&handle, request).await?)
        }
        DbRequest::Cancel { stream_id } => {
            ws.cancel(core, &stream_id);
            DbResponse::Cancel(())
        }
        DbRequest::QueryStream(_) => {
            return Err(RpcError::invalid_argument(
                "db.queryStream is served by the stream transport (core_stream, /rpc/stream), \
                 not as a single call",
            ))
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
    req.with_secrets(params.secrets)
        .with_host_key(host_key)
        .with_create_if_missing(params.create_if_missing)
}

#[cfg(feature = "workspace")]
async fn connect(core: &Core, ws: &Workspace, params: ConnectParams) -> Result<String, RpcError> {
    Ok(ws.connect(core, connect_request(params)).await?)
}

#[cfg(feature = "workspace")]
async fn test(core: &Core, ws: &Workspace, params: ConnectParams) -> Result<(), RpcError> {
    Ok(ws.test(core, connect_request(params)).await?)
}

#[cfg(not(feature = "workspace"))]
async fn connect(_: &Core, _: &Workspace, _: ConnectParams) -> Result<String, RpcError> {
    Err(RpcError::not_supported("Connecting"))
}

#[cfg(not(feature = "workspace"))]
async fn test(_: &Core, _: &Workspace, _: ConnectParams) -> Result<(), RpcError> {
    Err(RpcError::not_supported("Connecting"))
}

/// Start a `db.queryStream` request on `ws`: its events as
/// [`CoreEvent::Stream`]s tagged with its `streamId`, ending after `done` or
/// `error` (or with nothing more after a cancel). A connection `ws` doesn't
/// own gives one `CONNECTION_NOT_FOUND` error event.
///
/// Anything but `db.queryStream` is `INVALID_ARGUMENT`. The desktop's
/// `core_stream` and the web's `/rpc/stream` call it with a request parsed
/// by [`crate::parse_request`]. Logs the method name only.
pub fn dispatch_stream<'a>(
    core: &'a Core,
    ws: &Workspace,
    req: crate::Request,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    let (group, method) = (req.group(), req.method());
    log::debug!(activity = "rpc.stream", group = group, method = method; "Workspace stream");
    let crate::Request::Db(DbRequest::QueryStream(params)) = req else {
        log::debug!(activity = "rpc.stream", group = group, method = method, code = crate::INVALID_ARGUMENT; "Workspace stream refused");
        return Err(RpcError::invalid_argument(format!(
            "{group}.{method} isn't a stream; only db.queryStream is"
        )));
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

/// `ws`'s [`CoreEvent::ConnectionClosed`] events from now on (see
/// `Workspace::events`). The desktop's `core_events` and each web
/// `/rpc/stream` socket hold one; dropping it unsubscribes.
pub fn workspace_events(ws: &Workspace) -> BoxStream<'static, CoreEvent> {
    Box::pin(
        ws.events()
            .filter_map(|event| std::future::ready(CoreEvent::from_workspace(event))),
    )
}
