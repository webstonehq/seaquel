//! The library (phase 5d-1): saved connections, projects, custom labels,
//! saved queries and their versions, as Core writes them.
//!
//! The GUI sends drafts (a new row) and patches (only what changes; see
//! [`Clearable`]). Everything here is pure: it checks the input against the
//! entity's rules and the interface's [`LibraryLimits`], turns a draft into
//! the stored row, applies a patch to a stored row, compares names
//! ([`name_key`]) and plans a version prune ([`version_prune`]). Core reads,
//! checks and writes inside one storage transaction and assigns the ids and
//! times.
//!
//! The rules are pinned by the fixtures in `tests/fixtures/library`, recorded
//! from the TypeScript this replaces; `changes.json` there lists where Core
//! is meant to differ.
//!
//! Nothing here does I/O, panics on its input, or shows a name, a host, a
//! string, query text or a secret in `Debug` or an error message: ids,
//! kinds, flags and counts only.

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::RawValue;

use seaquel_types::storage::{
    ConnectionLabel, PersistedConnection, PersistedProject, PersistedQueryParameter,
    PersistedSavedQuery, SshTunnelConfig,
};

pub use crate::run::INVALID_ARGUMENT;

// ── Codes ──

/// A name another row of the same kind already has (Q3). The error's
/// [`LibraryError::taken_by`] names that row.
pub const NAME_TAKEN: &str = "NAME_TAKEN";
/// `projectRemove` on the only project left (Decision 9).
pub const LAST_PROJECT: &str = "LAST_PROJECT";
pub const PROJECT_NOT_FOUND: &str = "PROJECT_NOT_FOUND";
pub const SAVED_QUERY_NOT_FOUND: &str = "SAVED_QUERY_NOT_FOUND";
pub const LABEL_NOT_FOUND: &str = "LABEL_NOT_FOUND";
/// A connection type this Core has no engine for (the web server's SQLite
/// and DuckDB), the same code `db.connect` answers.
pub const ENGINE_NOT_AVAILABLE: &str = "ENGINE_NOT_AVAILABLE";

// ── Fixed values ──

/// The connection types a saved connection may have (Decision 7).
pub const ENGINE_TYPES: [&str; 6] = ["postgres", "mysql", "mariadb", "sqlite", "mssql", "duckdb"];

/// The predefined labels' ids. They aren't stored in `project_labels`; their
/// names and colours stay in the TypeScript (Decision 10).
pub const PREDEFINED_LABEL_IDS: [&str; 3] = ["local", "staging", "prod"];

/// A saved query parameter's types (Decision 11).
pub const PARAMETER_TYPES: [&str; 5] = ["number", "boolean", "text", "date", "datetime"];

/// The project `projectEnsureDefault` makes on a file with none.
pub const DEFAULT_PROJECT_ID: &str = "default-seaquel";
pub const DEFAULT_PROJECT_NAME: &str = "Seaquel";

/// The id prefixes Core keeps (Decision 1), each followed by a v4 uuid.
pub const CONNECTION_ID_PREFIX: &str = "conn-";
pub const PROJECT_ID_PREFIX: &str = "project-";
pub const LABEL_ID_PREFIX: &str = "label-";
pub const SAVED_QUERY_ID_PREFIX: &str = "saved-";
pub const QUERY_VERSION_ID_PREFIX: &str = "ver-";

/// The `app_state` key of the query version limit, and its default.
pub const QUERY_VERSION_LIMIT_KEY: &str = "query_version_limit";
pub const DEFAULT_VERSION_LIMIT: u32 = 100;

/// The most ids one change event names; past it, the event names none and
/// the GUI reloads the kind (Decision 16).
pub const MAX_EVENT_IDS: usize = 100;

/// The longest id (or scope) one change event carries, in bytes; past it,
/// the event names no ids (a longer scope: no scope either) and the GUI
/// reloads the kind. Core's own ids are about 40 bytes; the storage group's
/// keys are the caller's (phase 5d-1 probe fix).
pub const MAX_EVENT_ID_BYTES: usize = 1024;

/// The most bytes of ids one change event carries; past it, `ids: None`.
pub const MAX_EVENT_IDS_BYTES: usize = 16 * 1024;

// ── Errors ──

/// A refusal: a wire code, a message with no name or value in it, and for
/// [`NAME_TAKEN`] the id of the row that has the name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryError {
    pub code: String,
    pub message: String,
    pub taken_by: Option<String>,
}

impl LibraryError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            taken_by: None,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(INVALID_ARGUMENT, message)
    }

    /// [`NAME_TAKEN`] for a `what` ("connection", "project", …).
    pub fn name_taken(what: &str, taken_by: impl Into<String>) -> Self {
        Self {
            code: NAME_TAKEN.to_string(),
            message: format!("Another {what} here already has this name."),
            taken_by: Some(taken_by.into()),
        }
    }
}

impl fmt::Display for LibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for LibraryError {}

pub(crate) type Checked<T = ()> = Result<T, LibraryError>;

// ── Limits ──

/// What one library call may carry, set per interface with Core's
/// `CoreBuilder::library_limits` (Decision 15). The default is no limit (the
/// desktop, the CLI, MCP); the web server sets every one. A size past its
/// limit is refused with `INVALID_ARGUMENT` naming it, before anything is
/// read; the per-user counts are checked inside the write transaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LibraryLimits {
    /// Names, label names, folders, each tag, and ids, in bytes.
    pub max_name_bytes: Option<usize>,
    /// Host, database, user, connection string, SSL mode, key path,
    /// description and the other free-text fields, in bytes.
    pub max_field_bytes: Option<usize>,
    /// A saved query's text, in bytes.
    pub max_query_bytes: Option<usize>,
    /// Labels on a connection, parameters and tags of a saved query.
    pub max_list_items: Option<usize>,
    /// Saved connections per user (per file).
    pub max_connections: Option<usize>,
    /// Projects per user.
    pub max_projects: Option<usize>,
    /// Saved queries per user.
    pub max_saved_queries: Option<usize>,
    /// The stored bytes of one saved query's versions together: past it,
    /// the oldest are pruned as if the version limit were lower, keeping
    /// at least the newest ([`version_prune`]; phase 5d-1 probe fix).
    pub max_version_bytes: Option<u64>,
}

