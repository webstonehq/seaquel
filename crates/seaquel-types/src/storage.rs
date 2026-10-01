//! The metadata storage's row types: what `seaquel-storage` saves and loads.
//!
//! Their JSON is the contract the TypeScript repositories in
//! `src/lib/storage/repos/` had, field for field, frozen in
//! `crates/seaquel-storage/tests/fixtures/row-shapes.json`:
//!
//! - camelCase keys, and a field the TypeScript left out when unset is
//!   skipped, while one it sent as `null` is sent as `null`;
//! - numbers are JavaScript numbers (`f64`), sent without a fraction when
//!   they're whole, as `JSON.stringify` writes them;
//! - a column the TypeScript stored as `JSON.stringify(value)` is a
//!   [`RawValue`] holding that JSON text as it came over the wire. Storage
//!   writes it as is and reads it back as is, without checking its shape,
//!   the way the TypeScript did. The TS bindings still give it its type.
//!
//! `RawValue` fields only deserialize through serde_json itself (from text
//! or from a `serde_json::Value`), not through serde's buffered `Content`,
//! so a type holding one mustn't sit behind an untagged or internally tagged
//! enum or a `#[serde(flatten)]`. An adjacently tagged enum is fine as long
//! as the tag comes before the content.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

/// Writes a JavaScript number the way `JSON.stringify` does: whole values
/// without a fraction (`22`, not `22.0`), non-finite ones as `null`.
fn js_number<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    // Below 2^53 every whole f64 is an exact i64.
    if v.fract() == 0.0 && v.abs() < 9_007_199_254_740_992.0 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

fn js_number_opt<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(v) => js_number(v, s),
        None => s.serialize_none(),
    }
}

/// A flag the TypeScript wrote as `v ? 1 : 0`: absent and `null` are false.
fn null_as_false<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    Ok(Option::<bool>::deserialize(d)?.unwrap_or(false))
}

/// A label on a project (`project_labels`), or one in a history item's
/// `connectionLabelsSnapshot`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConnectionLabel {
    pub id: String,
    pub name: String,
    pub is_predefined: bool,
    pub color: String,
}

/// A project (`projects` and its `project_labels`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedProject {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub custom_labels: Vec<ConnectionLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_repo_path: Option<String>,
}

/// A connection's SSH tunnel, stored as JSON in `connections.ssh_tunnel`.
/// Storage keeps that JSON as it came ([`PersistedConnection::ssh_tunnel`]);
/// this type gives it its shape in TypeScript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, optional_fields, rename = "SSHTunnelConfig")
)]
pub struct SshTunnelConfig {
    pub enabled: bool,
    pub host: String,
    #[serde(serialize_with = "js_number")]
    pub port: f64,
    pub username: String,
    #[cfg_attr(feature = "ts", ts(type = "\"password\" | \"key\""))]
    pub auth_method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_path: Option<String>,
}

/// A saved connection (`connections` and its `connection_labels`).
///
/// - `lastConnected` is the stored text, which the TypeScript turns into a
///   `Date` (`new Date(text)`); an empty value is absent.
/// - `sshTunnel` is the stored JSON: absent when the column is NULL, empty
///   or not JSON, and `null` when it holds `null`.
/// - `isLocalOnly` loads as `true` or absent, never `false`.
/// - `aiShareSchema` and `aiShareData` load as absent when NULL.
/// - Saving strips a password from `connectionString`
///   (`seaquel_storage::strip_connection_string_password`).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedConnection {
    pub id: String,
    pub project_id: String,
    pub name: String,
    #[serde(rename = "type")]
    #[cfg_attr(
        feature = "ts",
        ts(type = "\"postgres\" | \"mysql\" | \"sqlite\" | \"mariadb\" | \"mssql\" | \"duckdb\"")
    )]
    pub ty: String,
    pub host: String,
    #[serde(serialize_with = "js_number")]
    pub port: f64,
    pub database_name: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_string: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_connected: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<SshTunnelConfig>", optional = nullable))]
    pub ssh_tunnel: Option<Box<RawValue>>,
    #[serde(default, deserialize_with = "null_as_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub save_password: bool,
    #[serde(default, deserialize_with = "null_as_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub save_ssh_password: bool,
    #[serde(default, deserialize_with = "null_as_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub save_ssh_key_passphrase: bool,
    pub label_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_local_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_share_schema: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_share_data: Option<bool>,
    #[serde(
        rename = "activeAIProviderId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_ai_provider_id: Option<String>,
    #[serde(
        rename = "activeAIModel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub active_ai_model: Option<String>,
}

