//! The tool registry (`seaquel_ai::tools`, re-exported whole) and Core's
//! side of it: running a parsed call on the paths Core already has
//! (Decision 4). Queries go through `Workspace::query_stream` with
//! `read_only`, `max_rows`, `max_bytes` (8 MiB) and `timeout` (60 s);
//! EXPLAIN through the read-only EXPLAIN; introspection through
//! `schema_tables` and `table_metadata`; saved queries through storage and
//! `{{param}}` substitution. No new SQL path, no new read-only rule.
//!
//! [`call`] runs any call for either profile. The assistant's turn splits
//! it: [`plan`] (a saved query's lookup, substitution and read-only check),
//! then its approval, then [`run`].

pub use seaquel_ai::tools::*;

use std::time::Duration;

use futures::StreamExt;
use seaquel_ai::limits::QuerySpec;
use seaquel_sql::params::substitute;
use seaquel_sql::SqlEngine;
use seaquel_storage::saved_queries;
use seaquel_types::{StreamEvent, Value};
use serde_json::Value as Json;

use crate::{Core, QueryOptions, Workspace};

/// A stream that ended without `Done` or an error: it was cancelled.
pub const CANCELLED: &str = "CANCELLED";

/// What a call runs against: the profile, the open connection (Core's
/// id), the names its answers and refusals use, and its time limit. Its
/// `Debug` leaves out the names.
#[derive(Clone, Copy)]
pub struct ToolContext<'a> {
    pub profile: Profile,
    /// Core's id of the open connection the call runs on.
    pub connection_id: &'a str,
    /// The saved connection's name, as answers and messages name it.
    pub connection_name: &'a str,
    /// The saved connection's project (saved queries).
    pub project_id: &'a str,
    pub project_name: &'a str,
    /// The database's statement timeout for the call's query or EXPLAIN:
    /// `limits::CALL_TIMEOUT` (60 s) for the assistant, the server's call
    /// timeout for MCP (60 s unless set). [`call`] sets no deadline from it;
    /// the caller bounds the call.
    pub timeout: Duration,
}

impl std::fmt::Debug for ToolContext<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("profile", &self.profile)
            .field("connection_id", &self.connection_id)
            .field("timeout_ms", &self.timeout.as_millis())
            .finish_non_exhaustive()
    }
}

/// A call ready to run: its SQL, after a saved query's substitution and
/// read-only check (what an approval shows).
#[derive(Clone)]
pub(crate) enum Planned {
    Query {
        sql: String,
        binds: Vec<Value>,
        max_rows: usize,
    },
    Explain {
        sql: String,
    },
    /// Introspection and listings: nothing to show first.
    Direct,
}

impl Planned {
    /// The SQL an approval card shows.
    pub(crate) fn sql(&self) -> Option<&str> {
        match self {
            Planned::Query { sql, .. } | Planned::Explain { sql } => Some(sql),
            Planned::Direct => None,
        }
    }
}

fn tool_error(e: impl Into<crate::CoreError>) -> ToolError {
    let e = e.into();
    ToolError::new(e.code, e.message)
}

fn db_error(e: seaquel_engine::DbError) -> ToolError {
    ToolError::new(e.code, e.message)
}

/// The connection's SQL rules, or `CONNECTION_NOT_FOUND` when this
/// workspace no longer has it (a reconnect, a disconnect).
pub(crate) fn engine_of(
    core: &Core,
    ws: &Workspace,
    connection_id: &str,
) -> Result<SqlEngine, ToolError> {
    let connection = core
        .connection_as(connection_id, Some(ws.id()))
        .map_err(db_error)?;
    connection.sql_engine.ok_or_else(ToolError::read_only)
}

/// Runs `call` for `ctx.profile`: [`plan`], then [`run`]. It sets no
/// deadline of its own: `ctx.timeout` goes to the database as the query's
/// and EXPLAIN's statement timeout, and the caller bounds the whole call.
/// MCP's `timed` is the only deadline for its calls, since it leaves out the
/// time a keychain read is pending (in any call, not only this one), which a
/// deadline here couldn't see; the assistant's turn races [`run`] against
/// its own timer.
///
/// It checks neither sharing nor the SQL's read-only tokens up front: the
/// caller checks sharing (MCP: per call, with its own order; the assistant:
/// [`prepare`]), and a query's refusal is `query_stream`'s `READ_ONLY` (a
/// saved query's SQL is checked in [`plan`], with the same message).
pub async fn call(
    core: &Core,
    ws: &Workspace,
    ctx: &ToolContext<'_>,
    call: &Call,
) -> Result<Json, ToolError> {
    let planned = plan(core, ws, ctx, call).await?;
    run(core, ws, ctx, call, planned).await
}

/// A turn's tool call that passed its time limit `limit` (MCP's wording).
pub(crate) fn timed_out(limit: Duration) -> ToolError {
    ToolError::new(
        "TIMEOUT",
        format!(
            "The call took longer than {} s and was cancelled",
            limit.as_secs_f64()
        ),
    )
}