/// A count check inside the transaction: `INVALID_ARGUMENT` when adding one
/// more would pass `limit` (named `name`).
pub fn check_count(count: u64, limit: Option<usize>, name: &str) -> Checked {
    match limit {
        Some(max) if count >= max as u64 => Err(LibraryError::invalid(format!(
            "You have reached the most allowed here ({name}: {max})."
        ))),
        _ => Ok(()),
    }
}

// ── Patches ──

/// A patch field that can be cleared: absent keeps the stored value, `null`
/// clears it, a value sets it.
pub type Clearable<T> = Option<Option<T>>;

/// Deserializes a [`Clearable`]: a present field (even `null`) is `Some`.
/// With `#[serde(default)]`, an absent one stays `None`.
pub fn clearable<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    d: D,
) -> Result<Clearable<T>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

/// Which of a patch's fields are present, for `Debug`.
pub(crate) fn present<T>(field: &Option<T>) -> bool {
    field.is_some()
}

// ── The change sequence (Decision 17) ──

/// A workspace's change sequence: its `epoch` (the workspace's random id;
/// a new one means the workspace was reopened) and `n`, which follows the
/// order writes committed in.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChangeSeq {
    pub epoch: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub n: u64,
}

/// A list or write result with the sequence it's at least as new as.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Seqd<T> {
    pub value: T,
    pub seq: ChangeSeq,
    /// Phase 5e, Decision 36: what a library write did to its row's file
    /// in a shared project (desktop only). Absent when nothing was
    /// published: no link, a row that isn't shared, nothing that changes
    /// the file, or a Core without `LocalFiles`. A `failed` write leaves
    /// the row stored; the next sync writes the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "ts",
        ts(
            optional,
            type = "{ status: \"written\" | \"deleted\" | \"failed\", code?: string, message?: string }"
        )
    )]
    pub projection: Option<crate::shared::PublishOutcome>,
}

impl<T> Seqd<T> {
    /// A result with no projection outcome.
    pub fn new(value: T, seq: ChangeSeq) -> Self {
        Self {
            value,
            seq,
            projection: None,
        }
    }
}

/// What a `StorageChanged` event is about (Decision 16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum StoredKind {
    Connection,
    Project,
    Label,
    /// A saved query, its versions included.
    SavedQuery,
    History,
    /// A write through the storage group.
    Storage,
    // Phase 5d-2 (Decision 16).
    /// One window's view state of a project (scope: the project; ids: the
    /// window).
    ProjectState,
    /// A saved workflow (scope: the project).
    Workflow,
    /// An app-state setting (ids: the key).
    Setting,
    /// The AI settings record, its providers and their API keys.
    AiSettings,
    /// The theme preferences and the user themes.
    Theme,
    /// A dashboard, its versions included (scope: the project).
    Dashboard,
    /// An AI chat (scope: the connection).
    Chat,
    /// An AI chat's messages (scope: the chat).
    ChatMessages,
    Onboarding,
    Tutorial,
    ImportState,
    /// Phase 5e (Decision 44): the shared repo list, and the files a sync
    /// or a publish wrote (ids: the repo id), so each window refreshes that
    /// repo's git status.
    SharedRepo,
    /// Phase 7a (Decision 6): another connection to the file (another
    /// process, such as the TUI beside the app) committed something; which
    /// rows isn't known (no scope, no ids). Reload every list and setting.
    External,
}

impl StoredKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StoredKind::Connection => "connection",
            StoredKind::Project => "project",
            StoredKind::Label => "label",
            StoredKind::SavedQuery => "savedQuery",
            StoredKind::History => "history",
            StoredKind::Storage => "storage",
            StoredKind::ProjectState => "projectState",
            StoredKind::Workflow => "workflow",
            StoredKind::Setting => "setting",
            StoredKind::AiSettings => "aiSettings",
            StoredKind::Theme => "theme",
            StoredKind::Dashboard => "dashboard",
            StoredKind::Chat => "chat",
            StoredKind::ChatMessages => "chatMessages",
            StoredKind::Onboarding => "onboarding",
            StoredKind::Tutorial => "tutorial",
            StoredKind::ImportState => "importState",
            StoredKind::SharedRepo => "sharedRepo",
            StoredKind::External => "external",
        }
    }
}

// ── Names ──

// `name_key` and the JavaScript trim live in `seaquel_types::names`, so
// storage's data step and its writes compute the stored `name_key` column
// with the very same function (phase 5d-1 probe fix).
use seaquel_types::names::is_js_space;
pub use seaquel_types::names::{js_trim, name_key};

/// The id of the first of `others` (id, name) whose name has `name`'s key,
/// skipping `except` (the row being renamed).
pub fn find_taken<'a>(
    name: &str,
    others: impl IntoIterator<Item = (&'a str, &'a str)>,
    except: Option<&str>,
) -> Option<String> {
    let key = name_key(name);
    others
        .into_iter()
        .filter(|(id, _)| Some(*id) != except)
        .find(|(_, other)| name_key(other) == key)
        .map(|(id, _)| id.to_string())
}

/// The first free name for an import (Decision 13): `name`, else
/// `"<name> (2)"`, `"<name> (3)"`, …, compared by [`name_key`] against
/// `taken` (keys). Linear in the number of taken names.
pub fn free_name(name: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(&name_key(name)) {
        return name.to_string();
    }
    // At most `taken.len() + 1` candidates can be tried before one is free.
    (2..=taken.len() + 2)
        .map(|n| format!("{name} ({n})"))
        .find(|candidate| !taken.contains(&name_key(candidate)))
        .unwrap_or_else(|| format!("{name} ({})", taken.len() + 2))
}

// ── Field checks ──

pub(crate) fn no_nul(s: &str, what: &str) -> Checked {
    if s.contains('\0') {
        Err(LibraryError::invalid(format!(
            "The {what} can't contain a NUL character."
        )))
    } else {
        Ok(())
    }
}

