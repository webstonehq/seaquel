//! `seaquel-cli conn list` and `seaquel-cli conn test`.
//!
//! `list` prints the saved connections, of every project or of one: as a
//! JSON array, or a table of name, type, host, database, project and id. It
//! never prints a connection string (it may hold what a password was
//! stripped from) or anything from the keychain.
//!
//! `test` connects to one saved connection and disconnects again, asking
//! for a missing password or an unknown SSH host key when it can
//! (`connect`), and prints `ok`.

use std::process::ExitCode;

use serde::Serialize;

use crate::connect::{self, Mode};
use crate::output::{self, Format};
use crate::prompt;
use crate::session::{block_on, fail, until_stopped, Session};
use crate::{resolve, ConnArgs, ConnCommand};

const LIST: &str = "conn list";
const TEST: &str = "conn test";

pub fn run(args: ConnArgs) -> ExitCode {
    match args.command {
        ConnCommand::List { project, output } => {
            let format = Format::pick(output.format);
            block_on(LIST, list(project, format))
        }
        ConnCommand::Test { connection, input } => block_on(TEST, test(connection, input.no_input)),
    }
}

/// One saved connection as `list` prints it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Listed {
    id: String,
    name: String,
    #[serde(rename = "type")]
    ty: String,
    project_id: String,
    /// `null` (an empty PROJECT cell in the table) when the row's project
    /// isn't among the listed projects.
    project_name: Option<String>,
    host: String,
    /// `null` when the row has none (0, or not a port).
    port: Option<u16>,
    database: String,
    user: String,
    /// An enabled SSH tunnel.
    ssh: bool,
}

async fn list(project: Option<String>, format: Format) -> ExitCode {
    let s = match Session::open().await {
        Ok(s) => s,
        Err(e) => return fail(LIST, &e),
    };
    let listed = listed(&s, project.as_deref()).await;
    s.close().await;
    match listed {
        Ok(rows) => {
            output::print(&match format {
                Format::Json => output::json(&rows),
                Format::Table => list_table(&rows),
            });
            ExitCode::SUCCESS
        }
        Err(e) => fail(LIST, &e),
    }
}

/// The saved connections, of `project` when given, in storage's order.
async fn listed(
    s: &Session,
    project: Option<&str>,
) -> Result<Vec<Listed>, seaquel_core::CoreError> {
    let projects = s.ws.list_projects().await?.value;
    let only = resolve::project_filter(&projects, project)?;
    let rows = s.ws.list_connections().await?.value;
    Ok(rows
        .into_iter()
        .filter(|r| only.as_ref().is_none_or(|id| *id == r.project_id))
        .map(|r| Listed {
            project_name: projects
                .iter()
                .find(|p| p.id == r.project_id)
                .map(|p| p.name.clone()),
            port: connect::port(r.port).filter(|p| *p != 0),
            ssh: connect::ssh_tunnel(&r).is_some(),
            id: r.id,
            name: r.name,
            ty: r.ty,
            project_id: r.project_id,
            host: r.host,
            database: r.database_name,
            user: r.username,
        })
        .collect())
}

fn list_table(rows: &[Listed]) -> String {
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            // A file's host is a leftover of the form; it has none.
            let host = match (r.ty.as_str(), r.port) {
                ("sqlite" | "duckdb", _) => String::new(),
                (_, Some(port)) => format!("{}:{port}", r.host),
                (_, None) => r.host.clone(),
            };
            vec![
                r.name.clone(),
                r.ty.clone(),
                host,
                r.database.clone(),
                r.project_name.clone().unwrap_or_default(),
                r.id.clone(),
            ]
        })
        .collect();
    output::table(
        &["NAME", "TYPE", "HOST", "DATABASE", "PROJECT", "ID"],
        &cells,
    )
}

async fn test(connection: String, no_input: bool) -> ExitCode {
    let s = match Session::open().await {
        Ok(s) => s,
        Err(e) => return fail(TEST, &e),
    };
    // Stopped: the test's connect is dropped, which closes what it opened.
    let result = until_stopped(TEST, s, async |s| test_one(s, &connection, no_input).await).await;
    match result {
        Ok(Ok(())) => {
            output::print("ok\n");
            ExitCode::SUCCESS
        }
        Ok(Err(e)) => fail(TEST, &e),
        Err(stopped) => stopped,
    }
}

async fn test_one(
    s: &Session,
    wanted: &str,
    no_input: bool,
) -> Result<(), seaquel_core::CoreError> {
    let row = resolve::saved_connection(s, wanted).await?;
    let mut prompter = prompt::for_command(no_input);
    connect::open(s, &row, Mode::Test, &mut prompter).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(ty: &str, port: Option<u16>) -> Listed {
        Listed {
            id: "c-1".into(),
            name: "main db".into(),
            ty: ty.into(),
            project_id: "p1".into(),
            project_name: Some("Main".into()),
            host: "db.internal".into(),
            port,
            database: "app".into(),
            user: "alice".into(),
            ssh: false,
        }
    }

    #[test]
    fn the_table_shows_a_server_s_host_and_port_and_no_host_for_a_file() {
        let text = list_table(&[listed("postgres", Some(5432)), listed("sqlite", None)]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(lines[0].starts_with("NAME"), "{text}");
        assert!(lines[2].contains("db.internal:5432"), "{text}");
        assert!(!lines[3].contains("db.internal"), "{text}");
    }

    #[test]
    fn json_keeps_the_documented_keys_in_order() {
        let text = output::json(&[listed("postgres", None)]);
        let keys: Vec<String> = serde_json::from_str::<serde_json::Value>(&text).unwrap()[0]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            [
                "database",
                "host",
                "id",
                "name",
                "port",
                "projectId",
                "projectName",
                "ssh",
                "type",
                "user"
            ]
        );
        assert!(text.find("\"id\"").unwrap() < text.find("\"name\"").unwrap());
        assert!(text.contains("\"port\": null"), "{text}");
    }
}
