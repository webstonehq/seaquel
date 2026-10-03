//! `list_saved_queries` and `run_saved_query`.
//!
//! A saved query belongs to a project, so it can run on any exposed
//! connection of that project. Its `{{parameters}}` are filled the way the
//! editor fills them:
//!
//! - the definitions are the saved query's own `parameters` when it has
//!   some, else one `text` parameter per `{{name}}` in the SQL
//!   (`param-dialog.svelte.ts` `getParameterDefinitions`), plus, unlike the
//!   dialog, a `text` parameter with no default for each `{{name}}` the
//!   stored definitions leave out (the dialog shows no field for it and runs
//!   it as NULL; here it needs a value like any other);
//! - each value is the dialog's text for it, the host's value or else the
//!   definition's default, coerced by its type (`coerceValue` in
//!   `src/lib/sql/parameters.ts`);
//! - `seaquel_sql::params::substitute` fills them in for the connection's
//!   engine, without forcing inline values (`resolve-query.ts`).
//!
//! Unlike the dialog, which runs a parameter left empty as NULL, a
//! parameter with no value and no default is refused, and so is a value
//! for a name the query doesn't take.
//!
//! The registry runs it (`seaquel_core::ai::tools::call`): it finds the
//! query, fills and substitutes its parameters for the connection's engine,
//! checks the result read-only and renders the rows. This module lists the
//! saved queries across the exposed projects, and checks a run's lookup and
//! parameters before the sharing check and the connect, in MCP's order.
//!
//! **Sharing.** `list_saved_queries` lists a project's saved queries only
//! when at least one of its exposed connections shares its schema, and each
//! entry names those connections. A query's name, description and parameter
//! names, defaults and descriptions describe its SQL (a parameter is usually
//! named after the column it filters), so stripping only descriptions and
//! defaults would still leak the schema: the whole entry goes. The project,
//! not a connection, owns a saved query, so one schema-sharing connection in
//! it is enough; with `connection` given, that connection must share its
//! schema (`SCHEMA_SHARING_OFF` otherwise). How many were left out is said,
//! never which.

use seaquel_core::ai::tools::render;
use seaquel_core::ai::tools::saved::{definitions, describe, parameter_values};
use seaquel_core::ai::tools::{Profile, ToolError as RegistryError};
use seaquel_core::sql::SqlEngine;
use seaquel_core::storage::saved_queries;
use seaquel_types::storage::PersistedSavedQuery;
use serde_json::{Map, Value as Json};

use crate::error::{ToolError, INVALID_ARGUMENT};
use crate::exposed::{Exposed, PROJECT_NOT_FOUND};
use crate::server::Inner;

/// An exposed project, and its exposed connections that share their schema.
struct Project<'a> {
    id: &'a str,
    name: &'a str,
    sharing: Vec<&'a str>,
}

pub(crate) async fn list_saved_queries(
    inner: &Inner,
    connection: Option<&str>,
    project: Option<&str>,
) -> Result<Json, ToolError> {
    let snapshot = inner.sharing_snapshot().await?;
    // Only the projects of exposed connections, never another project's.
    let mut projects: Vec<Project> = Vec::new();
    if let Some(wanted) = connection {
        let c = inner.resolve(wanted)?;
        if !snapshot.get(c)?.schema {
            return Err(RegistryError::schema_sharing_off(&c.name).into());
        }
        projects.push(Project {
            id: &c.project_id,
            name: &c.project_name,
            sharing: vec![&c.name],
        });
    } else {
        for c in &inner.exposed {
            let i = match projects.iter().position(|p| p.id == c.project_id) {
                Some(i) => i,
                None => {
                    projects.push(Project {
                        id: &c.project_id,
                        name: &c.project_name,
                        sharing: Vec::new(),
                    });
                    projects.len() - 1
                }
            };
            if snapshot.get(c)?.schema {
                projects[i].sharing.push(&c.name);
            }
        }
    }
    if let Some(wanted) = project {
        let by_id = projects.iter().any(|p| p.id == wanted);
        projects.retain(|p| {
            if by_id {
                p.id == wanted
            } else {
                p.name == wanted
            }
        });
        if projects.is_empty() {
            return Err(ToolError::new(
                PROJECT_NOT_FOUND,
                format!("No exposed connection belongs to a project named {wanted:?}"),
            ));
        }
    }
    let mut out = Vec::new();
    let mut hidden = 0;
    for p in &projects {
        let queries = saved_queries::load_by_project(inner.workspace.storage(), p.id).await?;
        if p.sharing.is_empty() {
            hidden += queries.len();
            continue;
        }
        out.extend(queries.iter().map(|q| describe(q, p.name, &p.sharing)));
    }
    Ok(render::saved_queries(Profile::Mcp, out, hidden))
}

/// The saved query `wanted` names in `c`'s project, by id, else by exact
/// name (`SAVED_QUERY_NOT_FOUND`, `AMBIGUOUS_SAVED_QUERY`): `run_saved_query`
/// looks it up before the sharing check, as it always has.
pub(super) async fn find(
    inner: &Inner,
    c: &Exposed,
    wanted: &str,
) -> Result<PersistedSavedQuery, ToolError> {
    let queries = saved_queries::load_by_project(inner.workspace.storage(), &c.project_id).await?;
    Ok(render::find_saved_query(&queries, wanted, &c.project_name, &c.name)?.clone())
}

/// `run_saved_query`'s parameter values checked against the query's
/// definitions (`INVALID_PARAMETERS`), before anything connects. The
/// registry fills them in again when it runs the query.
pub(super) fn check_parameters(
    c: &Exposed,
    query: &PersistedSavedQuery,
    params: &Map<String, Json>,
) -> Result<(), ToolError> {
    // Two engine sources, as before the registry: this check reads the
    // saved row's `type` (`c.engine`), so a type that isn't an engine is refused
    // before anything connects; the registry substitutes and checks the SQL
    // under the open connection's `sql_engine` (what Core recorded at
    // connect, MariaDB included), the rules `query_stream` scans with.
    c.engine.parse::<SqlEngine>().map_err(|_| {
        ToolError::new(
            INVALID_ARGUMENT,
            format!("Unknown engine {:?} for connection {:?}", c.engine, c.name),
        )
    })?;
    parameter_values(&definitions(query), params.clone())?;
    Ok(())
}