pub(crate) fn within(s: &str, what: &str, limit: Option<usize>, limit_name: &str) -> Checked {
    no_nul(s, what)?;
    match limit {
        Some(max) if s.len() > max => Err(LibraryError::invalid(format!(
            "The {what} is longer than allowed here ({limit_name}: {max} bytes)."
        ))),
        _ => Ok(()),
    }
}

pub(crate) fn field(s: &str, what: &str, limits: &LibraryLimits) -> Checked {
    within(s, what, limits.max_field_bytes, "max_field_bytes")
}

pub(crate) fn opt_field(s: Option<&str>, what: &str, limits: &LibraryLimits) -> Checked {
    s.map_or(Ok(()), |s| field(s, what, limits))
}

/// An id, label id, folder or tag: bounded by `max_name_bytes`.
pub(crate) fn short(s: &str, what: &str, limits: &LibraryLimits) -> Checked {
    within(s, what, limits.max_name_bytes, "max_name_bytes")
}

/// A name: bounded, no NUL, and not empty once trimmed.
pub fn check_name(name: &str, what: &str, limits: &LibraryLimits) -> Checked {
    short(name, &format!("{what} name"), limits)?;
    if js_trim(name).is_empty() {
        return Err(LibraryError::invalid(format!(
            "The {what} name can't be empty."
        )));
    }
    Ok(())
}

pub(crate) fn list_len(len: usize, what: &str, limits: &LibraryLimits) -> Checked {
    match limits.max_list_items {
        Some(max) if len > max => Err(LibraryError::invalid(format!(
            "There are more {what} than allowed here (max_list_items: {max})."
        ))),
        _ => Ok(()),
    }
}

/// A port: a whole number from 0 to 65535 (Decision 7).
pub fn check_port(port: f64, what: &str) -> Checked {
    if port.is_finite() && port.fract() == 0.0 && (0.0..=65535.0).contains(&port) {
        Ok(())
    } else {
        Err(LibraryError::invalid(format!(
            "The {what} must be a whole number from 0 to 65535."
        )))
    }
}

/// A connection type: one of [`ENGINE_TYPES`].
pub fn check_type(ty: &str) -> Checked {
    if ENGINE_TYPES.contains(&ty) {
        Ok(())
    } else {
        Err(LibraryError::invalid(
            "The connection type must be postgres, mysql, mariadb, sqlite, mssql or duckdb.",
        ))
    }
}

/// The engine a connection type connects with (`mariadb` goes through the
/// MySQL engine).
pub fn engine_of(ty: &str) -> &str {
    if ty == "mariadb" {
        "mysql"
    } else {
        ty
    }
}

fn check_tunnel(t: &SshTunnelConfig, limits: &LibraryLimits) -> Checked {
    field(&t.host, "SSH host", limits)?;
    field(&t.username, "SSH user", limits)?;
    field(&t.auth_method, "SSH authentication method", limits)?;
    opt_field(t.key_path.as_deref(), "SSH key path", limits)?;
    check_port(t.port, "SSH port")
}

fn check_label_ids(ids: &[String], limits: &LibraryLimits) -> Checked {
    list_len(ids.len(), "labels", limits)?;
    ids.iter().try_for_each(|id| short(id, "label id", limits))
}

/// Every one of `ids` is predefined or one of `custom` (the project's own):
/// otherwise [`LABEL_NOT_FOUND`] (Decision 7).
pub fn check_labels(ids: &[String], custom: &[ConnectionLabel]) -> Checked {
    let known: HashSet<&str> = custom.iter().map(|l| l.id.as_str()).collect();
    match ids
        .iter()
        .find(|id| !is_predefined_label(id) && !known.contains(id.as_str()))
    {
        Some(_) => Err(LibraryError::new(
            LABEL_NOT_FOUND,
            "A label on the connection isn't one of this project's labels.",
        )),
        None => Ok(()),
    }
}

pub fn is_predefined_label(id: &str) -> bool {
    PREDEFINED_LABEL_IDS.contains(&id)
}

/// A label colour: `#rrggbb`.
pub fn check_colour(color: &str) -> Checked {
    let ok = color.len() == 7
        && color.starts_with('#')
        && color[1..].bytes().all(|b| b.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(LibraryError::invalid(
            "A label colour must be written #rrggbb.",
        ))
    }
}

// ── Connections ──

/// A new saved connection (`connectionCreate`). `Debug` shows the project,
/// type and flags, never a name, host, user or string.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConnectionDraft {
    pub project_id: String,
    pub name: String,
    #[serde(rename = "type")]
    #[cfg_attr(
        feature = "ts",
        ts(type = "\"postgres\" | \"mysql\" | \"sqlite\" | \"mariadb\" | \"mssql\" | \"duckdb\"")
    )]
    pub ty: String,
    pub host: String,
    pub port: f64,
    pub database_name: String,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssl_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub connection_string: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssh_tunnel: Option<SshTunnelConfig>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub save_password: bool,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub save_ssh_password: bool,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub save_ssh_key_passphrase: bool,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<String>>", optional))]
    pub label_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub is_local_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub shared_connection_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ai_share_schema: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ai_share_data: Option<bool>,
    #[serde(
        rename = "activeAIProviderId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub active_ai_provider_id: Option<String>,
    #[serde(
        rename = "activeAIModel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub active_ai_model: Option<String>,
    /// The connection is connected now: `lastConnected` is set to now.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub connected: bool,
    /// An import's draft (Decision 13): a taken name becomes the first free
    /// `"<name> (n)"` instead of `NAME_TAKEN`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub rename_if_taken: bool,
}

impl fmt::Debug for ConnectionDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionDraft")
            .field("project_id", &self.project_id)
            .field("ty", &self.ty)
            .field("connection_string", &self.connection_string.is_some())
            .field("ssh_tunnel", &self.ssh_tunnel.is_some())
            .field("save_password", &self.save_password)
            .field("save_ssh_password", &self.save_ssh_password)
            .field("save_ssh_key_passphrase", &self.save_ssh_key_passphrase)
            .field("labels", &self.label_ids.len())
            .field("connected", &self.connected)
            .field("rename_if_taken", &self.rename_if_taken)
            .finish_non_exhaustive()
    }
}