/// Hides `connection_string`, which can hold a password until
/// `connections::save` strips it.
impl fmt::Debug for PersistedConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersistedConnection")
            .field("id", &self.id)
            .field("project_id", &self.project_id)
            .field("name", &self.name)
            .field("ty", &self.ty)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database_name", &self.database_name)
            .field("username", &self.username)
            .field("ssl_mode", &self.ssl_mode)
            .field(
                "connection_string",
                &self.connection_string.as_ref().map(|_| "<redacted>"),
            )
            .field("last_connected", &self.last_connected)
            .field("ssh_tunnel", &self.ssh_tunnel)
            .field("save_password", &self.save_password)
            .field("save_ssh_password", &self.save_ssh_password)
            .field("save_ssh_key_passphrase", &self.save_ssh_key_passphrase)
            .field("label_ids", &self.label_ids)
            .field("is_local_only", &self.is_local_only)
            .field("shared_connection_id", &self.shared_connection_id)
            .field("ai_share_schema", &self.ai_share_schema)
            .field("ai_share_data", &self.ai_share_data)
            .field("active_ai_provider_id", &self.active_ai_provider_id)
            .field("active_ai_model", &self.active_ai_model)
            .finish()
    }
}

/// This machine's settings for a shared connection (`connection_overrides`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedConnectionOverride {
    pub shared_connection_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_override: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_number_opt"
    )]
    pub port_override: Option<f64>,
    #[serde(default, deserialize_with = "null_as_false")]
    pub save_password: bool,
    #[serde(default, deserialize_with = "null_as_false")]
    pub save_ssh_password: bool,
    #[serde(default, deserialize_with = "null_as_false")]
    pub save_ssh_key_passphrase: bool,
}

// ---------------------------------------------------------------------------
// Project state (project_state, tabs, saved_canvases)
// ---------------------------------------------------------------------------

/// A query tab. `queryId` loads as the saved query's id, or the shared
/// query's when there's no saved one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedQueryTab {
    pub id: String,
    pub name: String,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedSchemaTab {
    pub id: String,
    pub table_name: String,
    pub schema_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedExplainTab {
    pub id: String,
    pub name: String,
    pub source_query: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedErdTab {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedStatisticsTab {
    pub id: String,
    pub name: String,
    pub connection_id: String,
}

/// A workflow tab. Stored with `tab_type = 'canvas'`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedWorkflowTab {
    pub id: String,
    pub name: String,
    pub connection_id: String,
}

/// A starter tab. A NULL `starter_type` loads as `getting-started`, and any
/// other stored text as it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedStarterTab {
    pub id: String,
    #[serde(rename = "type")]
    #[cfg_attr(feature = "ts", ts(type = "\"getting-started\" | \"migration-tips\""))]
    pub ty: String,
    pub name: String,
    #[serde(default, deserialize_with = "null_as_false")]
    pub closable: bool,
}

/// A connection tab. Accepted by `project_state::save` and not stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedConnectionTab {
    pub id: String,
    pub name: String,
}

/// A dashboard tab. The dashboard id is stored in `tabs.source_query`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedDashboardTab {
    pub id: String,
    pub name: String,
    pub dashboard_id: String,
}

/// A table-editor tab. The definition (JSON text) is stored in
/// `tabs.source_query`; a NULL one loads as `"{}"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedCreateTableTab {
    pub id: String,
    pub connection_id: String,
    pub name: String,
    pub table_definition: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedDataTab {
    pub id: String,
    pub connection_id: String,
    pub table_name: String,
    pub schema_name: String,
}

