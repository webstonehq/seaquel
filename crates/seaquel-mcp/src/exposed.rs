//! Which saved connections the server exposes (open question 3 of the phase 4
//! plan: only the ones named on the command line), and each one's AI sharing
//! flags.

use seaquel_core::storage::{app_state, connections, projects, Storage};
use seaquel_types::storage::{PersistedConnection, PersistedProject};
use serde_json::Value as Json;

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

/// A connection's AI sharing flags after the global default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sharing {
    pub schema: bool,
    pub data: bool,
}

/// The `app_state` key the GUI keeps its AI settings under
/// (`AI_SETTINGS_KEY` in `src/lib/stores/ai-settings.svelte.ts`).
pub const AI_SETTINGS_KEY: &str = "aiSettings";

/// The GUI's rule, ported from `ui-state.svelte.ts` `_resolveAISettings`
/// (and `ai-assistant.svelte`): a connection's `aiShareSchema`/`aiShareData`
/// when set, else the global `shareSchemaGlobally`/`shareDataGlobally`.
pub fn sharing(row: &PersistedConnection, global: Sharing) -> Sharing {
    Sharing {
        schema: row.ai_share_schema.unwrap_or(global.schema),
        data: row.ai_share_data.unwrap_or(global.data),
    }
}

/// `DEFAULT_AI_SETTINGS` in `src/lib/types/ai.ts`.
pub const DEFAULT_SHARING: Sharing = Sharing {
    schema: true,
    data: false,
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

/// The stored `aiSettings` text to the global flags, exactly as
/// `initialize` then `{ ...DEFAULT_AI_SETTINGS, ...parsed, providers }` and
/// the consumers' truthiness give them:
///
/// - no value, or `""` (`if (raw)`), keeps the defaults;
/// - text `JSON.parse` rejects keeps the defaults (the `catch`);
/// - so does anything the provider migration throws on: `null` (reading
///   `parsed.providers`), a `providers` that is neither absent/`null` nor an
///   array (`.map` isn't a function), and a `null` provider entry (the
///   destructuring);
/// - otherwise a key the parsed object has replaces the default whatever its
///   value (a spread copies `null` too), and the flag is that value's
///   JavaScript truthiness. Arrays and primitives carry no such key.
pub fn global_sharing_from(raw: Option<&str>) -> Sharing {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else {
        return DEFAULT_SHARING;
    };
    let Ok(parsed) = serde_json::from_str::<Json>(raw) else {
        return DEFAULT_SHARING;
    };
    if parsed.is_null() {
        return DEFAULT_SHARING;
    }
    if let Json::Object(obj) = &parsed {
        match obj.get("providers") {
            None | Some(Json::Null) => {}
            Some(Json::Array(items)) if !items.iter().any(Json::is_null) => {}
            Some(_) => return DEFAULT_SHARING,
        }
        return Sharing {
            schema: obj
                .get("shareSchemaGlobally")
                .map_or(DEFAULT_SHARING.schema, truthy),
            data: obj
                .get("shareDataGlobally")
                .map_or(DEFAULT_SHARING.data, truthy),
        };
    }
    // `(5).providers`, `"x".providers` and `[].providers` are undefined, and
    // spreading them adds no settings key.
    DEFAULT_SHARING
}

/// JavaScript's `Boolean(v)` for a JSON value.
fn truthy(v: &Json) -> bool {
    match v {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Json::String(s) => !s.is_empty(),
        Json::Array(_) | Json::Object(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(raw: &str) -> (bool, bool) {
        let s = global_sharing_from(Some(raw));
        (s.schema, s.data)
    }

    #[test]
    fn defaults_when_missing_empty_or_unparseable() {
        assert_eq!(global_sharing_from(None), DEFAULT_SHARING);
        assert_eq!(g(""), (true, false));
        assert_eq!(g("{not json"), (true, false));
        assert_eq!(g("null"), (true, false));
    }

    #[test]
    fn stored_flags_replace_the_defaults() {
        assert_eq!(
            g(r#"{"shareSchemaGlobally":false,"shareDataGlobally":true}"#),
            (false, true)
        );
        assert_eq!(g(r#"{"shareDataGlobally":true}"#), (true, true));
        assert_eq!(g(r#"{"enabled":true}"#), (true, false));
    }

    #[test]
    fn a_present_key_counts_by_truthiness() {
        assert_eq!(
            g(r#"{"shareSchemaGlobally":null,"shareDataGlobally":1}"#),
            (false, true)
        );
        assert_eq!(
            g(r#"{"shareSchemaGlobally":"","shareDataGlobally":"no"}"#),
            (false, true)
        );
        assert_eq!(
            g(r#"{"shareSchemaGlobally":0,"shareDataGlobally":[]}"#),
            (false, true)
        );
    }

    #[test]
    fn what_the_provider_migration_throws_on_keeps_the_defaults() {
        let off = r#""shareSchemaGlobally":false,"shareDataGlobally":true"#;
        assert_eq!(g(&format!(r#"{{"providers":5,{off}}}"#)), (true, false));
        assert_eq!(g(&format!(r#"{{"providers":{{}},{off}}}"#)), (true, false));
        assert_eq!(
            g(&format!(r#"{{"providers":[null],{off}}}"#)),
            (true, false)
        );
        assert_eq!(g(&format!(r#"{{"providers":null,{off}}}"#)), (false, true));
        assert_eq!(
            g(&format!(r#"{{"providers":[{{"id":"a"}},3],{off}}}"#)),
            (false, true)
        );
    }

    #[test]
    fn non_objects_carry_no_settings() {
        assert_eq!(g("5"), (true, false));
        assert_eq!(g("true"), (true, false));
        assert_eq!(g(r#""shareDataGlobally""#), (true, false));
        assert_eq!(g("[true, true]"), (true, false));
    }
}