/// A change to a saved connection (`connectionUpdate`): only the fields it
/// carries change (Decision 2). There is no project: a connection never
/// moves (Decision 7). `Debug` shows which fields are present.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConnectionPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "ts",
        ts(
            optional,
            type = "\"postgres\" | \"mysql\" | \"sqlite\" | \"mariadb\" | \"mssql\" | \"duckdb\""
        )
    )]
    pub ty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub port: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub database_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub username: Option<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssl_mode: Clearable<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub connection_string: Clearable<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssh_tunnel: Clearable<SshTunnelConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub save_password: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub save_ssh_password: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub save_ssh_key_passphrase: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub label_ids: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub is_local_only: Option<bool>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ai_share_schema: Clearable<bool>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ai_share_data: Clearable<bool>,
    #[serde(
        rename = "activeAIProviderId",
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub active_ai_provider_id: Clearable<String>,
    #[serde(
        rename = "activeAIModel",
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub active_ai_model: Clearable<String>,
    /// The connection just connected: `lastConnected` becomes now.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub connected: bool,
}

impl fmt::Debug for ConnectionPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fields: Vec<&str> = [
            ("name", present(&self.name)),
            ("type", present(&self.ty)),
            ("host", present(&self.host)),
            ("port", present(&self.port)),
            ("databaseName", present(&self.database_name)),
            ("username", present(&self.username)),
            ("sslMode", present(&self.ssl_mode)),
            ("connectionString", present(&self.connection_string)),
            ("sshTunnel", present(&self.ssh_tunnel)),
            ("savePassword", present(&self.save_password)),
            ("saveSshPassword", present(&self.save_ssh_password)),
            (
                "saveSshKeyPassphrase",
                present(&self.save_ssh_key_passphrase),
            ),
            ("labelIds", present(&self.label_ids)),
            ("isLocalOnly", present(&self.is_local_only)),
            ("aiShareSchema", present(&self.ai_share_schema)),
            ("aiShareData", present(&self.ai_share_data)),
            ("activeAIProviderId", present(&self.active_ai_provider_id)),
            ("activeAIModel", present(&self.active_ai_model)),
            ("connected", self.connected),
        ]
        .into_iter()
        .filter_map(|(name, on)| on.then_some(name))
        .collect();
        f.debug_struct("ConnectionPatch")
            .field("fields", &fields)
            .finish()
    }
}

/// The keychain entries a connection call writes (Decision 8; desktop
/// only): absent keeps the entry, `null` deletes it, a string sets it.
/// `Debug` shows only which.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SecretChanges {
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub db: Clearable<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssh: Clearable<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub ssh_key: Clearable<String>,
}

impl SecretChanges {
    /// Nothing to write.
    pub fn is_empty(&self) -> bool {
        self.db.is_none() && self.ssh.is_none() && self.ssh_key.is_none()
    }

    /// The entries to set, as (key prefix, value).
    pub fn sets(&self) -> Vec<(&'static str, &str)> {
        self.entries()
            .into_iter()
            .filter_map(|(prefix, v)| match v {
                Some(Some(v)) => Some((prefix, v.as_str())),
                _ => None,
            })
            .collect()
    }

    /// The key prefixes to delete.
    pub fn deletes(&self) -> Vec<&'static str> {
        self.entries()
            .into_iter()
            .filter_map(|(prefix, v)| matches!(v, Some(None)).then_some(prefix))
            .collect()
    }

    fn entries(&self) -> [(&'static str, &Clearable<String>); 3] {
        [
            (DB_SECRET, &self.db),
            (SSH_SECRET, &self.ssh),
            (SSH_KEY_SECRET, &self.ssh_key),
        ]
    }
}

impl fmt::Debug for SecretChanges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn which(v: &Clearable<String>) -> &'static str {
            match v {
                None => "keep",
                Some(None) => "delete",
                Some(Some(_)) => "set",
            }
        }
        f.debug_struct("SecretChanges")
            .field("db", &which(&self.db))
            .field("ssh", &which(&self.ssh))
            .field("ssh_key", &which(&self.ssh_key))
            .finish()
    }
}

/// The keychain key prefixes of a connection's secrets.
pub const DB_SECRET: &str = "db:";
pub const SSH_SECRET: &str = "ssh:";
pub const SSH_KEY_SECRET: &str = "ssh-key:";
/// Every key prefix a connection's secrets use.
pub const CONNECTION_SECRETS: [&str; 3] = [DB_SECRET, SSH_SECRET, SSH_KEY_SECRET];

/// Whether `row` keeps the secret with key prefix `prefix` (its save flag).
pub fn keeps_secret(row: &PersistedConnection, prefix: &str) -> bool {
    match prefix {
        DB_SECRET => row.save_password,
        SSH_SECRET => row.save_ssh_password,
        SSH_KEY_SECRET => row.save_ssh_key_passphrase,
        _ => false,
    }
}

/// The secrets' own checks, before anything is read: no NUL in a value.
pub fn check_secret_values(secrets: &SecretChanges) -> Checked {
    secrets
        .sets()
        .into_iter()
        .try_for_each(|(_, value)| no_nul(value, "password"))
}

/// No secret set whose save flag is off in `row`, the row as it will be
/// stored (Decision 8).
pub fn check_secret_flags(secrets: &SecretChanges, row: &PersistedConnection) -> Checked {
    if secrets
        .sets()
        .into_iter()
        .any(|(prefix, _)| !keeps_secret(row, prefix))
    {
        return Err(LibraryError::invalid(
            "A secret can only be saved when the connection's matching save option is on.",
        ));
    }
    Ok(())
}

/// An id a call names (a row to change, a parent): bounded like a name.
pub fn check_id(id: &str, what: &str, limits: &LibraryLimits) -> Checked {
    short(id, what, limits)
}