/// A DuckDB extensions tab. Accepted by `project_state::save` and not
/// stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedExtensionsDuckdbTab {
    pub id: String,
    pub name: String,
    pub connection_id: String,
}

/// One pane of the split layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedPane {
    pub id: String,
    pub tab_ids: Vec<String>,
    pub active_tab_id: Option<String>,
}

/// The split pane layout, stored as JSON in `project_state.pane_layout`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedPaneLayout {
    pub panes: Vec<PersistedPane>,
    pub active_pane_id: String,
}

/// A project's open tabs and workspace layout.
///
/// Loading fills every list (`[]` when there's none) and every active id
/// (`null`). The JSON columns load as their stored JSON, with `[]` for
/// NULL or unparseable text; stored `null` loads as `null`. `paneLayout` is
/// absent unless the column holds something. `connectionTabs`,
/// `activeConnectionTabId` and the `extensionsDuckdb*` fields are accepted
/// and not stored. `savedWorkflows` are the stored JSON, in the tagged form
/// `$lib/values`' `toStorable` gives them: the TypeScript decodes them with
/// `fromStorable` and drops any that don't decode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedProjectState {
    pub project_id: String,
    pub query_tabs: Vec<PersistedQueryTab>,
    pub schema_tabs: Vec<PersistedSchemaTab>,
    pub explain_tabs: Vec<PersistedExplainTab>,
    pub erd_tabs: Vec<PersistedErdTab>,
    #[serde(default)]
    #[cfg_attr(
        feature = "ts",
        ts(as = "Option<Vec<PersistedStatisticsTab>>", optional)
    )]
    pub statistics_tabs: Vec<PersistedStatisticsTab>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<PersistedWorkflowTab>>", optional))]
    pub workflow_tabs: Vec<PersistedWorkflowTab>,
    /// `null` when the column holds `null`.
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional = false))]
    pub tab_order: Box<RawValue>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional = nullable))]
    pub connection_order: Option<Box<RawValue>>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = false))]
    pub active_query_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = false))]
    pub active_schema_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = false))]
    pub active_explain_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = false))]
    pub active_erd_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub active_statistics_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub active_workflow_tab_id: Option<String>,
    /// `ActiveViewType`. A stored `canvas` (from before the workflow rename)
    /// loads as it is.
    pub active_view: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = false))]
    pub active_connection_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<PersistedStarterTab>>", optional))]
    pub starter_tabs: Vec<PersistedStarterTab>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub active_starter_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "Array<unknown>", optional))]
    pub saved_workflows: Vec<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_tabs: Option<Vec<PersistedConnectionTab>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub active_connection_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(
        feature = "ts",
        ts(as = "Option<Vec<PersistedDashboardTab>>", optional)
    )]
    pub dashboard_tabs: Vec<PersistedDashboardTab>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub active_dashboard_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional = nullable))]
    pub starred_shared_query_ids: Option<Box<RawValue>>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional = nullable))]
    pub starred_shared_dashboard_ids: Option<Box<RawValue>>,
    #[serde(default)]
    #[cfg_attr(
        feature = "ts",
        ts(as = "Option<Vec<PersistedCreateTableTab>>", optional)
    )]
    pub create_table_tabs: Vec<PersistedCreateTableTab>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub active_create_table_tab_id: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<PersistedDataTab>>", optional))]
    pub data_tabs: Vec<PersistedDataTab>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub active_data_tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions_duckdb_tabs: Option<Vec<PersistedExtensionsDuckdbTab>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub active_extensions_duckdb_tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<PersistedPaneLayout>", optional = nullable))]
    pub pane_layout: Option<Box<RawValue>>,
}

// ---------------------------------------------------------------------------
// Saved queries and their versions
// ---------------------------------------------------------------------------

