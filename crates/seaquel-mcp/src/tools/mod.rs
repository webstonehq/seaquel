//! The tools (Decision 5 of the phase 4 plan), on Core's registry since
//! phase 6 (Decision 20). `server.rs` routes calls here.
//!
//! The registry (`seaquel_core::ai::tools`) has the arguments, their
//! schemas, the renderers and the runner (`tools::call` with
//! `Profile::Mcp`). What stays here is what only the MCP server has: the
//! exposed set, the sharing check in the order MCP has always made it,
//! connecting on first use and the per-call timeout.
//!
//! Every tool resolves `connection` against the exposed set only, checks the
//! connection's AI sharing flags, connects on first use and runs under the
//! per-call timeout. Results are compact JSON in one text block; failures
//! are tool errors (`CODE: message`).

mod saved;

use seaquel_core::ai::limits;
use seaquel_core::ai::tools::{self as registry, Args, Call, Needs, Profile, ToolContext};
use serde_json::{json, Value as Json};

use crate::error::ToolError;
use crate::exposed::Exposed;
use crate::server::Inner;

pub(crate) use saved::list_saved_queries;

/// What `list_connections` and an unknown connection say when nothing is
/// exposed.
pub const NO_CONNECTIONS_HINT: &str = "The user chooses which saved connections to expose \
when starting the server: `seaquel-cli mcp --connection <name or id>` (repeatable) or \
`--project <name or id>` for all of a project's connections. Settings > MCP in the Seaquel \
app builds the command line.";

pub(crate) async fn list_connections(inner: &Inner) -> Result<Json, ToolError> {
    let snapshot = inner.sharing_snapshot().await?;
    let mut out = Vec::with_capacity(inner.exposed.len());
    for c in &inner.exposed {
        let sharing = snapshot.get(c)?;
        out.push(json!({
            "name": c.name,
            "id": c.id,
            "engine": c.engine,
            "project": c.project_name,
            "shareSchema": sharing.schema,
            "shareData": sharing.data,
        }));
    }
    if out.is_empty() {
        return Ok(json!({
            "connections": [],
            "message": format!("No connections are exposed. {NO_CONNECTIONS_HINT}"),
        }));
    }
    Ok(json!({ "connections": out }))
}

/// A tool that runs on one exposed connection: `list_schemas`,
/// `list_tables`, `describe_table`, `run_query`, `explain_query` and
/// `run_saved_query`. In order:
///
/// 1. the connection, among the exposed set (`CONNECTION_NOT_FOUND`,
///    `AMBIGUOUS_CONNECTION`);
/// 2. `max_rows`, 1 to 1000 (`INVALID_ARGUMENT`);
/// 3. a saved query's lookup in the connection's project
///    (`SAVED_QUERY_NOT_FOUND`, `AMBIGUOUS_SAVED_QUERY`);
/// 4. the sharing flag the tool needs, read now (`SCHEMA_SHARING_OFF`,
///    `DATA_SHARING_OFF`);
/// 5. a saved query's parameter values (`INVALID_PARAMETERS`), before
///    anything connects;
/// 6. under the call timeout: connect on first use, then the registry runs
///    the call ([`registry::call`]), which renders MCP's answer.
///
/// Steps 3 and 5 are checks only (the registry finds the query and fills
/// its parameters again when it runs it); they keep MCP's refusals in the
/// order hosts have always seen them. A query's `READ_ONLY` refusal is
/// Core's, from `query_stream` (or, for a saved query, the same check on its
/// substituted SQL).
pub(crate) async fn on_connection(inner: &Inner, call: Call) -> Result<Json, ToolError> {
    let wanted = call.connection.as_deref().unwrap_or_default();
    let c = inner.resolve(wanted)?;
    if let Args::RunQuery { max_rows, .. } | Args::RunSavedQuery { max_rows, .. } = &call.args {
        limits::max_rows(*max_rows)?;
    }
    let saved = match &call.args {
        Args::RunSavedQuery { saved_query, .. } => Some(saved::find(inner, c, saved_query).await?),
        _ => None,
    };
    let sharing = inner.sharing(c).await?;
    match call.tool.needs() {
        Needs::Schema if !sharing.schema => {
            return Err(registry::ToolError::schema_sharing_off(&c.name).into())
        }
        Needs::Data if !sharing.data => {
            return Err(registry::ToolError::data_sharing_off(&c.name).into())
        }
        _ => {}
    }
    if let (Some(query), Args::RunSavedQuery { params, .. }) = (&saved, &call.args) {
        saved::check_parameters(c, query, params)?;
    }
    run(inner, c, &call).await
}

/// Connect `c` on first use and run `call` on it through the registry,
/// under the per-call timeout.
async fn run(inner: &Inner, c: &Exposed, call: &Call) -> Result<Json, ToolError> {
    inner
        .timed(async {
            let connection_id = inner.connection(c).await?;
            let ctx = ToolContext {
                profile: Profile::Mcp,
                connection_id: &connection_id,
                connection_name: &c.name,
                project_id: &c.project_id,
                project_name: &c.project_name,
                // Also the database's timeout, as a backstop for `timed`'s
                // deadline: a query the cancel doesn't reach still ends
                // there, with `TIMEOUT`.
                timeout: inner.options.call_timeout,
            };
            Ok(registry::call(&inner.core, &inner.workspace, &ctx, call).await?)
        })
        .await
}