/// The draft's own checks, before anything is read: sizes, NULs, the name,
/// type and ports, the label ids' count and sizes.
pub fn check_connection_draft(d: &ConnectionDraft, limits: &LibraryLimits) -> Checked {
    short(&d.project_id, "project id", limits)?;
    check_name(&d.name, "connection", limits)?;
    check_type(&d.ty)?;
    field(&d.host, "host", limits)?;
    check_port(d.port, "port")?;
    field(&d.database_name, "database name", limits)?;
    field(&d.username, "user name", limits)?;
    opt_field(d.ssl_mode.as_deref(), "SSL mode", limits)?;
    opt_field(d.connection_string.as_deref(), "connection string", limits)?;
    if let Some(t) = &d.ssh_tunnel {
        check_tunnel(t, limits)?;
    }
    check_label_ids(&d.label_ids, limits)?;
    opt_field(
        d.shared_connection_id.as_deref(),
        "shared connection id",
        limits,
    )?;
    opt_field(d.active_ai_provider_id.as_deref(), "AI provider id", limits)?;
    opt_field(d.active_ai_model.as_deref(), "AI model", limits)
}

/// The patch's own checks, before anything is read: only the fields it
/// carries.
pub fn check_connection_patch(p: &ConnectionPatch, limits: &LibraryLimits) -> Checked {
    if let Some(name) = &p.name {
        check_name(name, "connection", limits)?;
    }
    if let Some(ty) = &p.ty {
        check_type(ty)?;
    }
    opt_field(p.host.as_deref(), "host", limits)?;
    if let Some(port) = p.port {
        check_port(port, "port")?;
    }
    opt_field(p.database_name.as_deref(), "database name", limits)?;
    opt_field(p.username.as_deref(), "user name", limits)?;
    opt_field(
        p.ssl_mode.as_ref().and_then(Option::as_deref),
        "SSL mode",
        limits,
    )?;
    opt_field(
        p.connection_string.as_ref().and_then(Option::as_deref),
        "connection string",
        limits,
    )?;
    if let Some(Some(t)) = &p.ssh_tunnel {
        check_tunnel(t, limits)?;
    }
    if let Some(ids) = &p.label_ids {
        check_label_ids(ids, limits)?;
    }
    opt_field(
        p.active_ai_provider_id.as_ref().and_then(Option::as_deref),
        "AI provider id",
        limits,
    )?;
    opt_field(
        p.active_ai_model.as_ref().and_then(Option::as_deref),
        "AI model",
        limits,
    )
}

/// A whole connection row's checks (a new one, or one about to be stored):
/// its fields as [`check_connection_draft`] checks them, and its labels
/// against the project's `custom_labels`.
pub fn check_connection(
    row: &PersistedConnection,
    custom_labels: &[ConnectionLabel],
    limits: &LibraryLimits,
) -> Checked {
    check_name(&row.name, "connection", limits)?;
    check_type(&row.ty)?;
    field(&row.host, "host", limits)?;
    check_port(row.port, "port")?;
    field(&row.database_name, "database name", limits)?;
    field(&row.username, "user name", limits)?;
    opt_field(row.ssl_mode.as_deref(), "SSL mode", limits)?;
    opt_field(
        row.connection_string.as_deref(),
        "connection string",
        limits,
    )?;
    check_label_ids(&row.label_ids, limits)?;
    check_labels(&row.label_ids, custom_labels)
}

fn tunnel_json(t: &SshTunnelConfig) -> Option<Box<RawValue>> {
    serde_json::to_string(t)
        .ok()
        .and_then(|s| RawValue::from_string(s).ok())
}

/// The row a draft makes, with Core's `id` and `now`.
pub fn connection_from_draft(id: String, d: &ConnectionDraft, now: &str) -> PersistedConnection {
    PersistedConnection {
        id,
        project_id: d.project_id.clone(),
        name: d.name.clone(),
        ty: d.ty.clone(),
        host: d.host.clone(),
        port: d.port,
        database_name: d.database_name.clone(),
        username: d.username.clone(),
        ssl_mode: d.ssl_mode.clone(),
        connection_string: d.connection_string.clone().filter(|s| !s.is_empty()),
        last_connected: d.connected.then(|| now.to_string()),
        ssh_tunnel: d.ssh_tunnel.as_ref().and_then(tunnel_json),
        save_password: d.save_password,
        save_ssh_password: d.save_ssh_password,
        save_ssh_key_passphrase: d.save_ssh_key_passphrase,
        label_ids: dedup(&d.label_ids),
        is_local_only: d.is_local_only,
        shared_connection_id: d.shared_connection_id.clone(),
        ai_share_schema: d.ai_share_schema,
        ai_share_data: d.ai_share_data,
        active_ai_provider_id: d.active_ai_provider_id.clone(),
        active_ai_model: d.active_ai_model.clone(),
        shared_origin: None,
    }
}

fn dedup(ids: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    ids.iter()
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect()
}

/// Applies `patch` to `row` (Decision 2): a field left out is kept, `null`
/// clears a clearable one, and `connected` sets `lastConnected` to `now`.
pub fn apply_connection_patch(row: &mut PersistedConnection, patch: &ConnectionPatch, now: &str) {
    fn set<T: Clone>(target: &mut T, value: &Option<T>) {
        if let Some(v) = value {
            *target = v.clone();
        }
    }
    fn clear<T: Clone>(target: &mut Option<T>, value: &Clearable<T>) {
        if let Some(v) = value {
            *target = v.clone();
        }
    }
    set(&mut row.name, &patch.name);
    set(&mut row.ty, &patch.ty);
    set(&mut row.host, &patch.host);
    set(&mut row.port, &patch.port);
    set(&mut row.database_name, &patch.database_name);
    set(&mut row.username, &patch.username);
    clear(&mut row.ssl_mode, &patch.ssl_mode);
    if let Some(s) = &patch.connection_string {
        row.connection_string = s.clone().filter(|s| !s.is_empty());
    }
    if let Some(t) = &patch.ssh_tunnel {
        row.ssh_tunnel = t.as_ref().and_then(tunnel_json);
    }
    set(&mut row.save_password, &patch.save_password);
    set(&mut row.save_ssh_password, &patch.save_ssh_password);
    set(
        &mut row.save_ssh_key_passphrase,
        &patch.save_ssh_key_passphrase,
    );
    if let Some(ids) = &patch.label_ids {
        row.label_ids = dedup(ids);
    }
    if let Some(local) = patch.is_local_only {
        row.is_local_only = Some(local);
    }
    clear(&mut row.ai_share_schema, &patch.ai_share_schema);
    clear(&mut row.ai_share_data, &patch.ai_share_data);
    clear(&mut row.active_ai_provider_id, &patch.active_ai_provider_id);
    clear(&mut row.active_ai_model, &patch.active_ai_model);
    if patch.connected {
        row.last_connected = Some(now.to_string());
    }
}

