//! `seaquel-cli saved list` and `seaquel-cli saved show`.
//!
//! `list` prints the saved queries, of every project or of one: as a JSON
//! array (`id`, `name`, `folder`, `projectId`, `projectName`, `parameters`,
//! `description`), or a table of name, folder, project, parameters and id.
//! `parameters` are the `{{name}}`s the query's text uses.
//!
//! `show` prints one saved query's SQL exactly, with a newline added when
//! it doesn't end in one, so `seaquel-cli saved show q > q.sql` is the
//! query. Neither needs a connection.

use std::process::ExitCode;

use seaquel_core::sql::params::extract_parameters;
use seaquel_core::CoreError;
use seaquel_types::storage::{PersistedProject, PersistedSavedQuery};
use serde::Serialize;

use crate::output::{self, Format};
use crate::session::{block_on, fail, Session};
use crate::{resolve, SavedArgs, SavedCommand};

const LIST: &str = "saved list";
const SHOW: &str = "saved show";

pub fn run(args: SavedArgs) -> ExitCode {
    match args.command {
        SavedCommand::List { project, output } => {
            let format = Format::pick(output.format);
            block_on(LIST, list(project, format))
        }
        SavedCommand::Show { query, project } => block_on(SHOW, show(query, project)),
    }
}

/// One saved query as `list` prints it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    id: String,
    name: String,
    folder: Option<String>,
    project_id: String,
    project_name: String,
    parameters: Vec<String>,
    description: Option<String>,
}

/// The saved queries of `project` when given, else of every project, each
/// with its project's name, in storage's order.
async fn queries(
    s: &Session,
    project: Option<&str>,
) -> Result<Vec<(PersistedSavedQuery, String)>, CoreError> {
    let projects = s.ws.list_projects().await?.value;
    queries_among(s, &projects, project).await
}

/// [`queries`] with the projects already listed.
pub(crate) async fn queries_among(
    s: &Session,
    projects: &[PersistedProject],
    project: Option<&str>,
) -> Result<Vec<(PersistedSavedQuery, String)>, CoreError> {
    let only = resolve::project_filter(projects, project)?;
    let mut out = Vec::new();
    for p in projects
        .iter()
        .filter(|p| only.as_ref().is_none_or(|id| *id == p.id))
    {
        let rows = s.ws.list_saved_queries(&s.core, &p.id).await?.value;
        out.extend(rows.into_iter().map(|q| (q, p.name.clone())));
    }
    Ok(out)
}

async fn list(project: Option<String>, format: Format) -> ExitCode {
    let s = match Session::open().await {
        Ok(s) => s,
        Err(e) => return fail(LIST, &e),
    };
    let rows = queries(&s, project.as_deref()).await;
    s.close().await;
    match rows {
        Ok(rows) => {
            let rows: Vec<Listed> = rows.into_iter().map(listed).collect();
            output::print(&match format {
                Format::Json => output::json(&rows),
                Format::Table => list_table(&rows),
            });
            ExitCode::SUCCESS
        }
        Err(e) => fail(LIST, &e),
    }
}

fn listed((q, project_name): (PersistedSavedQuery, String)) -> Listed {
    Listed {
        parameters: extract_parameters(&q.query),
        id: q.id,
        name: q.name,
        folder: q.folder.filter(|f| !f.is_empty()),
        project_id: q.project_id,
        project_name,
        description: q.description.filter(|d| !d.is_empty()),
    }
}

fn list_table(rows: &[Listed]) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            vec![
                r.name.clone(),
                r.folder.clone().unwrap_or_default(),
                r.project_name.clone(),
                r.parameters.join(", "),
                r.id.clone(),
            ]
        })
        .collect();
    output::table(&["NAME", "FOLDER", "PROJECT", "PARAMS", "ID"], &cells)
}

async fn show(query: String, project: Option<String>) -> ExitCode {
    let s = match Session::open().await {
        Ok(s) => s,
        Err(e) => return fail(SHOW, &e),
    };
    let rows = queries(&s, project.as_deref()).await;
    s.close().await;
    let text =
        rows.and_then(|rows| resolve::saved_query(&rows, &query).map(|q| sql_text(&q.query)));
    match text {
        Ok(text) => {
            output::print(&text);
            ExitCode::SUCCESS
        }
        Err(e) => fail(SHOW, &e),
    }
}

/// The query as stored, ending in exactly the newline it had or one added.
fn sql_text(query: &str) -> String {
    let mut text = query.to_string();
    if !text.ends_with('\n') {
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn query(text: &str, folder: Option<&str>) -> (PersistedSavedQuery, String) {
        let q = serde_json::from_value(json!({
            "id": "sq-1", "name": "by id", "query": text, "projectId": "p1",
            "createdAt": "2026-01-02T03:04:05.000Z", "updatedAt": "2026-01-02T03:04:05.000Z",
            "folder": folder,
        }))
        .unwrap();
        (q, "Main".to_string())
    }

    #[test]
    fn the_list_names_each_parameter_once_in_order() {
        let row = listed(query(
            "SELECT * FROM t WHERE a = {{a}} AND b = {{b}} OR a = {{a}}",
            Some("reports"),
        ));
        assert_eq!(row.parameters, ["a", "b"]);
        let text = output::json(&[row]);
        let keys: Vec<String> = serde_json::from_str::<serde_json::Value>(&text).unwrap()[0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys.len(), 7, "{text}");
        assert!(text.find("\"id\"").unwrap() < text.find("\"description\"").unwrap());
        assert!(text.contains("\"description\": null"), "{text}");

        let table = list_table(&[listed(query("SELECT {{x}}, {{y}}", None))]);
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines[0].starts_with("NAME"), "{table}");
        assert!(
            lines[2].contains("x, y") && lines[2].ends_with("sq-1"),
            "{table}"
        );
    }

    #[test]
    fn show_adds_a_newline_only_when_missing() {
        assert_eq!(sql_text("SELECT 1"), "SELECT 1\n");
        assert_eq!(sql_text("SELECT 1\n"), "SELECT 1\n");
        assert_eq!(sql_text("SELECT 1\n\n"), "SELECT 1\n\n");
    }
}
