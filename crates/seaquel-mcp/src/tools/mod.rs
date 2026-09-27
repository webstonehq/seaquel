//! The tools (Decision 5 of the phase 4 plan): their arguments and what
//! each one does. `server.rs` routes calls here.
//!
//! Every tool resolves `connection` against the exposed set only, checks the
//! connection's AI sharing flags, connects on first use and runs under the
//! per-call timeout. Results are compact JSON in one text block; failures
//! are tool errors (`CODE: message`).

mod query;
mod saved;
mod schema;

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Map, Value as Json};

use crate::error::{ToolError, DATA_SHARING_OFF, SCHEMA_SHARING_OFF};
use crate::exposed::Exposed;
use crate::server::Inner;

pub(crate) use query::{explain_query, run_query};
pub(crate) use saved::{list_saved_queries, run_saved_query};
pub(crate) use schema::{describe_table, list_schemas, list_tables};

/// What `list_connections` and an unknown connection say when nothing is
/// exposed.
pub const NO_CONNECTIONS_HINT: &str = "The user chooses which saved connections to expose \
when starting the server: `seaquel-cli mcp --connection <name or id>` (repeatable) or \
`--project <name or id>` for all of a project's connections. Settings > MCP in the Seaquel \
app builds the command line.";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConnectionArgs {
    /// The connection's name or id, exactly as list_connections shows it.
    pub connection: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTablesArgs {
    /// The connection's name or id, exactly as list_connections shows it.
    pub connection: String,
    /// Only the tables of this schema (as list_schemas shows it). All
    /// schemas when omitted.
    #[serde(default)]
    pub schema: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DescribeTableArgs {
    /// The connection's name or id, exactly as list_connections shows it.
    pub connection: String,
    /// The table's schema, as list_tables shows it. When omitted, the table
    /// name must be unique across schemas.
    #[serde(default)]
    pub schema: Option<String>,
    /// The table or view name, as list_tables shows it.
    pub table: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunQueryArgs {
    /// The connection's name or id, exactly as list_connections shows it.
    pub connection: String,
    /// One read-only SQL statement in the connection's dialect.
    pub sql: String,
    /// The most rows to return: 1 to 1000, default 100.
    #[serde(default)]
    #[schemars(range(min = 1, max = 1000))]
    pub max_rows: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExplainArgs {
    /// The connection's name or id, exactly as list_connections shows it.
    pub connection: String,
    /// One read-only SQL statement to explain.
    pub sql: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListSavedQueriesArgs {
    /// Only the saved queries of this connection's project.
    #[serde(default)]
    pub connection: Option<String>,
    /// Only the saved queries of this project (name or id), which must hold
    /// an exposed connection.
    #[serde(default)]
    pub project: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunSavedQueryArgs {
    /// The connection to run it on (name or id); the saved query must belong
    /// to its project.
    pub connection: String,
    /// The saved query's id or name, as list_saved_queries shows it.
    pub saved_query: String,
    /// A value for each of the query's parameters, by name: a string, number,
    /// boolean or null. A parameter with a default may be left out; every
    /// other one is required, and names the query doesn't take are refused.
    #[serde(default)]
    pub params: Option<Map<String, Json>>,
    /// The most rows to return: 1 to 1000, default 100.
    #[serde(default)]
    #[schemars(range(min = 1, max = 1000))]
    pub max_rows: Option<u32>,
}

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

/// Refuse unless the connection shares its schema.
async fn require_schema(inner: &Inner, c: &Exposed) -> Result<(), ToolError> {
    if inner.sharing(c).await?.schema {
        return Ok(());
    }
    Err(schema_sharing_off(c))
}

/// `SCHEMA_SHARING_OFF` for `c`.
fn schema_sharing_off(c: &Exposed) -> ToolError {
    ToolError::new(
        SCHEMA_SHARING_OFF,
        format!(
            "The connection {:?} doesn't share its schema with AI tools. The user can turn \
             schema sharing on for it in the Seaquel app (the connection's AI settings, or \
             Settings > AI for the default).",
            c.name
        ),
    )
}

/// Refuse unless the connection shares its data.
async fn require_data(inner: &Inner, c: &Exposed) -> Result<(), ToolError> {
    if inner.sharing(c).await?.data {
        return Ok(());
    }
    Err(ToolError::new(
        DATA_SHARING_OFF,
        format!(
            "The connection {:?} doesn't share data with AI tools. The user can turn data \
             sharing on for it in the Seaquel app (the connection's AI settings, or \
             Settings > AI for the default).",
            c.name
        ),
    ))
}