// ── Projects ──

/// A new project (`projectCreate`). `Debug` shows no name or description.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProjectDraft {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub description: Option<String>,
    /// A shared-project import (Decision 13): a taken name becomes the
    /// first free `"<name> (n)"`.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub rename_if_taken: bool,
}

impl fmt::Debug for ProjectDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectDraft")
            .field("description", &self.description.is_some())
            .field("rename_if_taken", &self.rename_if_taken)
            .finish_non_exhaustive()
    }
}

/// A change to a project (`projectUpdate`).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProjectPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub description: Clearable<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub git_repo_path: Clearable<String>,
}

impl fmt::Debug for ProjectPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectPatch")
            .field("name", &self.name.is_some())
            .field("description", &self.description.is_some())
            .field("git_repo_path", &self.git_repo_path.is_some())
            .finish()
    }
}

/// What `projectRemove` removed besides the project: its connections'
/// ids, whose secrets went too.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProjectRemoved {
    pub connection_ids: Vec<String>,
}

pub fn check_project_draft(d: &ProjectDraft, limits: &LibraryLimits) -> Checked {
    check_name(&d.name, "project", limits)?;
    opt_field(d.description.as_deref(), "description", limits)
}

pub fn check_project_patch(p: &ProjectPatch, limits: &LibraryLimits) -> Checked {
    if let Some(name) = &p.name {
        check_name(name, "project", limits)?;
    }
    opt_field(
        p.description.as_ref().and_then(Option::as_deref),
        "description",
        limits,
    )?;
    opt_field(
        p.git_repo_path.as_ref().and_then(Option::as_deref),
        "repository path",
        limits,
    )
}

pub fn project_from_draft(
    id: String,
    d: &ProjectDraft,
    name: String,
    now: &str,
) -> PersistedProject {
    PersistedProject {
        id,
        name,
        description: d.description.clone(),
        created_at: now.to_string(),
        updated_at: now.to_string(),
        custom_labels: Vec::new(),
        git_repo_path: None,
    }
}

/// The default project, made by `projectEnsureDefault` on a file with none.
pub fn default_project(now: &str) -> PersistedProject {
    PersistedProject {
        id: DEFAULT_PROJECT_ID.to_string(),
        name: DEFAULT_PROJECT_NAME.to_string(),
        description: None,
        created_at: now.to_string(),
        updated_at: now.to_string(),
        custom_labels: Vec::new(),
        git_repo_path: None,
    }
}

/// Applies `patch`, and sets `updated_at` to `now`.
pub fn apply_project_patch(row: &mut PersistedProject, patch: &ProjectPatch, now: &str) {
    if let Some(name) = &patch.name {
        row.name = name.clone();
    }
    if let Some(d) = &patch.description {
        row.description = d.clone();
    }
    if let Some(p) = &patch.git_repo_path {
        row.git_repo_path = p.clone();
    }
    row.updated_at = now.to_string();
}

// ── Labels ──

/// A new custom label (`labelCreate`).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LabelDraft {
    pub name: String,
    /// `#rrggbb`.
    pub color: String,
}

impl fmt::Debug for LabelDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LabelDraft").finish_non_exhaustive()
    }
}

/// A change to a custom label (`labelUpdate`).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LabelPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub color: Option<String>,
}

impl fmt::Debug for LabelPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LabelPatch")
            .field("name", &self.name.is_some())
            .field("color", &self.color.is_some())
            .finish()
    }
}

/// What `labelRemove` changed besides the label: the connections that had
/// it, in any project (Decision 10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LabelRemoved {
    pub connection_ids: Vec<String>,
}

pub fn check_label_draft(d: &LabelDraft, limits: &LibraryLimits) -> Checked {
    check_name(&d.name, "label", limits)?;
    check_colour(&d.color)
}

pub fn check_label_patch(p: &LabelPatch, limits: &LibraryLimits) -> Checked {
    if let Some(name) = &p.name {
        check_name(name, "label", limits)?;
    }
    p.color.as_deref().map_or(Ok(()), check_colour)
}

/// A label id a call may change: a custom one. The predefined ids aren't
/// stored and are never removed from connections by a label call.
pub fn check_custom_label_id(id: &str, limits: &LibraryLimits) -> Checked {
    short(id, "label id", limits)?;
    if is_predefined_label(id) {
        return Err(LibraryError::invalid(
            "The predefined labels can't be changed or removed.",
        ));
    }
    Ok(())
}

pub fn apply_label_patch(label: &mut ConnectionLabel, patch: &LabelPatch) {
    if let Some(name) = &patch.name {
        label.name = name.clone();
    }
    if let Some(color) = &patch.color {
        label.color = color.clone();
    }
}

// ── Saved queries ──

/// A new saved query (`savedQueryCreate`). `Debug` shows no name or text.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SavedQueryDraft {
    pub project_id: String,
    pub name: String,
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub parameters: Option<Vec<PersistedQueryParameter>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub database_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tags: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub folder: Option<String>,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub starred: bool,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub shared: bool,
}

impl fmt::Debug for SavedQueryDraft {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SavedQueryDraft")
            .field("project_id", &self.project_id)
            .field("query_bytes", &self.query.len())
            .field("parameters", &self.parameters.as_ref().map(Vec::len))
            .field("tags", &self.tags.as_ref().map(Vec::len))
            .field("starred", &self.starred)
            .field("shared", &self.shared)
            .finish_non_exhaustive()
    }
}