/// What `call` will run. A saved query is found in the context's project,
/// its parameters filled and substituted for the connection's engine, and
/// the SQL that will run checked read-only, so a saved query that writes
/// is refused before an approval would show it.
pub(crate) async fn plan(
    core: &Core,
    ws: &Workspace,
    ctx: &ToolContext<'_>,
    call: &Call,
) -> Result<Planned, ToolError> {
    Ok(match &call.args {
        Args::RunQuery { sql, .. } => Planned::Query {
            sql: sql.clone(),
            binds: Vec::new(),
            max_rows: call.max_rows(),
        },
        Args::ExplainQuery { sql } => Planned::Explain { sql: sql.clone() },
        Args::RunSavedQuery {
            saved_query,
            params,
            ..
        } => {
            let engine = engine_of(core, ws, ctx.connection_id)?;
            let queries = saved_queries::load_by_project(ws.storage(), ctx.project_id)
                .await
                .map_err(tool_error)?;
            let q = render::find_saved_query(
                &queries,
                saved_query,
                ctx.project_name,
                ctx.connection_name,
            )?;
            let values = saved::parameter_values(&saved::definitions(q), params.clone())?;
            let s = substitute(&q.query, &values, engine, false)
                .map_err(|e| ToolError::new(saved::INVALID_PARAMETERS, e.message))?;
            read_only_sql(&s.sql, engine)?;
            Planned::Query {
                sql: s.sql,
                binds: s.bind_values,
                max_rows: call.max_rows(),
            }
        }
        _ => Planned::Direct,
    })
}

/// Runs a planned call and renders its answer for `ctx.profile`.
pub(crate) async fn run(
    core: &Core,
    ws: &Workspace,
    ctx: &ToolContext<'_>,
    call: &Call,
    planned: Planned,
) -> Result<Json, ToolError> {
    let handle = || ws.engine(core, ctx.connection_id).map_err(db_error);
    match (planned, &call.args) {
        (
            Planned::Query {
                sql,
                binds,
                max_rows,
            },
            _,
        ) => rows(core, ws, ctx, sql, binds, max_rows).await,
        (Planned::Explain { sql }, _) => {
            let plan = handle()?
                .explain_read_only(&sql, Vec::new(), Some(ctx.timeout))
                .await
                .map_err(db_error)?;
            Ok(render::explain(ctx.profile, &plan))
        }
        (Planned::Direct, Args::ListSchemas) => {
            let schemas = handle()?.list_schemas().await.map_err(db_error)?;
            Ok(render::schemas(&schemas))
        }
        (Planned::Direct, Args::ListTables { schema }) => {
            let tables = handle()?.schema_tables().await.map_err(db_error)?;
            Ok(render::tables(ctx.profile, &tables, schema.as_deref()))
        }
        (Planned::Direct, Args::DescribeTable { schema, table }) => {
            let h = handle()?;
            let tables = h.schema_tables().await.map_err(db_error)?;
            let found = render::find_table(&tables, schema.as_deref(), table)?;
            let (columns, indexes) = h
                .table_metadata(&found.schema, &found.name)
                .await
                .map_err(db_error)?;
            Ok(render::describe(
                ctx.profile,
                found,
                table,
                &columns,
                &indexes,
            ))
        }
        (Planned::Direct, Args::ListSavedQueries { .. }) => {
            let queries = saved_queries::load_by_project(ws.storage(), ctx.project_id)
                .await
                .map_err(tool_error)?;
            let entries = queries
                .iter()
                .map(|q| saved::describe(q, ctx.project_name, &[ctx.connection_name]))
                .collect();
            Ok(render::saved_queries(ctx.profile, entries, 0))
        }
        (Planned::Direct, _) => Err(ToolError::unknown_tool(call.tool.name())),
    }
}

/// A read-only query's rows: at most `max_rows`, the driver stopping at
/// 8 MiB and the database at `ctx.timeout`, formatted as they arrive within
/// the profile's budget.
async fn rows(
    core: &Core,
    ws: &Workspace,
    ctx: &ToolContext<'_>,
    sql: String,
    binds: Vec<Value>,
    max_rows: usize,
) -> Result<Json, ToolError> {
    let spec = QuerySpec {
        timeout: ctx.timeout,
        ..QuerySpec::new(max_rows)
    };
    let options = QueryOptions::default()
        .with_read_only(true)
        .with_max_rows(Some(spec.max_rows))
        .with_max_bytes(Some(spec.max_bytes))
        .with_timeout(Some(spec.timeout));
    let stream_id = format!("ai-{}", uuid::Uuid::new_v4());
    let mut stream = ws.query_stream(
        core,
        stream_id,
        ctx.connection_id.to_string(),
        sql,
        binds,
        options,
    );
    let mut out = render::Rows::new(ctx.profile, max_rows);
    while let Some(event) = stream.next().await {
        match event {
            StreamEvent::Batch(batch) => {
                if let Some(columns) = batch.columns {
                    out.set_columns(columns);
                }
                if batch.truncated {
                    out.mark_truncated();
                }
                for row in &batch.rows {
                    if !out.push(row) {
                        // Full: dropping the stream stops the driver.
                        return Ok(out.into_json());
                    }
                }
            }
            StreamEvent::Done => return Ok(out.into_json()),
            StreamEvent::Error { code, message } => return Err(ToolError::new(code, message)),
        }
    }
    Err(ToolError::new(CANCELLED, "The query was cancelled"))
}
