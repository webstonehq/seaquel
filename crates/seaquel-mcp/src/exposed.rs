//! Which saved connections the server exposes (open question 3 of the phase 4
//! plan: only the ones named on the command line), and each one's AI sharing
//! flags.

use seaquel_core::storage::{app_state, connections, projects, Storage};
use seaquel_types::storage::{PersistedConnection, PersistedProject};

use crate::error::ToolError;

/// What `seaquel-cli mcp` was asked to expose: `--connection` and
/// `--project` values, each a name or an id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub connections: Vec<String>,
    pub projects: Vec<String>,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.connections.is_empty() && self.projects.is_empty()
    }
}

/// An exposed saved connection, as resolved at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exposed {
    pub id: String,
    pub name: String,
    /// The row's `type`: `postgres`, `mysql`, `mariadb`, `sqlite`, `mssql` or
    /// `duckdb`.
    pub engine: String,
    pub project_id: String,
    pub project_name: String,
}

/// `--connection` names no saved connection, or names two.
pub const CONNECTION_NOT_FOUND: &str = "CONNECTION_NOT_FOUND";
pub const AMBIGUOUS_CONNECTION: &str = "AMBIGUOUS_CONNECTION";
pub const PROJECT_NOT_FOUND: &str = "PROJECT_NOT_FOUND";
pub const AMBIGUOUS_PROJECT: &str = "AMBIGUOUS_PROJECT";

/// Resolve the selection against the saved connections and projects.
///
/// - `--connection v` is the connection whose id is `v`, else the one whose
///   name is `v`. Matching is exact and case-sensitive. A name two saved
///   connections share is an error naming both (with their ids), so the user
///   passes the id instead.
/// - `--project v` is every connection of the project whose id is `v`, else
///   the one whose name is `v`, with the same rules.
///
/// The result has each connection once, in the storage order.
pub async fn resolve(st: &Storage, selection: &Selection) -> Result<Vec<Exposed>, ToolError> {
    if selection.is_empty() {
        return Ok(Vec::new());
    }
    let rows = connections::load_all(st).await?;
    let projects = projects::load_all(st).await?;
    let mut ids: Vec<&str> = Vec::new();

    for wanted in &selection.connections {
        let row = find_connection(&rows, &projects, wanted)?;
        ids.push(&row.id);
    }
    for wanted in &selection.projects {
        let project = find_project(&projects, wanted)?;
        ids.extend(
            rows.iter()
                .filter(|r| r.project_id == project.id)
                .map(|r| r.id.as_str()),
        );
    }

    Ok(rows
        .iter()
        .filter(|r| ids.contains(&r.id.as_str()))
        .map(|r| Exposed {
            id: r.id.clone(),
            name: r.name.clone(),
            engine: r.ty.clone(),
            project_id: r.project_id.clone(),
            project_name: project_name(&projects, &r.project_id),
        })
        .collect())
}

fn project_name(projects: &[PersistedProject], id: &str) -> String {
    projects
        .iter()
        .find(|p| p.id == id)
        .map_or_else(|| id.to_string(), |p| p.name.clone())
}

/// What [`lookup`] found.
pub(crate) enum Found<'a, T> {
    One(&'a T),
    None,
    /// Every item with the name, when no id matched and more than one name
    /// did.
    Many(Vec<&'a T>),
}

/// The item whose id is `wanted`, else the one whose name is. Exact and
/// case-sensitive; a name several items share is [`Found::Many`]. Used for
/// `--connection`, `--project`, a tool's `connection` and a saved query.
pub(crate) fn lookup<'a, T>(
    items: impl IntoIterator<Item = &'a T> + Clone,
    wanted: &str,
    id: impl Fn(&T) -> &str,
    name: impl Fn(&T) -> &str,
) -> Found<'a, T> {
    if let Some(item) = items.clone().into_iter().find(|i| id(i) == wanted) {
        return Found::One(item);
    }
    let mut named: Vec<&T> = items.into_iter().filter(|i| name(i) == wanted).collect();
    match named.len() {
        0 => Found::None,
        1 => Found::One(named.remove(0)),
        _ => Found::Many(named),
    }
}

fn find_connection<'a>(
    rows: &'a [PersistedConnection],
    projects: &[PersistedProject],
    wanted: &str,
) -> Result<&'a PersistedConnection, ToolError> {
    match lookup(rows, wanted, |r| &r.id, |r| &r.name) {
        Found::One(row) => Ok(row),
        Found::None => Err(ToolError::new(
            CONNECTION_NOT_FOUND,
            format!("--connection {wanted:?}: no saved connection has this name or id"),
        )),
        Found::Many(many) => Err(ToolError::new(
            AMBIGUOUS_CONNECTION,
            format!(
                "--connection {wanted:?}: {} saved connections have this name: {}. Pass \
                 --connection with the id instead",
                many.len(),
                many.iter()
                    .map(|r| format!(
                        "id {:?} in project {:?}",
                        r.id,
                        project_name(projects, &r.project_id)
                    ))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        )),
    }
}

fn find_project<'a>(
    projects: &'a [PersistedProject],
    wanted: &str,
) -> Result<&'a PersistedProject, ToolError> {
    match lookup(projects, wanted, |p| &p.id, |p| &p.name) {
        Found::One(project) => Ok(project),
        Found::None => Err(ToolError::new(
            PROJECT_NOT_FOUND,
            format!("--project {wanted:?}: no project has this name or id"),
        )),
        Found::Many(many) => Err(ToolError::new(
            AMBIGUOUS_PROJECT,
            format!(
                "--project {wanted:?}: {} projects have this name (ids {}). Pass --project \
                 with the id instead",
                many.len(),
                many.iter()
                    .map(|p| format!("{:?}", p.id))
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        )),
    }
}

/// The sharing rule, the assistant's too (`seaquel_core::ai::sharing`,
/// Decision 5).
pub use seaquel_core::ai::sharing::{
    global_sharing_from, sharing, Sharing, AI_SETTINGS_KEY, DEFAULT_SHARING,
};

/// The global sharing defaults, read the way `AISettingsStore.initialize`
/// reads them. A failed read leaves the GUI on `DEFAULT_AI_SETTINGS`, and so
/// does it here.
pub async fn global_sharing(st: &Storage) -> Sharing {
    match app_state::get(st, AI_SETTINGS_KEY).await {
        Ok(raw) => global_sharing_from(raw.as_deref()),
        Err(e) => {
            log::warn!("Couldn't read the AI settings, using the defaults: {e}");
            DEFAULT_SHARING
        }
    }
}