/// A change to a saved query (`savedQueryUpdate`).
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SavedQueryPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub query: Option<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub parameters: Clearable<Vec<PersistedQueryParameter>>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub description: Clearable<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub database_type: Clearable<String>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub tags: Clearable<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "clearable",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub folder: Clearable<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub starred: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub shared: Option<bool>,
}

impl fmt::Debug for SavedQueryPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fields: Vec<&str> = [
            ("name", present(&self.name)),
            ("query", present(&self.query)),
            ("parameters", present(&self.parameters)),
            ("description", present(&self.description)),
            ("databaseType", present(&self.database_type)),
            ("tags", present(&self.tags)),
            ("folder", present(&self.folder)),
            ("starred", present(&self.starred)),
            ("shared", present(&self.shared)),
        ]
        .into_iter()
        .filter_map(|(name, on)| on.then_some(name))
        .collect();
        f.debug_struct("SavedQueryPatch")
            .field("fields", &fields)
            .finish()
    }
}

/// What `savedQueryUpdate` stored: the row, the version it appended (a
/// keyframe of the previous text, only when the text changed) and the
/// versions the prune removed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SavedQueryUpdated {
    pub query: PersistedSavedQuery,
    pub version: Option<seaquel_types::storage::PersistedQueryVersion>,
    pub pruned_version_ids: Vec<String>,
}

fn check_parameters(params: &[PersistedQueryParameter], limits: &LibraryLimits) -> Checked {
    list_len(params.len(), "parameters", limits)?;
    let mut names = HashSet::new();
    for p in params {
        short(&p.name, "parameter name", limits)?;
        if !PARAMETER_TYPES.contains(&p.ty.as_str()) {
            return Err(LibraryError::invalid(
                "A parameter's type must be number, boolean, text, date or datetime.",
            ));
        }
        opt_field(p.default_value.as_deref(), "parameter default", limits)?;
        opt_field(p.description.as_deref(), "parameter description", limits)?;
        if !names.insert(p.name.as_str()) {
            return Err(LibraryError::invalid(
                "Two parameters of the query have the same name.",
            ));
        }
    }
    Ok(())
}

fn check_tags(tags: &[String], limits: &LibraryLimits) -> Checked {
    list_len(tags.len(), "tags", limits)?;
    tags.iter().try_for_each(|t| short(t, "tag", limits))
}

fn check_query_text(query: &str, limits: &LibraryLimits) -> Checked {
    within(query, "query", limits.max_query_bytes, "max_query_bytes")
}

pub fn check_saved_query_draft(d: &SavedQueryDraft, limits: &LibraryLimits) -> Checked {
    short(&d.project_id, "project id", limits)?;
    check_name(&d.name, "saved query", limits)?;
    check_query_text(&d.query, limits)?;
    if let Some(p) = &d.parameters {
        check_parameters(p, limits)?;
    }
    opt_field(d.description.as_deref(), "description", limits)?;
    opt_field(d.database_type.as_deref(), "database type", limits)?;
    if let Some(t) = &d.tags {
        check_tags(t, limits)?;
    }
    d.folder
        .as_deref()
        .map_or(Ok(()), |f| short(f, "folder", limits))
}

pub fn check_saved_query_patch(p: &SavedQueryPatch, limits: &LibraryLimits) -> Checked {
    if let Some(name) = &p.name {
        check_name(name, "saved query", limits)?;
    }
    if let Some(q) = &p.query {
        check_query_text(q, limits)?;
    }
    if let Some(Some(params)) = &p.parameters {
        check_parameters(params, limits)?;
    }
    opt_field(
        p.description.as_ref().and_then(Option::as_deref),
        "description",
        limits,
    )?;
    opt_field(
        p.database_type.as_ref().and_then(Option::as_deref),
        "database type",
        limits,
    )?;
    if let Some(Some(tags)) = &p.tags {
        check_tags(tags, limits)?;
    }
    p.folder
        .as_ref()
        .and_then(Option::as_deref)
        .map_or(Ok(()), |f| short(f, "folder", limits))
}

/// A whole saved query row's checks: the name and text, and the stored
/// parameters and tags, which must parse.
pub fn check_saved_query(row: &PersistedSavedQuery, limits: &LibraryLimits) -> Checked {
    check_name(&row.name, "saved query", limits)?;
    check_query_text(&row.query, limits)?;
    if let Some(raw) = &row.parameters {
        let params: Option<Vec<PersistedQueryParameter>> = serde_json::from_str(raw.get())
            .map_err(|_| LibraryError::invalid("The query's parameters aren't a list."))?;
        check_parameters(params.as_deref().unwrap_or_default(), limits)?;
    }
    if let Some(raw) = &row.tags {
        let tags: Option<Vec<String>> = serde_json::from_str(raw.get())
            .map_err(|_| LibraryError::invalid("The query's tags aren't a list of text."))?;
        check_tags(tags.as_deref().unwrap_or_default(), limits)?;
    }
    opt_field(row.description.as_deref(), "description", limits)?;
    opt_field(row.database_type.as_deref(), "database type", limits)?;
    row.folder
        .as_deref()
        .map_or(Ok(()), |f| short(f, "folder", limits))
}

fn json_of<T: Serialize>(value: &T) -> Option<Box<RawValue>> {
    serde_json::to_string(value)
        .ok()
        .and_then(|s| RawValue::from_string(s).ok())
}

pub fn saved_query_from_draft(id: String, d: &SavedQueryDraft, now: &str) -> PersistedSavedQuery {
    PersistedSavedQuery {
        id,
        name: d.name.clone(),
        query: d.query.clone(),
        project_id: d.project_id.clone(),
        created_at: now.to_string(),
        updated_at: now.to_string(),
        parameters: d.parameters.as_ref().and_then(json_of),
        starred: d.starred,
        shared: d.shared,
        description: d.description.clone(),
        database_type: d.database_type.clone(),
        tags: d.tags.as_ref().and_then(json_of),
        folder: d.folder.clone(),
        // A link is set by the sync, never by a draft.
        shared_path: None,
    }
}