/// A `{{name}}` parameter of a saved query, stored as JSON in
/// `saved_queries.parameters`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedQueryParameter {
    pub name: String,
    #[serde(rename = "type")]
    #[cfg_attr(
        feature = "ts",
        ts(type = "\"number\" | \"boolean\" | \"text\" | \"date\" | \"datetime\"")
    )]
    pub ty: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A saved query (`saved_queries`). `parameters` and `tags` are the stored
/// JSON: absent when NULL or unparseable, `null` when the column holds
/// `null`. `starred` and `shared` always load.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedSavedQuery {
    pub id: String,
    pub name: String,
    pub query: String,
    pub project_id: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "ts",
        ts(as = "Option<Vec<PersistedQueryParameter>>", optional = nullable)
    )]
    pub parameters: Option<Box<RawValue>>,
    #[serde(default, deserialize_with = "null_as_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub starred: bool,
    #[serde(default, deserialize_with = "null_as_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub shared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional = nullable))]
    pub tags: Option<Box<RawValue>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// One version of a saved query (`query_versions`): a keyframe holds
/// `snapshot`, a delta holds `diff` (diff-match-patch text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedQueryVersion {
    pub id: String,
    pub query_id: String,
    #[serde(serialize_with = "js_number")]
    pub version: f64,
    #[serde(default)]
    pub snapshot: Option<String>,
    #[serde(default)]
    pub diff: Option<String>,
    pub created_at: String,
}

/// A prune of a saved query's versions, computed in TypeScript (where the
/// diffs are resolved) and run by `query_versions::prune` in one
/// transaction: delete `deleteIds`, then turn `promote` into a keyframe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct QueryVersionsPrune {
    pub saved_query_id: String,
    pub delete_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promote: Option<QueryVersionPromote>,
}

/// The oldest surviving version of a prune, with its resolved text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct QueryVersionPromote {
    pub id: String,
    pub snapshot: String,
}

/// A query history item (`query_history`). `connectionLabelsSnapshot` is
/// the stored JSON, `[]` when NULL or unparseable.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedQueryHistoryItem {
    pub id: String,
    pub query: String,
    pub timestamp: String,
    #[serde(serialize_with = "js_number")]
    pub execution_time: f64,
    #[serde(serialize_with = "js_number")]
    pub row_count: f64,
    pub connection_id: String,
    #[serde(default, deserialize_with = "null_as_false")]
    pub favorite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "ts",
        ts(as = "Option<Vec<ConnectionLabel>>", optional = false)
    )]
    pub connection_labels_snapshot: Option<Box<RawValue>>,
    pub connection_name_snapshot: String,
}

// ---------------------------------------------------------------------------
// Shared repos, themes, singletons
// ---------------------------------------------------------------------------

/// A shared query repository, stored as JSON in `shared_repos.data`.
/// Storage keeps that JSON as it came (see [`SharedReposState`]); this type
/// gives it its shape in TypeScript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedSharedQueryRepo {
    pub id: String,
    pub name: String,
    pub path: String,
    pub remote_url: String,
    pub branch: String,
    #[cfg_attr(feature = "ts", ts(optional = false))]
    pub last_sync_at: Option<String>,
    #[cfg_attr(
        feature = "ts",
        ts(
            type = "\"synced\" | \"ahead\" | \"behind\" | \"diverged\" | \"error\" | \"uninitialized\""
        )
    )]
    pub sync_status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials_id: Option<String>,
}

/// What `shared_repos::load_all` returns: every stored repo that parses
/// (as its stored JSON) and `app_state['activeRepoId']`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SharedReposState {
    #[cfg_attr(feature = "ts", ts(as = "Vec<PersistedSharedQueryRepo>"))]
    pub repos: Vec<Box<RawValue>>,
    pub active_repo_id: Option<String>,
}

/// The light and dark theme ids (`theme_preferences`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ThemePreferences {
    pub light_theme_id: String,
    pub dark_theme_id: String,
}

