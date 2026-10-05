//! A saved connection, project or saved query named on the command line: by
//! id, else by its exact, case-sensitive name. A name several rows share is
//! refused with every id, so the user can pass one of them instead.
//!
//! Connections and projects use the MCP server's lookup (`--connection`,
//! `--project`), worded for a command's argument rather than those flags:
//! `Connection "x": no saved connection has this name or id`, and for a
//! shared name `… Pass the id instead.`, as for a saved query.

use seaquel_core::CoreError;
use seaquel_mcp::exposed::{find_connection, find_project, lookup, Found, Subject};
use seaquel_mcp::ToolError;
use seaquel_types::storage::{PersistedConnection, PersistedProject, PersistedSavedQuery};

use crate::session::Session;

pub const SAVED_QUERY_NOT_FOUND: &str = "SAVED_QUERY_NOT_FOUND";
pub const AMBIGUOUS_SAVED_QUERY: &str = "AMBIGUOUS_SAVED_QUERY";

fn core_error(e: ToolError) -> CoreError {
    CoreError::new(e.code, e.message)
}

/// The saved connection whose id is `wanted`, else the one whose name is.
pub fn connection<'a>(
    rows: &'a [PersistedConnection],
    projects: &[PersistedProject],
    wanted: &str,
) -> Result<&'a PersistedConnection, CoreError> {
    find_connection(rows, projects, wanted, Subject::Noun("Connection")).map_err(core_error)
}

/// The saved connection `wanted` names, read from the app's data.
pub async fn saved_connection(s: &Session, wanted: &str) -> Result<PersistedConnection, CoreError> {
    let projects = s.ws.list_projects().await?.value;
    saved_connection_among(s, &projects, wanted).await
}

/// [`saved_connection`] with the projects already listed.
pub async fn saved_connection_among(
    s: &Session,
    projects: &[PersistedProject],
    wanted: &str,
) -> Result<PersistedConnection, CoreError> {
    let rows = s.ws.list_connections().await?.value;
    connection(&rows, projects, wanted).cloned()
}

/// The project whose id is `wanted`, else the one whose name is.
pub fn project<'a>(
    projects: &'a [PersistedProject],
    wanted: &str,
) -> Result<&'a PersistedProject, CoreError> {
    find_project(projects, wanted, Subject::Noun("Project")).map_err(core_error)
}

/// `--project`'s filter: the id of the project it names, or `None` (every
/// project) without it.
pub fn project_filter(
    projects: &[PersistedProject],
    wanted: Option<&str>,
) -> Result<Option<String>, CoreError> {
    wanted
        .map(|wanted| project(projects, wanted).map(|p| p.id.clone()))
        .transpose()
}

/// The saved query whose id is `wanted`, else the one whose name is, among
/// `rows` (each with its project's name): one project's, or every
/// project's.
pub fn saved_query<'a>(
    rows: &'a [(PersistedSavedQuery, String)],
    wanted: &str,
) -> Result<&'a PersistedSavedQuery, CoreError> {
    match lookup(rows, wanted, |(q, _)| &q.id, |(q, _)| &q.name) {
        Found::One((q, _)) => Ok(q),
        Found::None => Err(CoreError::new(
            SAVED_QUERY_NOT_FOUND,
            format!("Saved query {wanted:?}: no saved query has this name or id"),
        )),
        Found::Many(many) => Err(CoreError::new(
            AMBIGUOUS_SAVED_QUERY,
            format!(
                "Saved query {wanted:?}: {} saved queries have this name: {}. Pass the id \
                 instead.",
                many.len(),
                many.iter()
                    .map(|(q, project)| format!("id {:?} in project {:?}", q.id, project))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn query(id: &str, name: &str) -> (PersistedSavedQuery, String) {
        let q = serde_json::from_value(json!({
            "id": id, "name": name, "query": "SELECT 1", "projectId": "p1",
            "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
        }))
        .unwrap();
        (q, "Main".to_string())
    }

    #[test]
    fn a_saved_query_by_id_then_by_name() {
        let rows = [
            query("sq-1", "count"),
            query("sq-2", "sq-1"),
            query("sq-3", "twin"),
            query("sq-4", "twin"),
        ];
        // An id wins over another row's name.
        assert_eq!(saved_query(&rows, "sq-1").unwrap().id, "sq-1");
        assert_eq!(saved_query(&rows, "count").unwrap().id, "sq-1");
        assert_eq!(
            saved_query(&rows, "Count").unwrap_err().code,
            SAVED_QUERY_NOT_FOUND
        );
        let e = saved_query(&rows, "twin").unwrap_err();
        assert_eq!(e.code, AMBIGUOUS_SAVED_QUERY);
        assert!(
            e.message.contains("\"sq-3\"") && e.message.contains("\"sq-4\""),
            "{}",
            e.message
        );
        assert!(e.message.contains("\"Main\""), "{}", e.message);
        assert!(
            e.message.ends_with(". Pass the id instead."),
            "{}",
            e.message
        );
    }

    fn connection_row(id: &str, name: &str) -> PersistedConnection {
        serde_json::from_value(json!({
            "id": id, "projectId": "p1", "name": name, "type": "sqlite",
            "host": "", "port": 0, "databaseName": "", "username": "",
            "savePassword": false, "saveSshPassword": false,
            "saveSshKeyPassphrase": false, "labelIds": [],
        }))
        .unwrap()
    }

    fn project_row(id: &str, name: &str) -> PersistedProject {
        serde_json::from_value(json!({
            "id": id, "name": name, "customLabels": [],
            "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
        }))
        .unwrap()
    }

    /// The commands take a connection or a project as an argument, not as
    /// `--connection` or `--project`: the messages don't name a flag.
    #[test]
    fn lookups_name_no_flag() {
        let rows = [connection_row("a", "twin"), connection_row("b", "twin")];
        let e = connection(&rows, &[], "nope").unwrap_err();
        assert_eq!(
            e.message,
            "Connection \"nope\": no saved connection has this name or id"
        );
        let e = connection(&rows, &[], "twin").unwrap_err();
        assert!(!e.message.contains("--"), "{}", e.message);
        assert!(
            e.message.ends_with(". Pass the id instead."),
            "{}",
            e.message
        );

        let projects = [project_row("p1", "Main"), project_row("p2", "Main")];
        let e = project(&projects, "nope").unwrap_err();
        assert_eq!(
            e.message,
            "Project \"nope\": no project has this name or id"
        );
        let e = project(&projects, "Main").unwrap_err();
        assert!(!e.message.contains("--"), "{}", e.message);
        assert!(
            e.message.ends_with(". Pass the id instead."),
            "{}",
            e.message
        );

        assert_eq!(project_filter(&projects, None).unwrap(), None);
        assert_eq!(
            project_filter(&projects, Some("p2")).unwrap().as_deref(),
            Some("p2")
        );
    }
}