/// What applying a saved query patch changed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SavedQueryChange {
    /// The text before the patch, when the patch changed it: the keyframe
    /// Core appends (Q10).
    pub previous_text: Option<String>,
    /// The name or folder changed: the name is checked again.
    pub renamed: bool,
}

/// Applies `patch` (Decision 11). `updated_at` becomes `now` when anything
/// but `starred` is in the patch, as today.
pub fn apply_saved_query_patch(
    row: &mut PersistedSavedQuery,
    patch: &SavedQueryPatch,
    now: &str,
) -> SavedQueryChange {
    let mut change = SavedQueryChange::default();
    if let Some(name) = &patch.name {
        change.renamed |= name_key(name) != name_key(&row.name);
        row.name = name.clone();
    }
    if let Some(query) = &patch.query {
        if *query != row.query {
            change.previous_text = Some(std::mem::replace(&mut row.query, query.clone()));
        }
    }
    if let Some(p) = &patch.parameters {
        row.parameters = p.as_ref().and_then(json_of);
    }
    if let Some(d) = &patch.description {
        row.description = d.clone();
    }
    if let Some(t) = &patch.database_type {
        row.database_type = t.clone();
    }
    if let Some(t) = &patch.tags {
        row.tags = t.as_ref().and_then(json_of);
    }
    if let Some(f) = &patch.folder {
        change.renamed |= folder_key(f.as_deref()) != folder_key(row.folder.as_deref());
        row.folder = f.clone();
    }
    if let Some(s) = patch.starred {
        row.starred = s;
    }
    if let Some(s) = patch.shared {
        row.shared = s;
    }
    let only_starred = patch.name.is_none()
        && patch.query.is_none()
        && patch.parameters.is_none()
        && patch.description.is_none()
        && patch.database_type.is_none()
        && patch.tags.is_none()
        && patch.folder.is_none()
        && patch.shared.is_none();
    if !only_starred {
        row.updated_at = now.to_string();
    }
    change
}

/// A folder as the name check groups it: no folder (NULL) and `""` are one.
pub fn folder_key(folder: Option<&str>) -> &str {
    folder.unwrap_or("")
}

// ── Versions (Q10) ──

/// A version without its text, for [`version_prune`].
#[derive(Debug, Clone, PartialEq)]
pub struct VersionMeta {
    pub id: String,
    pub version: f64,
    /// It holds a whole text (`snapshot`), not a diff.
    pub keyframe: bool,
    /// The bytes of its stored text (the snapshot or the diff).
    pub bytes: u64,
}

/// The ids to delete so that the newest `keep` versions stay, together with
/// every older one back to the nearest keyframe, so a kept diff still has
/// its base. `keep` 0 keeps everything. No text is resolved.
///
/// With `max_bytes` (`LibraryLimits::max_version_bytes`), the newest
/// versions stay only while their bytes together fit it, and always at
/// least the newest one; the keyframe a kept diff needs stays even past
/// the budget.
pub fn version_prune(versions: &[VersionMeta], keep: u32, max_bytes: Option<u64>) -> Vec<String> {
    let mut ordered: Vec<&VersionMeta> = versions.iter().collect();
    ordered.sort_by(|a, b| a.version.total_cmp(&b.version));
    let n = ordered.len();
    let mut kept = if keep == 0 { n } else { n.min(keep as usize) };
    if let Some(max) = max_bytes {
        let mut total = 0u64;
        let mut fit = 0;
        for v in ordered.iter().rev().take(kept) {
            total = total.saturating_add(v.bytes);
            if total > max && fit > 0 {
                break;
            }
            fit += 1;
        }
        kept = fit;
    }
    if kept >= n {
        return Vec::new();
    }
    // The oldest kept version, then back to a keyframe.
    let mut first = n - kept;
    while first > 0 && !ordered[first].keyframe {
        first -= 1;
    }
    ordered[..first].iter().map(|v| v.id.clone()).collect()
}

/// `query_version_limit` as the TypeScript read it: `parseInt`, and 100
/// when it's unset, unreadable or negative. 0 keeps every version.
pub fn parse_version_limit(value: Option<&str>) -> u32 {
    let Some(value) = value else {
        return DEFAULT_VERSION_LIMIT;
    };
    // `parseInt(value, 10)`: leading space, a sign, then digits.
    let s = value.trim_start_matches(is_js_space);
    let (negative, digits) = match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    };
    let end = digits
        .bytes()
        .position(|b| !b.is_ascii_digit())
        .unwrap_or(digits.len());
    let digits = &digits[..end];
    if digits.is_empty() {
        return DEFAULT_VERSION_LIMIT;
    }
    if negative {
        return if digits.bytes().all(|b| b == b'0') {
            0
        } else {
            DEFAULT_VERSION_LIMIT
        };
    }
    digits.parse::<u32>().unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_keys_fold_and_normalise() {
        assert_eq!(name_key(" ärger db "), name_key("Ärger DB"));
        assert_eq!(name_key("STRASSE"), name_key("Straße"));
        assert_eq!(name_key("Cafe\u{301}"), name_key("Café"));
        assert_eq!(name_key("\u{feff}x\u{3000}"), name_key("X"));
        assert_ne!(name_key("a"), name_key("b"));
    }

    #[test]
    fn free_names_count_up() {
        let taken: HashSet<String> = ["local", "local (2)"].iter().map(|s| name_key(s)).collect();
        assert_eq!(free_name("Local", &taken), "Local (3)");
        assert_eq!(free_name("Other", &taken), "Other");
    }

    #[test]
    fn version_limits_read_like_parse_int() {
        assert_eq!(parse_version_limit(None), 100);
        assert_eq!(parse_version_limit(Some("3")), 3);
        assert_eq!(parse_version_limit(Some(" 12abc")), 12);
        assert_eq!(parse_version_limit(Some("0")), 0);
        assert_eq!(parse_version_limit(Some("-1")), 100);
        assert_eq!(parse_version_limit(Some("-0")), 0);
        assert_eq!(parse_version_limit(Some("x")), 100);
        assert_eq!(parse_version_limit(Some("")), 100);
    }
}