/// One row of `tutorial_progress`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TutorialProgress {
    pub lesson_id: String,
    pub challenge_id: String,
    pub state: Option<String>,
}

/// One row of `import_state`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ImportState {
    pub has_offered_import: bool,
    pub last_check_timestamp: Option<String>,
}

// ---------------------------------------------------------------------------
// Dashboards
// ---------------------------------------------------------------------------

/// A dashboard (`dashboards`). `viewport` and `widgets` are JSON text the
/// TypeScript builds itself. `dateFilter` always loads (`null` when unset);
/// a NULL `starred` loads as `false`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedDashboard {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub viewport: String,
    pub widgets: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional = nullable))]
    pub date_filter: Option<String>,
    #[serde(default, deserialize_with = "null_as_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub starred: bool,
    #[serde(default, deserialize_with = "null_as_false")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub shared: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// One version of a dashboard (`dashboard_versions`), always a full
/// snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedDashboardVersion {
    pub id: String,
    pub dashboard_id: String,
    #[serde(serialize_with = "js_number")]
    pub version: f64,
    pub snapshot: String,
    pub created_at: String,
}

/// A dashboard version without its snapshot (`dashboardVersionsList` and
/// `dashboardUpdate`'s new version, phase 5d-2 Task 7): what the version
/// history shows. `dashboardVersionGet` answers one version whole.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedDashboardVersionMeta {
    pub id: String,
    pub dashboard_id: String,
    #[serde(serialize_with = "js_number")]
    pub version: f64,
    pub created_at: String,
    /// How many widgets the snapshot holds; `None` when its `widgets`
    /// isn't a list (or the snapshot isn't JSON).
    pub widget_count: Option<u32>,
    /// The snapshot's size in bytes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes: u64,
}

/// A saved workflow without its body (`workflowsList`, phase 5d-2 Task 7):
/// what the workflow sidebar shows. `workflowGet` answers one workflow
/// whole. `name` is `""` when the stored JSON has no text `name`; the
/// times are `None` when it has no text time. `Debug` leaves the name out.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedWorkflowMeta {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// The stored JSON's size in bytes.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub bytes: u64,
}

impl fmt::Debug for PersistedWorkflowMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersistedWorkflowMeta")
            .field("id", &self.id)
            .field("project_id", &self.project_id)
            .field("name_bytes", &self.name.len())
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

/// A prune of a dashboard's versions, computed in TypeScript and run by
/// `dashboard_versions::prune` in one transaction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DashboardVersionsPrune {
    pub dashboard_id: String,
    pub delete_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// AI chats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedAIChat {
    pub id: String,
    pub connection_id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PersistedAIMessage {
    pub id: String,
    pub chat_id: String,
    #[cfg_attr(feature = "ts", ts(type = "\"user\" | \"assistant\""))]
    pub role: String,
    pub content: String,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dashboard_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Web vault
// ---------------------------------------------------------------------------

/// The vault's Argon2id parameters, stored as JSON in
/// `vault_state.kdf_params`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VaultKdfParams {
    #[serde(serialize_with = "js_number")]
    pub version: f64,
    #[serde(serialize_with = "js_number")]
    pub t: f64,
    #[serde(serialize_with = "js_number")]
    pub m: f64,
    #[serde(serialize_with = "js_number")]
    pub p: f64,
}

/// The web vault's singleton row (`vault_state`): what the browser needs to
/// re-derive the vault key from a passphrase. Base64 text throughout.
/// Loading fails when `kdf_params` isn't JSON, as the TypeScript's bare
/// `JSON.parse` did. `Debug` leaves out the verifier and its nonce.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedVaultState {
    pub salt: String,
    #[cfg_attr(feature = "ts", ts(as = "VaultKdfParams"))]
    pub kdf_params: Box<RawValue>,
    pub verifier: String,
    pub verifier_nonce: String,
    pub created_at: String,
}

impl fmt::Debug for PersistedVaultState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersistedVaultState")
            .field("salt", &self.salt)
            .field("kdf_params", &self.kdf_params)
            .field("verifier", &"<redacted>")
            .field("verifier_nonce", &"<redacted>")
            .field("created_at", &self.created_at)
            .finish()
    }
}

/// One vault-encrypted credential (`user_credentials`). `scope` is the
/// keyring category (`db`, `ssh`, `ssh-key`, …) and `key` the connection or
/// provider id, empty for singletons. `nonce` and `ciphertext` are base64.
/// `Debug` leaves them out.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PersistedCredential {
    pub scope: String,
    pub key: String,
    pub nonce: String,
    pub ciphertext: String,
    pub updated_at: String,
}

impl fmt::Debug for PersistedCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersistedCredential")
            .field("scope", &self.scope)
            .field("key", &self.key)
            .field("nonce", &"<redacted>")
            .field("ciphertext", &"<redacted>")
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_numbers_serialize_without_a_fraction() {
        let v = PersistedDashboardVersion {
            id: "v".into(),
            dashboard_id: "d".into(),
            version: 3.0,
            snapshot: "{}".into(),
            created_at: "c".into(),
        };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains(r#""version":3,"#), "{json}");
        let h: PersistedQueryHistoryItem = serde_json::from_str(
            r#"{"id":"h","query":"q","timestamp":"t","executionTime":12.5,"rowCount":-0,
                "connectionId":"c","favorite":true,"connectionNameSnapshot":""}"#,
        )
        .unwrap();
        let json = serde_json::to_string(&h).unwrap();
        assert!(
            json.contains(r#""executionTime":12.5,"rowCount":0,"#),
            "{json}"
        );
    }

    #[test]
    fn raw_json_fields_keep_their_text_and_null() {
        let q: PersistedSavedQuery = serde_json::from_str(
            r#"{"id":"q","name":"n","query":"SELECT 1","projectId":"p","createdAt":"c",
                "updatedAt":"u","tags":["b","a"],"parameters":null,"starred":null}"#,
        )
        .unwrap();
        assert_eq!(q.tags.as_deref().map(RawValue::get), Some(r#"["b","a"]"#));
        assert!(q.parameters.is_none());
        assert!(!q.starred);
        // A raw value deserializes from a serde_json::Value too (Tauri's path).
        let v = serde_json::json!({"projectId": "p", "queryTabs": [], "schemaTabs": [],
            "explainTabs": [], "erdTabs": [], "tabOrder": ["b", "a"], "activeView": "query"});
        let s: PersistedProjectState = serde_json::from_value(v).unwrap();
        assert_eq!(s.tab_order.get(), r#"["b","a"]"#);
    }

    #[test]
    fn debug_hides_secrets() {
        let c = PersistedCredential {
            scope: "db".into(),
            key: "c1".into(),
            nonce: "NONCE-VALUE".into(),
            ciphertext: "CIPHER-VALUE".into(),
            updated_at: "u".into(),
        };
        let d = format!("{c:?}");
        assert!(
            !d.contains("NONCE-VALUE") && !d.contains("CIPHER-VALUE"),
            "{d}"
        );

        let conn: PersistedConnection = serde_json::from_str(
            r#"{"id":"c","projectId":"p","name":"n","type":"mssql","host":"h","port":1433,
                "databaseName":"d","username":"u","labelIds":[],
                "connectionString":"Server=h;Password=hunter2"}"#,
        )
        .unwrap();
        let d = format!("{conn:?}");
        assert!(!d.contains("hunter2"), "{d}");

        let vault: PersistedVaultState = serde_json::from_str(
            r#"{"salt":"c2FsdA==","kdfParams":{"version":1,"t":3,"m":65536,"p":1},
                "verifier":"VERIFIER-VALUE","verifierNonce":"NONCE-VALUE","createdAt":"c"}"#,
        )
        .unwrap();
        let d = format!("{vault:?}");
        assert!(
            !d.contains("VERIFIER-VALUE") && !d.contains("NONCE-VALUE"),
            "{d}"
        );
    }
}
