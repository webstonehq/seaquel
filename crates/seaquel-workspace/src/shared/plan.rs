//! Pairing and the sync and publish plans.
//!
//! [`plan_sync`] takes a scan of one linked project's directory and all
//! that project's rows with their links (`shared_path`, `shared_base`,
//! `shared_file_id`; for a connection, the path in
//! `shared_connection_id`), and answers which rows to create, update or
//! unshare, which files to write, the links to store and the notices.
//! [`plan_publish`] answers the file operations for one row a library call
//! just changed. Neither reads a file or the clock; ids come from an
//! [`IdSource`].
//!
//! **Hashes.** A file hashes as `write(parse(text))` and a
//! row as `write(parse(write(row)))`, `write` being the kind's
//! `*_content` (no id; no viewport for a dashboard, no labels for a
//! template). See [`row_hash`].
//!
//! **Pairing**, each file with at most one row and each row with at most
//! one file: by the file's id (when no other file in the scan has the same
//! id), then the stored path, then the name (`name_key`, and the folder
//! for queries; a file at the row's slug path wins a tie), then a file at
//! the row's slug path. A file that claims, by name, a row another file
//! has is `Unpaired` and isn't imported. Only shared queries and
//! dashboards, and only connections linked to a template,
//! pair.
//!
//! **Order for Core.** `SyncPlan::rows`, applied in plan order (a rename
//! that frees a name comes before the op that takes it), and the `links`
//! without `pending_on` go in one transaction; then the `files` (independent
//! writes: one failing doesn't stop the others); then each link whose
//! `pending_on` write succeeded. `PublishPlan::files` is a sequence: stop
//! at the first failure, store `on_failure` (when the failure is a failed
//! write, not a refusal), else `on_success`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;

use seaquel_types::storage::{
    PersistedConnection, PersistedDashboard, PersistedQueryParameter, PersistedSavedQuery,
    SshTunnelConfig,
};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::value::RawValue;

use super::content_hash;
use super::format::{
    dashboard_content, parse_dashboard, parse_project, parse_query, parse_query_core,
    parse_template, parse_template_core, query_content, template_content, write_dashboard,
    write_project, write_query, write_template, DashboardFile, ProjectFile, QueryFile,
    TemplateFile, TemplateSsh, DEFAULT_VIEWPORT,
};
use super::names::{check_folder, file_stem, free_path, legacy_stem, path_key, TakenPaths};
use crate::library::{
    check_connection_draft, check_connection_patch, check_saved_query_draft,
    check_saved_query_patch, folder_key, free_name, js_trim, name_key, ConnectionDraft,
    ConnectionPatch, LibraryError, LibraryLimits, SavedQueryDraft, SavedQueryPatch,
};
use crate::state::{
    check_dashboard_draft, check_dashboard_patch, DashboardDraft, DashboardPatch, StateLimits,
};

/// The kinds of rows that have files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "SharedKind"))]
pub enum Kind {
    SavedQuery,
    Dashboard,
    Connection,
}

/// A row's link to its file (migration `0004`). `None` is NULL: a path
/// NULL means today's rule (the slug path), a base NULL means no sync has
/// recorded one (a conflict then goes to the file).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Link {
    /// Repo-relative (`.seaquel/projects/<dir>/…`).
    pub path: Option<String>,
    /// [`content_hash`] of the content both sides had at the last sync.
    pub base: Option<String>,
    pub file_id: Option<String>,
}

impl fmt::Debug for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Link")
            .field("path", &self.path.is_some())
            .field("base", &self.base.is_some())
            .field("file_id", &self.file_id.is_some())
            .finish()
    }
}

/// Where a linked project's files are: its repo (the id goes into
/// `shared_connection_id`) and its directory under `.seaquel/projects/`.
#[derive(Clone, PartialEq, Eq)]
pub struct ProjectLink {
    pub repo_id: String,
    pub dir: String,
}

impl fmt::Debug for ProjectLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The directory is a project's name.
        f.debug_struct("ProjectLink")
            .field("repo_id", &self.repo_id)
            .finish_non_exhaustive()
    }
}

impl ProjectLink {
    /// `.seaquel/projects/<dir>`.
    pub fn root(&self) -> String {
        format!("{}/projects/{}", super::SEAQUEL_DIR, self.dir)
    }

    /// The directory a kind's files live in.
    pub fn kind_dir(&self, kind: Kind) -> String {
        let sub = match kind {
            Kind::SavedQuery => "queries",
            Kind::Dashboard => "dashboards",
            Kind::Connection => "connections",
        };
        format!("{}/{sub}", self.root())
    }

    pub fn project_yaml(&self) -> String {
        format!("{}/project.yaml", self.root())
    }

    /// A connection's `shared_connection_id` for a template at `path`:
    /// `<repoId>:<path>`, as older releases match it.
    pub fn template_link(&self, path: &str) -> String {
        format!("{}:{path}", self.repo_id)
    }
}

/// The repo-relative path in a `shared_connection_id` (`<repoId>:<path>`).
pub fn template_path(shared_connection_id: &str) -> Option<&str> {
    let at = shared_connection_id
        .find(":.seaquel/")
        .or_else(|| shared_connection_id.find(':'))?;
    Some(&shared_connection_id[at + 1..]).filter(|p| !p.is_empty())
}

/// A file the scan read.
#[derive(Clone, PartialEq, Eq)]
pub struct RawFile {
    pub rel_path: String,
    pub text: String,
}

impl fmt::Debug for RawFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawFile")
            .field("bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// Why the scan skipped a path, or why the planner did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SkipReason {
    Symlink,
    Unreadable,
    NotUtf8,
    TooLarge,
    TooMany,
    DoesNotParse,
    /// The file parses, but the row it would make (or the change it would
    /// make to one) is one the library refuses: an unknown parameter type,
    /// two parameters of one name, a NUL, an engine Core doesn't know, a
    /// port outside 0–65535, a limit (C1). Nothing changes on either side.
    Invalid,
}

/// A file or directory the scan didn't read. Nothing under it reads as
/// missing.
#[derive(Clone, PartialEq, Eq)]
pub struct Skipped {
    pub rel_path: String,
    pub why: SkipReason,
}

impl fmt::Debug for Skipped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Skipped")
            .field("why", &self.why)
            .finish_non_exhaustive()
    }
}

/// One project directory as `seaquel-git`'s `tree::scan` read it.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct DirScan {
    pub files: Vec<RawFile>,
    pub skipped: Vec<Skipped>,
    /// The repo has conflicted files: nothing is planned.
    pub conflicted: bool,
}

impl fmt::Debug for DirScan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DirScan")
            .field("files", &self.files.len())
            .field("skipped", &self.skipped)
            .field("conflicted", &self.conflicted)
            .finish()
    }
}

#[derive(Clone)]
pub struct LinkedQuery {
    pub row: PersistedSavedQuery,
    pub link: Link,
}

#[derive(Clone)]
pub struct LinkedDashboard {
    pub row: PersistedDashboard,
    pub link: Link,
}

/// A connection and its link; `link.path` is the path in
/// `shared_connection_id`.
#[derive(Clone)]
pub struct LinkedConnection {
    pub row: PersistedConnection,
    pub link: Link,
}

macro_rules! linked_debug {
    ($t:ty, $name:literal) => {
        impl fmt::Debug for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct($name)
                    .field("id", &self.row.id)
                    .field("link", &self.link)
                    .finish_non_exhaustive()
            }
        }
    };
}
linked_debug!(LinkedQuery, "LinkedQuery");
linked_debug!(LinkedDashboard, "LinkedDashboard");
linked_debug!(LinkedConnection, "LinkedConnection");

/// All of one project's queries, dashboards and connections, shared or
/// not (a row that isn't shared still takes its name, `NameTaken`).
#[derive(Clone, Debug, Default)]
pub struct SharedRows {
    pub project_id: String,
    pub queries: Vec<LinkedQuery>,
    pub dashboards: Vec<LinkedDashboard>,
    pub connections: Vec<LinkedConnection>,
}

/// Where new ids come from: Core's are `<prefix><uuid v4>` for rows and a
/// uuid v4 for files.
pub trait IdSource {
    fn row_id(&mut self, kind: Kind) -> String;
    fn file_id(&mut self) -> String;
}

/// A row write a sync plans.
#[derive(Clone)]
pub enum RowOp {
    CreateQuery {
        id: String,
        draft: SavedQueryDraft,
    },
    /// The file's content (`R = B, F ≠ B`, or the file winning a conflict); Core
    /// keeps the previous text as a version (its keyframe rule). Also a
    /// folder that follows the file.
    UpdateQuery {
        id: String,
        patch: SavedQueryPatch,
    },
    CreateDashboard {
        id: String,
        draft: DashboardDraft,
    },
    /// The file's content, with `capture_version`.
    UpdateDashboard {
        id: String,
        patch: DashboardPatch,
    },
    /// A template's connection: `rename_if_taken`, local-only off, linked;
    /// Core appends it to the project's connection order.
    CreateConnection {
        id: String,
        draft: ConnectionDraft,
    },
    /// The template's fields: name, host, port, database,
    /// SSL mode and SSH host and port. Never the user name, a secret,
    /// labels or the type.
    UpdateConnection {
        id: String,
        patch: ConnectionPatch,
    },
    /// The row stays: a query or dashboard is no longer shared, a
    /// connection becomes local-only. Its link is cleared by a
    /// [`LinkUpdate`].
    Unshare {
        kind: Kind,
        id: String,
    },
}

impl fmt::Debug for RowOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowOp::CreateQuery { id, draft } => write!(f, "CreateQuery({id}, {draft:?})"),
            RowOp::UpdateQuery { id, patch } => write!(f, "UpdateQuery({id}, {patch:?})"),
            RowOp::CreateDashboard { id, draft } => write!(f, "CreateDashboard({id}, {draft:?})"),
            RowOp::UpdateDashboard { id, patch } => write!(f, "UpdateDashboard({id}, {patch:?})"),
            RowOp::CreateConnection { id, draft } => {
                write!(f, "CreateConnection({id}, {draft:?})")
            }
            RowOp::UpdateConnection { id, patch } => {
                write!(f, "UpdateConnection({id}, {patch:?})")
            }
            RowOp::Unshare { kind, id } => write!(f, "Unshare({kind:?}, {id})"),
        }
    }
}

/// A row's whole new link (Core's `set_link` writes all three columns; for
/// a connection the path goes into `shared_connection_id` as
/// `<repoId>:<path>`, or NULL).
#[derive(Clone, PartialEq, Eq)]
pub struct LinkUpdate {
    pub kind: Kind,
    pub id: String,
    pub link: Link,
    /// The file write this link waits for: store it only once that write
    /// succeeded (a base is never recorded for a failed write).
    pub pending_on: Option<String>,
}

impl fmt::Debug for LinkUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LinkUpdate")
            .field("kind", &self.kind)
            .field("id", &self.id)
            .field("link", &self.link)
            .field("pending", &self.pending_on.is_some())
            .finish()
    }
}

/// A file operation, repo-relative.
#[derive(Clone, PartialEq, Eq)]
pub enum FileOp {
    /// `expect_hash` (M1): the [`file_hash`] the file at `rel_path` has now
    /// as far as the plan knows (the scan's, or the row's base). `None`
    /// means no file may be there. When the disk differs (another hash, a
    /// file where none was expected, or none where one was), the write is
    /// stale: Core doesn't write, it syncs that pair instead, and the file
    /// wins.
    Write {
        rel_path: String,
        text: String,
        expect_hash: Option<String>,
    },
    /// `expect_hash`, as a `Write`'s: the file Core may delete (`None`:
    /// only when there is none). A stale delete deletes nothing.
    Delete {
        rel_path: String,
        expect_hash: Option<String>,
    },
}

impl fmt::Debug for FileOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FileOp::Write { text, .. } => write!(f, "Write({} bytes)", text.len()),
            FileOp::Delete { .. } => f.write_str("Delete"),
        }
    }
}

fn js_number_opt<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(n) if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => {
            s.serialize_i64(*n as i64)
        }
        Some(n) => s.serialize_f64(*n),
        None => s.serialize_none(),
    }
}

fn js_number_opt_opt<S: Serializer>(v: &Option<Option<f64>>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(inner) => js_number_opt(inner, s),
        None => s.serialize_none(),
    }
}

/// The local values a template overwrote, only those that differed.
/// Never a user name or a secret: the type has neither.
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ReplacedValues {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_number_opt"
    )]
    pub port: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssl_mode: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ssh_host: Option<Option<String>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "js_number_opt_opt"
    )]
    pub ssh_port: Option<Option<f64>>,
}

impl fmt::Debug for ReplacedValues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let fields: Vec<&str> = [
            ("name", self.name.is_some()),
            ("host", self.host.is_some()),
            ("port", self.port.is_some()),
            ("databaseName", self.database_name.is_some()),
            ("sslMode", self.ssl_mode.is_some()),
            ("sshHost", self.ssh_host.is_some()),
            ("sshPort", self.ssh_port.is_some()),
        ]
        .into_iter()
        .filter_map(|(n, on)| on.then_some(n))
        .collect();
        f.debug_struct("ReplacedValues")
            .field("fields", &fields)
            .finish()
    }
}

/// What a sync tells the user. Paths are relative to `.seaquel/`.
#[derive(Clone, PartialEq, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SyncNotice {
    /// Changed here and in the repo: the repo's version is shown, the
    /// previous content is a version; `replaced` for connections.
    Conflict {
        kind: Kind,
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        replaced: Option<ReplacedValues>,
    },
    /// A teammate removed the file: the row stays, unshared (a connection:
    /// unlinked and local-only).
    RemovedInRepo {
        kind: Kind,
        id: String,
    },
    /// A file that claims no row, whose name a row that isn't shared has.
    NameTaken {
        path: String,
        taken_by: String,
    },
    /// A file that claims a row another file has (M1); not imported.
    Unpaired {
        path: String,
        claims: String,
    },
    Skipped {
        path: String,
        why: SkipReason,
    },
    /// The template names another database type: the connection is
    /// unlinked and kept, and `imported` is the connection this sync made
    /// from the template.
    TemplateTypeChanged {
        kind: Kind,
        id: String,
        path: String,
        template_type: String,
        imported: String,
    },
}

impl fmt::Debug for SyncNotice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncNotice::Conflict { kind, id, replaced } => f
                .debug_struct("Conflict")
                .field("kind", kind)
                .field("id", id)
                .field("replaced", replaced)
                .finish(),
            SyncNotice::RemovedInRepo { kind, id } => f
                .debug_struct("RemovedInRepo")
                .field("kind", kind)
                .field("id", id)
                .finish(),
            SyncNotice::NameTaken { taken_by, .. } => f
                .debug_struct("NameTaken")
                .field("taken_by", taken_by)
                .finish_non_exhaustive(),
            SyncNotice::Unpaired { claims, .. } => f
                .debug_struct("Unpaired")
                .field("claims", claims)
                .finish_non_exhaustive(),
            SyncNotice::Skipped { why, .. } => f
                .debug_struct("Skipped")
                .field("why", why)
                .finish_non_exhaustive(),
            SyncNotice::TemplateTypeChanged {
                kind, id, imported, ..
            } => f
                .debug_struct("TemplateTypeChanged")
                .field("kind", kind)
                .field("id", id)
                .field("imported", imported)
                .finish_non_exhaustive(),
        }
    }
}

impl SyncNotice {
    /// The file a notice names, if it names one.
    pub fn path(&self) -> Option<&str> {
        match self {
            SyncNotice::NameTaken { path, .. }
            | SyncNotice::Unpaired { path, .. }
            | SyncNotice::Skipped { path, .. }
            | SyncNotice::TemplateTypeChanged { path, .. } => Some(path),
            SyncNotice::Conflict { .. } | SyncNotice::RemovedInRepo { .. } => None,
        }
    }
}

/// "Each file is named at most once per session" (`*` (6)): Core keeps one
/// per workspace and passes every sync's notices through it.
#[derive(Default)]
pub struct NoticeMemory(HashSet<String>);

impl fmt::Debug for NoticeMemory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NoticeMemory")
            .field("named", &self.0.len())
            .finish()
    }
}

impl NoticeMemory {
    pub fn filter(&mut self, notices: Vec<SyncNotice>) -> Vec<SyncNotice> {
        notices
            .into_iter()
            .filter(|n| n.path().is_none_or(|p| self.0.insert(path_key(p))))
            .collect()
    }
}

/// What a sync does.
#[derive(Clone, Debug, Default)]
pub struct SyncPlan {
    pub conflicted: bool,
    pub rows: Vec<RowOp>,
    pub links: Vec<LinkUpdate>,
    pub files: Vec<FileOp>,
    pub notices: Vec<SyncNotice>,
}

/// A row a library call changed, read again after its commit,
/// with the link it had. `row: None` is a removal.
pub enum RowChange<'a> {
    Query {
        row: Option<&'a PersistedSavedQuery>,
        link: &'a Link,
        /// The name (by `name_key`) or folder changed: the file moves.
        renamed: bool,
    },
    Dashboard {
        row: Option<&'a PersistedDashboard>,
        link: &'a Link,
        renamed: bool,
    },
    Connection {
        row: Option<&'a PersistedConnection>,
        link: &'a Link,
        renamed: bool,
        /// The user shared it just now (the local-only toggle, or ticked at the first link):
        /// write and link its template. Otherwise a
        /// connection without a link publishes nothing.
        shared_now: bool,
    },
    /// A linked project's name: `project.yaml` follows, the
    /// directory never moves.
    Project { name: &'a str },
}

impl fmt::Debug for RowChange<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RowChange::Query { row, link, renamed } => f
                .debug_struct("Query")
                .field("id", &row.map(|r| &r.id))
                .field("link", link)
                .field("renamed", renamed)
                .finish(),
            RowChange::Dashboard { row, link, renamed } => f
                .debug_struct("Dashboard")
                .field("id", &row.map(|r| &r.id))
                .field("link", link)
                .field("renamed", renamed)
                .finish(),
            RowChange::Connection {
                row,
                link,
                renamed,
                shared_now,
            } => f
                .debug_struct("Connection")
                .field("id", &row.map(|r| &r.id))
                .field("link", link)
                .field("renamed", renamed)
                .field("shared_now", shared_now)
                .finish(),
            RowChange::Project { .. } => f.debug_struct("Project").finish_non_exhaustive(),
        }
    }
}

/// What publishing needs from the disk: which paths are taken (compare by
/// [`path_key`]) and the text of the row's file now (at its stored path;
/// `project.yaml` for a project).
pub struct PublishContext<'a> {
    pub taken: &'a dyn Fn(&str) -> bool,
    pub existing: Option<&'a str>,
}

/// One row's file operations.
#[derive(Clone, Debug, Default)]
pub struct PublishPlan {
    pub files: Vec<FileOp>,
    pub on_success: Option<LinkUpdate>,
    /// The link to store when a write fails (a first share keeps its path
    /// and no base, so the next sync writes the file); `None` keeps the
    /// link as it was.
    pub on_failure: Option<LinkUpdate>,
}

/// Where Core records the result of a publish in the answer's `Seqd`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PublishStatus {
    Written,
    Deleted,
    Failed,
}

/// `projection` on a library answer.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishOutcome {
    pub status: PublishStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// The GUI-facing message; may name a path relative to `.seaquel/`,
    /// never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl fmt::Debug for PublishOutcome {
    // The message may name a path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PublishOutcome")
            .field("status", &self.status)
            .field("code", &self.code)
            .field("message", &self.message.is_some())
            .finish()
    }
}

// ── Rows to files and back ──

fn json_list<T: for<'de> Deserialize<'de>>(raw: Option<&RawValue>) -> Vec<T> {
    raw.and_then(|r| serde_json::from_str::<Option<Vec<T>>>(r.get()).ok())
        .flatten()
        .unwrap_or_default()
}

fn query_file(row: &PersistedSavedQuery) -> QueryFile {
    QueryFile {
        name: row.name.clone(),
        description: row.description.clone(),
        database: row.database_type.clone(),
        tags: json_list(row.tags.as_deref()),
        parameters: json_list::<PersistedQueryParameter>(row.parameters.as_deref()),
        query: row.query.clone(),
        folder: row.folder.clone().unwrap_or_default(),
        file_id: None,
    }
}

fn dashboard_file(row: &PersistedDashboard) -> DashboardFile {
    DashboardFile {
        name: row.name.clone(),
        description: row.description.clone(),
        widgets: row.widgets.clone(),
        viewport: Some(row.viewport.clone()),
        date_filter: row.date_filter.clone().filter(|f| f != "null"),
        file_id: None,
    }
}

fn ssh_of(row: &PersistedConnection) -> Option<SshTunnelConfig> {
    row.ssh_tunnel
        .as_deref()
        .and_then(|r| serde_json::from_str::<SshTunnelConfig>(r.get()).ok())
}

fn template_file(row: &PersistedConnection) -> TemplateFile {
    TemplateFile {
        name: row.name.clone(),
        ty: row.ty.clone(),
        host: row.host.clone(),
        port: row.port,
        database_name: row.database_name.clone(),
        ssl_mode: row.ssl_mode.clone().filter(|s| !s.is_empty()),
        ssh_tunnel: ssh_of(row).filter(|s| s.enabled).map(|s| TemplateSsh {
            host: Some(s.host),
            port: Some(s.port),
        }),
        labels: Vec::new(),
        file_id: None,
    }
}

const HASH_QUERY_DIR: &str = ".seaquel/projects/x/queries";

fn query_hash(q: &QueryFile) -> String {
    // Core's own text: its escapes are undone whatever the id.
    let again = parse_query_core(
        &query_content(q),
        &format!("{HASH_QUERY_DIR}/x.sql"),
        HASH_QUERY_DIR,
    );
    content_hash(&query_content(&again))
}

fn dashboard_hash(d: &DashboardFile) -> String {
    let text = dashboard_content(d);
    let again = parse_dashboard(&text, "x.json").unwrap_or_else(|| d.clone());
    content_hash(&dashboard_content(&again))
}

fn template_hash(t: &TemplateFile) -> String {
    let again = parse_template_core(&template_content(t)).unwrap_or_else(|| t.clone());
    content_hash(&template_content(&again))
}

/// A row's hash, `write(parse(write(row)))`, or `None` for a
/// removal or a project.
pub fn row_hash(change: &RowChange<'_>) -> Option<String> {
    match change {
        RowChange::Query { row, .. } => row.map(|r| query_hash(&query_file(r))),
        RowChange::Dashboard { row, .. } => row.map(|r| dashboard_hash(&dashboard_file(r))),
        RowChange::Connection { row, .. } => row.map(|r| template_hash(&template_file(r))),
        RowChange::Project { .. } => None,
    }
}

fn notice_path(rel: &str) -> String {
    rel.strip_prefix(super::SEAQUEL_DIR)
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(rel)
        .to_string()
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn raw_or(text: &str, fallback: &str) -> Box<RawValue> {
    RawValue::from_string(text.to_string())
        .or_else(|_| RawValue::from_string(fallback.to_string()))
        .unwrap_or_else(|_| RawValue::from_string("null".into()).expect("null is JSON"))
}

fn non_empty(s: &Option<String>) -> Option<String> {
    s.clone().filter(|s| !s.is_empty())
}

// ── Parsed files and the rows that may pair with them ──

/// What the library's checks run under (C1): Core passes the limits it was
/// built with, so a sync never plans a row write the library would refuse.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub library: LibraryLimits,
    pub state: StateLimits,
}

enum Content {
    Query(QueryFile),
    Dashboard(DashboardFile),
    Template(TemplateFile),
}

/// Every row holding each name, in the plan's final state. Rows that
/// already share a name all hold it; a notice names the first by id.
type Held = HashMap<NameKey, BTreeSet<String>>;

fn hold(held: &mut Held, key: NameKey, id: &str) {
    held.entry(key).or_default().insert(id.to_string());
}

/// The first holder of `key` other than `me`, if any.
fn holder(held: &Held, key: &NameKey, me: &str) -> Option<String> {
    held.get(key)?.iter().find(|h| *h != me).cloned()
}

fn content_name(c: &Content) -> &str {
    match c {
        Content::Query(q) => &q.name,
        Content::Dashboard(d) => &d.name,
        Content::Template(t) => &t.name,
    }
}

/// A shared file's content, by where it is.
fn parse_content(rel_path: &str, text: &str) -> Option<Content> {
    let lower = rel_path.to_ascii_lowercase();
    if lower.ends_with(".sql") {
        let at = rel_path
            .rfind("/queries/")
            .or_else(|| rel_path.find("/queries"))?;
        let dir = &rel_path[..at + "/queries".len()];
        return Some(Content::Query(parse_query(text, rel_path, dir)));
    }
    if lower.ends_with(".json") && rel_path.contains("/dashboards/") {
        return parse_dashboard(text, rel_path).map(Content::Dashboard);
    }
    if (lower.ends_with(".yaml") || lower.ends_with(".yml")) && rel_path.contains("/connections/") {
        return parse_template(text).map(Content::Template);
    }
    None
}

fn file_content_hash(c: &Content) -> String {
    match c {
        Content::Query(q) => content_hash(&query_content(q)),
        Content::Dashboard(d) => content_hash(&dashboard_content(d)),
        Content::Template(t) => content_hash(&template_content(t)),
    }
}

/// A shared file's hash ([`content_hash`] of
/// `write(parse(text))`) and its stable id, by its repo-relative path:
/// a query, dashboard or template, or a project's `project.yaml` (name and
/// description; no id). `None` for a file that doesn't parse or isn't one
/// of these. The planner's hashes are this function's, so Core uses it to
/// compare a file on disk with a `Write`'s or `Delete`'s `expect_hash`.
pub fn file_hash(rel_path: &str, text: &str) -> Option<(String, Option<String>)> {
    if let Some(dir_path) = rel_path.strip_suffix("/project.yaml") {
        let dir = dir_path.rsplit('/').next().unwrap_or(dir_path);
        let p = parse_project(text, dir);
        return Some((content_hash(&write_project(&p)), None));
    }
    let c = parse_content(rel_path, text)?;
    let id = match &c {
        Content::Query(q) => q.file_id.clone(),
        Content::Dashboard(d) => d.file_id.clone(),
        Content::Template(t) => t.file_id.clone(),
    };
    Some((file_content_hash(&c), id))
}

/// The hash of a file's content under another name (F′, R2).
fn hash_named(c: &Content, name: &str) -> String {
    match c {
        Content::Query(q) => {
            let mut q = q.clone();
            q.name = name.to_string();
            query_hash(&q)
        }
        Content::Dashboard(d) => {
            let mut d = d.clone();
            d.name = name.to_string();
            dashboard_hash(&d)
        }
        Content::Template(t) => {
            let mut t = t.clone();
            t.name = name.to_string();
            template_hash(&t)
        }
    }
}

/// A name as the library's duplicate check groups it: `name_key`, and for
/// a saved query its folder (`folder_key`); `""` for the other kinds.
type NameKey = (String, String);

struct PFile {
    path: String,
    path_key: String,
    folder: String,
    key: NameKey,
    file_id: Option<String>,
    hash: String,
    content: Content,
}

struct PRow {
    id: String,
    name: String,
    /// The name as pairing compares it: the folder its stored file is in.
    key: NameKey,
    /// The name as the library holds it: the row's own folder column.
    held: NameKey,
    link: Link,
    stored_key: Option<String>,
    slug_keys: Vec<String>,
    hash: String,
    /// Index in the kind's `SharedRows` list.
    at: usize,
}

struct Sync<'a> {
    link: &'a ProjectLink,
    rows: &'a SharedRows,
    limits: &'a Limits,
    ids: &'a mut dyn IdSource,
    /// `path_key`s of every skipped path: the scan's, and files that don't
    /// parse or that the library would refuse.
    skipped: HashSet<String>,
    /// The scan's files, for a free path.
    taken: TakenPaths,
    plan: SyncPlan,
}

fn ext_of(kind: Kind, path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    match kind {
        Kind::SavedQuery => lower.ends_with(".sql"),
        Kind::Dashboard => lower.ends_with(".json"),
        Kind::Connection => lower.ends_with(".yaml") || lower.ends_with(".yml"),
    }
}

fn ext_for(kind: Kind) -> &'static str {
    match kind {
        Kind::SavedQuery => ".sql",
        Kind::Dashboard => ".json",
        Kind::Connection => ".yaml",
    }
}

const KINDS: [Kind; 3] = [Kind::SavedQuery, Kind::Dashboard, Kind::Connection];

fn query_draft(q: &QueryFile, project_id: &str) -> SavedQueryDraft {
    SavedQueryDraft {
        project_id: project_id.to_string(),
        name: q.name.clone(),
        query: q.query.clone(),
        parameters: (!q.parameters.is_empty()).then(|| q.parameters.clone()),
        description: non_empty(&q.description),
        database_type: non_empty(&q.database),
        tags: Some(q.tags.clone()),
        folder: Some(q.folder.clone()).filter(|x| !x.is_empty()),
        starred: false,
        shared: true,
    }
}

fn dashboard_draft(d: &DashboardFile, project_id: &str) -> DashboardDraft {
    DashboardDraft {
        project_id: project_id.to_string(),
        name: d.name.clone(),
        description: non_empty(&d.description),
        widgets: raw_or(&d.widgets, "[]"),
        viewport: raw_or(
            d.viewport.as_deref().unwrap_or(DEFAULT_VIEWPORT),
            DEFAULT_VIEWPORT,
        ),
        date_filter: d.date_filter.as_deref().map(|t| raw_or(t, "null")),
        shared: true,
        rename_if_taken: false,
    }
}

fn connection_draft(t: &TemplateFile, project_id: &str, link_id: String) -> ConnectionDraft {
    ConnectionDraft {
        project_id: project_id.to_string(),
        name: t.name.clone(),
        ty: t.ty.clone(),
        host: t.host.clone(),
        port: t.port,
        database_name: t.database_name.clone(),
        username: String::new(),
        ssl_mode: t.ssl_mode.clone(),
        connection_string: None,
        ssh_tunnel: t.ssh_tunnel.as_ref().map(|s| SshTunnelConfig {
            enabled: true,
            host: s.host.clone().unwrap_or_default(),
            port: s.port.unwrap_or(22.0),
            username: String::new(),
            auth_method: "key".into(),
            key_path: None,
        }),
        save_password: false,
        save_ssh_password: false,
        save_ssh_key_passphrase: false,
        label_ids: Vec::new(),
        is_local_only: Some(false),
        shared_connection_id: Some(link_id),
        ai_share_schema: None,
        ai_share_data: None,
        active_ai_provider_id: None,
        active_ai_model: None,
        connected: false,
        rename_if_taken: true,
    }
}

/// The repo a `shared_connection_id` names (`<repoId>:<path>`).
fn template_repo(shared_connection_id: &str) -> Option<&str> {
    let path = template_path(shared_connection_id)?;
    shared_connection_id
        .len()
        .checked_sub(path.len() + 1)
        .map(|end| &shared_connection_id[..end])
}

impl Sync<'_> {
    /// `path` (a `path_key`) is a skipped path or lies under one.
    fn under_skipped(&self, key: &str) -> bool {
        if self.skipped.is_empty() {
            return false;
        }
        if self.skipped.contains(key) {
            return true;
        }
        key.match_indices('/')
            .any(|(i, _)| self.skipped.contains(&key[..i]))
    }

    fn skip(&mut self, path: &str, why: SkipReason) {
        self.skipped.insert(path_key(path));
        self.plan.notices.push(SyncNotice::Skipped {
            path: notice_path(path),
            why,
        });
    }

    /// Reads one file of `kind`, or skips it: `doesNotParse`, or `invalid`
    /// when the row it would make is one the library refuses (C1).
    fn parse(&mut self, kind: Kind, f: &RawFile) -> Option<PFile> {
        let dir = self.link.kind_dir(kind);
        let pid = &self.rows.project_id;
        let lib = &self.limits.library;
        let (name, folder, file_id, hash, checked, content) = match kind {
            Kind::SavedQuery => {
                let q = parse_query(&f.text, &f.rel_path, &dir);
                if js_trim(&q.name).is_empty() {
                    self.skip(&f.rel_path, SkipReason::DoesNotParse);
                    return None;
                }
                let checked = check_saved_query_draft(&query_draft(&q, pid), lib);
                (
                    q.name.clone(),
                    q.folder.clone(),
                    q.file_id.clone(),
                    content_hash(&query_content(&q)),
                    checked,
                    Content::Query(q),
                )
            }
            Kind::Dashboard => match parse_dashboard(&f.text, &f.rel_path) {
                Some(d) if !js_trim(&d.name).is_empty() => {
                    let checked =
                        check_dashboard_draft(&dashboard_draft(&d, pid), lib, &self.limits.state);
                    (
                        d.name.clone(),
                        String::new(),
                        d.file_id.clone(),
                        content_hash(&dashboard_content(&d)),
                        checked,
                        Content::Dashboard(d),
                    )
                }
                _ => {
                    self.skip(&f.rel_path, SkipReason::DoesNotParse);
                    return None;
                }
            },
            Kind::Connection => match parse_template(&f.text) {
                Some(t) => {
                    let draft = connection_draft(&t, pid, self.link.template_link(&f.rel_path));
                    let checked = check_connection_draft(&draft, lib);
                    (
                        t.name.clone(),
                        String::new(),
                        t.file_id.clone(),
                        content_hash(&template_content(&t)),
                        checked,
                        Content::Template(t),
                    )
                }
                None => {
                    self.skip(&f.rel_path, SkipReason::DoesNotParse);
                    return None;
                }
            },
        };
        if checked.is_err() {
            self.skip(&f.rel_path, SkipReason::Invalid);
            return None;
        }
        let key = (
            name_key(&name),
            if kind == Kind::SavedQuery {
                folder_key(Some(&folder)).to_string()
            } else {
                String::new()
            },
        );
        Some(PFile {
            path_key: path_key(&f.rel_path),
            path: f.rel_path.clone(),
            folder,
            key,
            file_id,
            hash,
            content,
        })
    }

    /// The paths a row's file would have by name: the current stem rule's and today's,
    /// as `path_key`s.
    fn slug_keys(&self, kind: Kind, name: &str, folder: &str) -> Vec<String> {
        let mut dir = self.link.kind_dir(kind);
        if kind == Kind::SavedQuery && !folder.is_empty() {
            dir = format!("{dir}/{folder}");
        }
        let ext = ext_for(kind);
        let mut out = vec![path_key(&format!("{dir}/{}{ext}", file_stem(name)))];
        let legacy = path_key(&format!("{dir}/{}{ext}", legacy_stem(name)));
        if !out.contains(&legacy) {
            out.push(legacy);
        }
        out
    }

    /// The rows that pair (shared queries and dashboards, connections
    /// linked to this repo), and every row's name as the library holds it
    /// (the name registry the sync keeps in its final state).
    fn rows_of(&self, kind: Kind) -> (Vec<PRow>, Held) {
        let mut rows = Vec::new();
        let mut held = HashMap::new();
        let base = self.link.kind_dir(kind);
        let add = |me: &Self,
                   rows: &mut Vec<PRow>,
                   at: usize,
                   id: &str,
                   name: &str,
                   own_folder: &str,
                   link: &Link,
                   hash: String| {
            let folder = match (&link.path, kind) {
                (Some(p), Kind::SavedQuery) => dir_of(p)
                    .strip_prefix(&base)
                    .map(|r| r.trim_start_matches('/').to_string())
                    .unwrap_or_else(|| own_folder.to_string()),
                (_, Kind::SavedQuery) => own_folder.to_string(),
                _ => String::new(),
            };
            let nk = name_key(name);
            rows.push(PRow {
                id: id.to_string(),
                name: name.to_string(),
                key: (nk.clone(), folder_key(Some(&folder)).to_string()),
                held: (nk, folder_key(Some(own_folder)).to_string()),
                stored_key: link.path.as_deref().map(path_key),
                slug_keys: me.slug_keys(kind, name, &folder),
                link: link.clone(),
                hash,
                at,
            });
        };
        match kind {
            Kind::SavedQuery => {
                for (at, q) in self.rows.queries.iter().enumerate() {
                    let own = q.row.folder.clone().unwrap_or_default();
                    hold(
                        &mut held,
                        (name_key(&q.row.name), folder_key(Some(&own)).to_string()),
                        &q.row.id,
                    );
                    if q.row.shared {
                        let hash = query_hash(&query_file(&q.row));
                        add(
                            self,
                            &mut rows,
                            at,
                            &q.row.id,
                            &q.row.name,
                            &own,
                            &q.link,
                            hash,
                        );
                    }
                }
            }
            Kind::Dashboard => {
                for (at, d) in self.rows.dashboards.iter().enumerate() {
                    hold(&mut held, (name_key(&d.row.name), String::new()), &d.row.id);
                    if d.row.shared {
                        let hash = dashboard_hash(&dashboard_file(&d.row));
                        add(
                            self,
                            &mut rows,
                            at,
                            &d.row.id,
                            &d.row.name,
                            "",
                            &d.link,
                            hash,
                        );
                    }
                }
            }
            Kind::Connection => {
                for (at, c) in self.rows.connections.iter().enumerate() {
                    hold(&mut held, (name_key(&c.row.name), String::new()), &c.row.id);
                    // M6: a link to another repo's template, or to another
                    // directory's (bug 7's damage), isn't this directory's
                    // to pair or remove.
                    let other_repo = c
                        .row
                        .shared_connection_id
                        .as_deref()
                        .and_then(template_repo)
                        .is_some_and(|r| r != self.link.repo_id);
                    let other_dir = c.link.path.as_deref().is_some_and(|p| dir_of(p) != base);
                    if c.link.path.is_some() && !other_repo && !other_dir {
                        let hash = template_hash(&template_file(&c.row));
                        add(
                            self,
                            &mut rows,
                            at,
                            &c.row.id,
                            &c.row.name,
                            "",
                            &c.link,
                            hash,
                        );
                    }
                }
            }
        }
        (rows, held)
    }

    fn run(&mut self, kind: Kind, scan: &DirScan) {
        let dir = self.link.kind_dir(kind);
        let dir_prefix = format!("{dir}/");
        let mut files = Vec::new();
        for f in &scan.files {
            let in_dir = match kind {
                Kind::Connection => dir_of(&f.rel_path) == dir,
                _ => f.rel_path.starts_with(&dir_prefix),
            };
            if !in_dir || !ext_of(kind, &f.rel_path) {
                continue;
            }
            if self.under_skipped(&path_key(&f.rel_path)) {
                continue;
            }
            if let Some(p) = self.parse(kind, f) {
                files.push(p);
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));

        let (rows, mut held) = self.rows_of(kind);
        let dir_key = path_key(&dir);
        let kind_skipped = self.skipped.iter().any(|s| {
            s == &dir_key
                || s.starts_with(&format!("{dir_key}/"))
                || dir_key.starts_with(&format!("{s}/"))
        });
        // I5: a row whose stored or slug path was skipped stays out of
        // pairing; it can't be removed either.
        let excluded: Vec<bool> = rows
            .iter()
            .map(|r| {
                r.stored_key.iter().any(|k| self.under_skipped(k))
                    || r.slug_keys.iter().any(|k| self.under_skipped(k))
            })
            .collect();

        // Indexes over the files (M4).
        let mut id_count: HashMap<&str, usize> = HashMap::new();
        for f in &files {
            if let Some(id) = &f.file_id {
                *id_count.entry(id.as_str()).or_default() += 1;
            }
        }
        let mut by_id: HashMap<&str, usize> = HashMap::new();
        let mut by_path: HashMap<&str, usize> = HashMap::new();
        let mut by_name: HashMap<&NameKey, Vec<usize>> = HashMap::new();
        let mut by_slug: HashMap<&str, usize> = HashMap::new();
        for (fi, f) in files.iter().enumerate() {
            if let Some(id) = f.file_id.as_deref().filter(|id| id_count[id] == 1) {
                by_id.insert(id, fi);
            }
            by_path.entry(f.path.as_str()).or_insert(fi);
            by_name.entry(&f.key).or_default().push(fi);
            by_slug.entry(f.path_key.as_str()).or_insert(fi);
        }

        // Pairing: id, stored path, name (a file at a slug path wins a
        // tie), slug path.
        let mut file_row: Vec<Option<usize>> = vec![None; files.len()];
        let mut row_file: Vec<Option<usize>> = vec![None; rows.len()];
        let mut claimed: Vec<Option<String>> = vec![None; files.len()];
        for (ri, r) in rows.iter().enumerate() {
            if excluded[ri] {
                continue;
            }
            if let Some(&fi) = r.link.file_id.as_deref().and_then(|id| by_id.get(id)) {
                if file_row[fi].is_none() {
                    file_row[fi] = Some(ri);
                    row_file[ri] = Some(fi);
                }
            }
        }
        for (ri, r) in rows.iter().enumerate() {
            if excluded[ri] || row_file[ri].is_some() {
                continue;
            }
            if let Some(&fi) = r.link.path.as_deref().and_then(|p| by_path.get(p)) {
                if file_row[fi].is_none() {
                    file_row[fi] = Some(ri);
                    row_file[ri] = Some(fi);
                }
            }
        }
        for (ri, r) in rows.iter().enumerate() {
            if excluded[ri] || row_file[ri].is_some() {
                continue;
            }
            let free = |fi: &usize| file_row[*fi].is_none() && claimed[*fi].is_none();
            let by_name_free: Vec<usize> = by_name
                .get(&r.key)
                .map(|v| v.iter().copied().filter(free).collect())
                .unwrap_or_default();
            let at_slug = |fi: &usize| r.slug_keys.contains(&files[*fi].path_key);
            let winner = if by_name_free.is_empty() {
                r.slug_keys
                    .iter()
                    .filter_map(|k| by_slug.get(k.as_str()).copied())
                    .find(free)
            } else {
                by_name_free
                    .iter()
                    .copied()
                    .find(at_slug)
                    .or(by_name_free.first().copied())
            };
            let Some(w) = winner else { continue };
            file_row[w] = Some(ri);
            row_file[ri] = Some(w);
            for fi in by_name_free.into_iter().filter(|fi| *fi != w) {
                claimed[fi] = Some(r.id.clone());
            }
        }
        // A free file naming a row another file has (or one left out of
        // pairing) claims it.
        let mut has_file: HashMap<&NameKey, &str> = HashMap::new();
        for (ri, r) in rows.iter().enumerate() {
            if row_file[ri].is_some() || excluded[ri] {
                has_file.entry(&r.key).or_insert(&r.id);
            }
        }
        for (fi, f) in files.iter().enumerate() {
            if file_row[fi].is_none() && claimed[fi].is_none() {
                if let Some(id) = has_file.get(&f.key) {
                    claimed[fi] = Some(id.to_string());
                }
            }
        }

        // The rule, each pair, in row order.
        let mut created: HashSet<String> = HashSet::new();
        for (ri, r) in rows.iter().enumerate() {
            if let Some(fi) = row_file[ri] {
                self.paired(kind, r, &files[fi], &mut held, &mut created);
            }
        }
        // Files no row has.
        for (fi, f) in files.iter().enumerate() {
            if file_row[fi].is_some() {
                continue;
            }
            if let Some(claims) = &claimed[fi] {
                self.plan.notices.push(SyncNotice::Unpaired {
                    path: notice_path(&f.path),
                    claims: claims.clone(),
                });
                continue;
            }
            if kind != Kind::Connection {
                if let Some(holder) = holder(&held, &f.key, "") {
                    let notice = if created.contains(&holder) {
                        SyncNotice::Unpaired {
                            path: notice_path(&f.path),
                            claims: holder,
                        }
                    } else {
                        SyncNotice::NameTaken {
                            path: notice_path(&f.path),
                            taken_by: holder,
                        }
                    };
                    self.plan.notices.push(notice);
                    continue;
                }
            }
            let keep_id = f
                .file_id
                .as_deref()
                .is_some_and(|id| id_count.get(id) == Some(&1));
            self.create(kind, f, keep_id, true, &mut held, &mut created);
        }
        // Rows no file has.
        for (ri, r) in rows.iter().enumerate() {
            if row_file[ri].is_some() || excluded[ri] {
                continue;
            }
            // An older release's row (no stored path) may be any file of
            // its kind: while one was skipped, none reads as missing.
            if r.link.path.is_none() && kind_skipped {
                continue;
            }
            match (&r.link.path, &r.link.base) {
                (Some(path), None) => self.write_unwritten(kind, r, path),
                _ => {
                    self.plan.rows.push(RowOp::Unshare {
                        kind,
                        id: r.id.clone(),
                    });
                    self.set_link(kind, &r.id, Link::default(), None);
                    self.plan.notices.push(SyncNotice::RemovedInRepo {
                        kind,
                        id: r.id.clone(),
                    });
                }
            }
        }
    }

    fn set_link(&mut self, kind: Kind, id: &str, link: Link, pending_on: Option<String>) {
        self.plan.links.push(LinkUpdate {
            kind,
            id: id.to_string(),
            link,
            pending_on,
        });
    }

    fn paired(
        &mut self,
        kind: Kind,
        r: &PRow,
        f: &PFile,
        held: &mut Held,
        created: &mut HashSet<String>,
    ) {
        if let Content::Template(t) = &f.content {
            let row = &self.rows.connections[r.at].row;
            if t.ty != row.ty {
                // A changed type: unlink and keep the connection; the template is
                // imported as a new one, which this notice announces.
                self.plan.rows.push(RowOp::Unshare {
                    kind,
                    id: r.id.clone(),
                });
                self.set_link(kind, &r.id, Link::default(), None);
                if let Some(imported) = self.create(kind, f, true, false, held, created) {
                    self.plan.notices.push(SyncNotice::TemplateTypeChanged {
                        kind,
                        id: r.id.clone(),
                        path: notice_path(&f.path),
                        template_type: t.ty.clone(),
                        imported,
                    });
                }
                return;
            }
        }
        let file_link = Link {
            path: Some(f.path.clone()),
            base: Some(f.hash.clone()),
            file_id: f.file_id.clone(),
        };
        let base = r.link.base.as_deref();
        // F′: the file under the row's name. A base equal to it means the
        // last sync withheld the file's name from the row (flag 4).
        let withheld = name_key(content_name(&f.content)) != name_key(&r.name)
            && base == Some(hash_named(&f.content, &r.name).as_str());
        if r.hash == f.hash {
            if kind == Kind::SavedQuery && r.held.1 != f.key.1 {
                // The folder follows the file (when the library can hold
                // the name there).
                let to = (r.held.0.clone(), f.key.1.clone());
                if self.free_for(held, &to, &r.id) {
                    if !self.push_checked(
                        f,
                        RowOp::UpdateQuery {
                            id: r.id.clone(),
                            patch: SavedQueryPatch {
                                folder: Some(Some(f.folder.clone()).filter(|x| !x.is_empty())),
                                ..Default::default()
                            },
                        },
                    ) {
                        return;
                    }
                    Self::move_name(held, &r.held, to, &r.id);
                }
            }
            if r.link != file_link {
                self.set_link(kind, &r.id, file_link, None);
            }
        } else if base == Some(f.hash.as_str()) || (withheld && base != Some(r.hash.as_str())) {
            // A local change: write the file, unless it changed since the
            // scan read it (M1). While the row's name is withheld (R2: the
            // base is F′, the file under the row's name), the change goes
            // out under the file's name, so the teammate's name stays in
            // the repo, and the base becomes R again.
            let file_id = r
                .link
                .file_id
                .clone()
                .or_else(|| f.file_id.clone())
                .unwrap_or_else(|| self.ids.file_id());
            let as_named = withheld.then(|| content_name(&f.content));
            let text = self.text_for(kind, r, Some(file_id.clone()), Some(f), as_named);
            self.plan.files.push(FileOp::Write {
                rel_path: f.path.clone(),
                text,
                expect_hash: Some(f.hash.clone()),
            });
            self.set_link(
                kind,
                &r.id,
                Link {
                    path: Some(f.path.clone()),
                    base: Some(r.hash.clone()),
                    file_id: Some(file_id),
                },
                Some(f.path.clone()),
            );
        } else {
            // `R = B, F ≠ B` takes the file; anything else with `R ≠ F` is
            // a conflict, the file winning.
            let conflict = base != Some(r.hash.as_str());
            self.take_file(kind, r, f, conflict, held);
        }
    }

    /// Nobody but `me` holds `key` in the plan's final state.
    fn free_for(&self, held: &Held, key: &NameKey, me: &str) -> bool {
        holder(held, key, me).is_none()
    }

    fn move_name(held: &mut Held, from: &NameKey, to: NameKey, me: &str) {
        if let Some(set) = held.get_mut(from) {
            set.remove(me);
            if set.is_empty() {
                held.remove(from);
            }
        }
        hold(held, to, me);
    }

    /// Pushes `op` unless the library's checks refuse it (C1); then the
    /// file is skipped as `invalid` and nothing changes for its row.
    fn push_checked(&mut self, f: &PFile, op: RowOp) -> bool {
        let lib = &self.limits.library;
        let checked = match &op {
            RowOp::CreateQuery { draft, .. } => check_saved_query_draft(draft, lib),
            RowOp::UpdateQuery { patch, .. } => check_saved_query_patch(patch, lib),
            RowOp::CreateDashboard { draft, .. } => {
                check_dashboard_draft(draft, lib, &self.limits.state)
            }
            RowOp::UpdateDashboard { patch, .. } => {
                check_dashboard_patch(patch, lib, &self.limits.state)
            }
            RowOp::CreateConnection { draft, .. } => check_connection_draft(draft, lib),
            RowOp::UpdateConnection { patch, .. } => check_connection_patch(patch, lib),
            RowOp::Unshare { .. } => Ok(()),
        };
        if checked.is_err() {
            self.skip(&f.path, SkipReason::Invalid);
            return false;
        }
        self.plan.rows.push(op);
        true
    }

    /// The row takes the file's content. A name another row
    /// holds is withheld (flag 4): the row keeps its name, takes the rest,
    /// the notice names the file, and the base is the row's own hash after
    /// the partial patch, so every later sync tries again.
    fn take_file(&mut self, kind: Kind, r: &PRow, f: &PFile, conflict: bool, held: &mut Held) {
        let mut to = f.key.clone();
        let mut keep_name = false;
        let mut keep_folder = false;
        if !self.free_for(held, &to, &r.id) {
            keep_name = true;
            to = (r.held.0.clone(), f.key.1.clone());
            if !self.free_for(held, &to, &r.id) {
                keep_folder = true;
                to = r.held.clone();
            }
        }
        let holder = holder(held, &f.key, &r.id);
        let row_name = r.name.clone();
        let (op, after_hash, replaced) = match &f.content {
            Content::Query(q) => {
                let row = &self.rows.queries[r.at].row;
                let mut after = q.clone();
                if keep_name {
                    after.name = row_name.clone();
                }
                let folder = (!keep_folder
                    && folder_key(row.folder.as_deref()) != folder_key(Some(&q.folder)))
                .then(|| Some(q.folder.clone()).filter(|x| !x.is_empty()));
                let patch = SavedQueryPatch {
                    name: (!keep_name).then(|| q.name.clone()),
                    query: Some(q.query.clone()),
                    parameters: Some((!q.parameters.is_empty()).then(|| q.parameters.clone())),
                    description: Some(non_empty(&q.description)),
                    database_type: Some(non_empty(&q.database)),
                    tags: Some(Some(q.tags.clone())),
                    folder: folder.clone(),
                    ..Default::default()
                };
                let changed = folder.is_some() || query_hash(&after) != r.hash;
                (
                    changed.then(|| RowOp::UpdateQuery {
                        id: r.id.clone(),
                        patch,
                    }),
                    query_hash(&after),
                    None,
                )
            }
            Content::Dashboard(d) => {
                let mut after = d.clone();
                if keep_name {
                    after.name = row_name.clone();
                }
                let patch = DashboardPatch {
                    name: (!keep_name).then(|| d.name.clone()),
                    description: Some(non_empty(&d.description)),
                    widgets: Some(raw_or(&d.widgets, "[]")),
                    viewport: Some(raw_or(
                        d.viewport.as_deref().unwrap_or(DEFAULT_VIEWPORT),
                        DEFAULT_VIEWPORT,
                    )),
                    date_filter: Some(d.date_filter.as_deref().map(|t| raw_or(t, "null"))),
                    capture_version: true,
                    ..Default::default()
                };
                let changed = dashboard_hash(&after) != r.hash;
                (
                    changed.then(|| RowOp::UpdateDashboard {
                        id: r.id.clone(),
                        patch,
                    }),
                    dashboard_hash(&after),
                    None,
                )
            }
            Content::Template(t) => {
                let row = &self.rows.connections[r.at].row;
                let (patch, values) = template_patch(row, t, keep_name);
                let mut after = t.clone();
                if keep_name {
                    after.name = row_name.clone();
                }
                let changed = template_hash(&after) != r.hash;
                (
                    changed.then(|| RowOp::UpdateConnection {
                        id: r.id.clone(),
                        patch,
                    }),
                    template_hash(&after),
                    Some(values),
                )
            }
        };
        if let Some(op) = op {
            if !self.push_checked(f, op) {
                return;
            }
        }
        if to != r.held {
            Self::move_name(held, &r.held, to, &r.id);
        }
        if keep_name {
            if let Some(taken_by) = holder {
                self.plan.notices.push(SyncNotice::NameTaken {
                    path: notice_path(&f.path),
                    taken_by,
                });
            }
        }
        if conflict && after_hash != r.hash {
            self.plan.notices.push(SyncNotice::Conflict {
                kind,
                id: r.id.clone(),
                replaced: replaced.filter(|_| kind == Kind::Connection),
            });
        }
        let link = Link {
            path: Some(f.path.clone()),
            base: Some(if keep_name {
                after_hash
            } else {
                f.hash.clone()
            }),
            file_id: f.file_id.clone(),
        };
        if r.link != link {
            self.set_link(kind, &r.id, link, None);
        }
    }

    /// A file for the row: its content, its id, and what the file it
    /// replaces held that the row doesn't (a template's labels).
    fn text_for(
        &self,
        kind: Kind,
        r: &PRow,
        file_id: Option<String>,
        old: Option<&PFile>,
        as_named: Option<&str>,
    ) -> String {
        let named = |n: &mut String| {
            if let Some(name) = as_named {
                *n = name.to_string();
            }
        };
        match kind {
            Kind::SavedQuery => {
                let mut q = query_file(&self.rows.queries[r.at].row);
                q.file_id = file_id;
                named(&mut q.name);
                write_query(&q)
            }
            Kind::Dashboard => {
                let mut d = dashboard_file(&self.rows.dashboards[r.at].row);
                d.file_id = file_id;
                named(&mut d.name);
                write_dashboard(&d)
            }
            Kind::Connection => {
                let mut t = template_file(&self.rows.connections[r.at].row);
                t.file_id = file_id;
                named(&mut t.name);
                if let Some(Content::Template(old)) = old.map(|o| &o.content) {
                    t.labels = old.labels.clone();
                }
                write_template(&t)
            }
        }
    }

    /// A row whose share stored a path but whose file was never written:
    /// write it now, at its path unless a file took it.
    fn write_unwritten(&mut self, kind: Kind, r: &PRow, path: &str) {
        let target = if self.taken.contains(path) {
            let taken = &self.taken;
            free_path(dir_of(path), &file_stem(&r.name), ext_for(kind), &|p| {
                taken.contains(p)
            })
        } else {
            path.to_string()
        };
        let file_id = r.link.file_id.clone().unwrap_or_else(|| self.ids.file_id());
        let text = self.text_for(kind, r, Some(file_id.clone()), None, None);
        self.plan.files.push(FileOp::Write {
            rel_path: target.clone(),
            text,
            expect_hash: None,
        });
        self.set_link(
            kind,
            &r.id,
            Link {
                path: Some(target.clone()),
                base: Some(r.hash.clone()),
                file_id: Some(file_id),
            },
            Some(target),
        );
    }

    /// A new row from a file no row has; its id, or `None` when the library
    /// would refuse it. A template whose name the project holds is
    /// imported under a free one ("Warehouse (2)"): that name is
    /// withheld like an update's (flag 4), the base being the row's own
    /// hash, and `announce` names the file.
    fn create(
        &mut self,
        kind: Kind,
        f: &PFile,
        keep_id: bool,
        announce: bool,
        held: &mut Held,
        created: &mut HashSet<String>,
    ) -> Option<String> {
        let id = self.ids.row_id(kind);
        let pid = self.rows.project_id.clone();
        let mut base = f.hash.clone();
        let mut key = f.key.clone();
        let op = match &f.content {
            Content::Query(q) => RowOp::CreateQuery {
                id: id.clone(),
                draft: query_draft(q, &pid),
            },
            Content::Dashboard(d) => RowOp::CreateDashboard {
                id: id.clone(),
                draft: dashboard_draft(d, &pid),
            },
            Content::Template(t) => {
                let mut draft = connection_draft(t, &pid, self.link.template_link(&f.path));
                if let Some(holder) = holder(held, &key, "") {
                    let taken: HashSet<String> = held.keys().map(|(n, _)| n.clone()).collect();
                    draft.name = free_name(&t.name, &taken);
                    let mut as_named = t.clone();
                    as_named.name = draft.name.clone();
                    base = template_hash(&as_named);
                    key = (name_key(&draft.name), String::new());
                    if announce {
                        self.plan.notices.push(SyncNotice::NameTaken {
                            path: notice_path(&f.path),
                            taken_by: holder,
                        });
                    }
                }
                RowOp::CreateConnection {
                    id: id.clone(),
                    draft,
                }
            }
        };
        if !self.push_checked(f, op) {
            return None;
        }
        hold(held, key, &id);
        created.insert(id.clone());
        self.set_link(
            kind,
            &id,
            Link {
                path: Some(f.path.clone()),
                base: Some(base),
                file_id: f.file_id.clone().filter(|_| keep_id),
            },
            None,
        );
        Some(id)
    }
}

/// The patch that gives `row` the template's fields, and the local values
/// it replaces (the notice's `replaced`). `keep_name` withholds the name.
fn template_patch(
    row: &PersistedConnection,
    t: &TemplateFile,
    keep_name: bool,
) -> (ConnectionPatch, ReplacedValues) {
    let mut patch = ConnectionPatch::default();
    let mut was = ReplacedValues::default();
    if row.name != t.name && !keep_name {
        patch.name = Some(t.name.clone());
        was.name = Some(row.name.clone());
    }
    if row.host != t.host {
        patch.host = Some(t.host.clone());
        was.host = Some(row.host.clone());
    }
    if row.port != t.port {
        patch.port = Some(t.port);
        was.port = Some(row.port);
    }
    if row.database_name != t.database_name {
        patch.database_name = Some(t.database_name.clone());
        was.database_name = Some(row.database_name.clone());
    }
    let ssl = row.ssl_mode.clone().filter(|s| !s.is_empty());
    if ssl != t.ssl_mode {
        patch.ssl_mode = Some(t.ssl_mode.clone());
        was.ssl_mode = Some(ssl);
    }
    let ssh = ssh_of(row);
    let enabled = ssh.as_ref().filter(|s| s.enabled);
    let (host_now, port_now) = (enabled.map(|s| s.host.clone()), enabled.map(|s| s.port));
    let (host_then, port_then) = match &t.ssh_tunnel {
        Some(s) => (
            Some(s.host.clone().unwrap_or_default()),
            Some(s.port.unwrap_or(22.0)),
        ),
        None => (None, None),
    };
    if host_now != host_then || port_now != port_then {
        if host_now != host_then {
            was.ssh_host = Some(host_now);
        }
        if port_now != port_then {
            was.ssh_port = Some(port_now);
        }
        patch.ssh_tunnel = Some(match (&t.ssh_tunnel, ssh) {
            (Some(_), Some(mut s)) => {
                s.enabled = true;
                s.host = host_then.unwrap_or_default();
                s.port = port_then.unwrap_or(22.0);
                Some(s)
            }
            (Some(_), None) => Some(SshTunnelConfig {
                enabled: true,
                host: host_then.unwrap_or_default(),
                port: port_then.unwrap_or(22.0),
                username: String::new(),
                auth_method: "key".into(),
                key_path: None,
            }),
            (None, Some(mut s)) => {
                s.enabled = false;
                Some(s)
            }
            (None, None) => None,
        });
    }
    (patch, was)
}

/// One reconcile of a linked project's directory against its rows.
/// A conflicted scan plans nothing. Every
/// planned row write passes the library's checks under `limits` (C1).
pub fn plan_sync(
    link: &ProjectLink,
    scan: &DirScan,
    rows: &SharedRows,
    limits: &Limits,
    ids: &mut dyn IdSource,
) -> SyncPlan {
    if scan.conflicted {
        return SyncPlan {
            conflicted: true,
            ..Default::default()
        };
    }
    let mut sync = Sync {
        link,
        rows,
        limits,
        ids,
        skipped: HashSet::new(),
        taken: TakenPaths::from_paths(scan.files.iter().map(|f| &f.rel_path)),
        plan: SyncPlan::default(),
    };
    for s in &scan.skipped {
        sync.skip(&s.rel_path, s.why);
    }
    for kind in KINDS {
        sync.run(kind, scan);
    }
    sync.plan
}

/// The file operations for one row a library call changed.
/// Nothing for a row that isn't shared (or a connection never shared);
/// a rename moves the file (write the new path, then delete
/// the old one unless they are the same ignoring case); a content-neutral
/// change (a viewport; `starred`) writes nothing. A query folder that
/// fails the path rules is `INVALID_ARGUMENT`.
pub fn plan_publish(
    link: &ProjectLink,
    change: &RowChange<'_>,
    ctx: &PublishContext<'_>,
    ids: &mut dyn IdSource,
) -> Result<PublishPlan, LibraryError> {
    let (kind, id, row_link, renamed, ext, active, dir) = match change {
        RowChange::Project { name } => return Ok(publish_project(link, name, ctx)),
        RowChange::Query {
            row,
            link: l,
            renamed,
        } => {
            let folder = row.and_then(|r| r.folder.clone()).unwrap_or_default();
            check_folder(&folder).map_err(|_| {
                LibraryError::invalid("The query's folder can't be a path in the repository.")
            })?;
            let mut dir = link.kind_dir(Kind::SavedQuery);
            if !folder.is_empty() {
                dir = format!("{dir}/{folder}");
            }
            (
                Kind::SavedQuery,
                row.map(|r| r.id.clone()),
                *l,
                *renamed,
                ".sql",
                row.is_some_and(|r| r.shared),
                dir,
            )
        }
        RowChange::Dashboard {
            row,
            link: l,
            renamed,
        } => (
            Kind::Dashboard,
            row.map(|r| r.id.clone()),
            *l,
            *renamed,
            ".json",
            row.is_some_and(|r| r.shared),
            link.kind_dir(Kind::Dashboard),
        ),
        RowChange::Connection {
            row,
            link: l,
            renamed,
            shared_now,
        } => (
            Kind::Connection,
            row.map(|r| r.id.clone()),
            *l,
            *renamed,
            ".yaml",
            row.is_some_and(|r| r.is_local_only != Some(true)) && (l.path.is_some() || *shared_now),
            link.kind_dir(Kind::Connection),
        ),
    };
    let row_folder = match change {
        RowChange::Query { row, .. } => row.and_then(|r| r.folder.clone()).unwrap_or_default(),
        _ => String::new(),
    };
    let row_name = match change {
        RowChange::Query { row, .. } => row.map(|r| r.name.clone()),
        RowChange::Dashboard { row, .. } => row.map(|r| r.name.clone()),
        RowChange::Connection { row, .. } => row.map(|r| r.name.clone()),
        RowChange::Project { .. } => None,
    };
    // R2: the stored file differs from the row only by a name the last
    // sync withheld (the base is F′, the file under the row's name). Then
    // a write keeps the file's name, and both a write and a delete expect
    // the file's own hash.
    let withheld: Option<(String, String)> = match (&row_link.path, ctx.existing, &row_name) {
        (Some(stored), Some(text), Some(own)) => parse_content(stored, text).and_then(|c| {
            let theirs = content_name(&c).to_string();
            (name_key(&theirs) != name_key(own)
                && row_link.base.as_deref() == Some(hash_named(&c, own).as_str()))
            .then(|| (theirs, file_content_hash(&c)))
        }),
        _ => None,
    };
    if !active {
        // Unshared, local-only or removed: delete the stored file.
        let Some(path) = &row_link.path else {
            return Ok(PublishPlan::default());
        };
        return Ok(PublishPlan {
            files: vec![FileOp::Delete {
                rel_path: path.clone(),
                expect_hash: withheld.map(|(_, f)| f).or_else(|| row_link.base.clone()),
            }],
            on_success: id.map(|id| LinkUpdate {
                kind,
                id,
                link: Link::default(),
                pending_on: None,
            }),
            on_failure: None,
        });
    }
    let (Some(id), Some(name)) = (id, row_name) else {
        return Ok(PublishPlan::default());
    };
    let hash = row_hash(change).unwrap_or_default();
    let stem = file_stem(&name);
    let target = match &row_link.path {
        Some(stored) if renamed => {
            let stored_key = path_key(stored);
            let taken = |p: &str| path_key(p) != stored_key && (ctx.taken)(p);
            let ext_now =
                if kind == Kind::Connection && stored.to_ascii_lowercase().ends_with(".yml") {
                    ".yml"
                } else {
                    ext
                };
            // I1: a query moved to another folder goes there; otherwise the
            // file stays in its own directory (even one deeper than the
            // row's folder).
            let stored_dir = dir_of(stored);
            let to_dir = if kind == Kind::SavedQuery {
                let queries = link.kind_dir(Kind::SavedQuery);
                let stored_folder = stored_dir
                    .strip_prefix(&queries)
                    .map(|r| r.trim_start_matches('/'))
                    .unwrap_or(stored_dir);
                if folder_key(Some(stored_folder)) == folder_key(Some(&row_folder)) {
                    stored_dir.to_string()
                } else {
                    dir.clone()
                }
            } else {
                stored_dir.to_string()
            };
            let wanted = free_path(&to_dir, &stem, ext_now, &taken);
            if path_key(&wanted) == stored_key {
                stored.clone()
            } else {
                wanted
            }
        }
        Some(stored) => stored.clone(),
        None => free_path(&dir, &stem, ext, ctx.taken),
    };
    if Some(&target) == row_link.path.as_ref() && row_link.base.as_deref() == Some(hash.as_str()) {
        return Ok(PublishPlan::default());
    }
    let existing_id = ctx.existing.and_then(|t| match kind {
        Kind::SavedQuery => parse_query(t, "x.sql", "").file_id,
        Kind::Dashboard => parse_dashboard(t, "x.json").and_then(|d| d.file_id),
        Kind::Connection => parse_template(t).and_then(|t| t.file_id),
    });
    let file_id = row_link
        .file_id
        .clone()
        .or(existing_id)
        .unwrap_or_else(|| ids.file_id());
    let in_place = Some(&target) == row_link.path.as_ref();
    let as_named = withheld
        .as_ref()
        .filter(|_| in_place)
        .map(|(n, _)| n.clone());
    let named = |n: &mut String| {
        if let Some(name) = &as_named {
            *n = name.clone();
        }
    };
    let text = match change {
        RowChange::Query { row: Some(r), .. } => {
            let mut q = query_file(r);
            q.file_id = Some(file_id.clone());
            named(&mut q.name);
            write_query(&q)
        }
        RowChange::Dashboard { row: Some(r), .. } => {
            let mut d = dashboard_file(r);
            d.file_id = Some(file_id.clone());
            named(&mut d.name);
            write_dashboard(&d)
        }
        RowChange::Connection { row: Some(r), .. } => {
            let mut t = template_file(r);
            t.file_id = Some(file_id.clone());
            named(&mut t.name);
            if let Some(old) = ctx.existing.and_then(parse_template) {
                t.labels = old.labels;
            }
            write_template(&t)
        }
        _ => return Ok(PublishPlan::default()),
    };
    // M1: writing over the stored file expects what the base says it
    // holds (the file's own hash while a name is withheld, R2); a new path
    // expects no file at all.
    let expect_hash = if in_place {
        withheld
            .as_ref()
            .map(|(_, f)| f.clone())
            .or_else(|| row_link.base.clone())
    } else {
        None
    };
    let mut files = vec![FileOp::Write {
        rel_path: target.clone(),
        text,
        expect_hash,
    }];
    if let Some(stored) = &row_link.path {
        if path_key(stored) != path_key(&target) {
            files.push(FileOp::Delete {
                rel_path: stored.clone(),
                expect_hash: withheld
                    .as_ref()
                    .map(|(_, f)| f.clone())
                    .or_else(|| row_link.base.clone()),
            });
        }
    }
    Ok(PublishPlan {
        files,
        on_success: Some(LinkUpdate {
            kind,
            id: id.clone(),
            link: Link {
                path: Some(target.clone()),
                base: Some(hash),
                file_id: Some(file_id),
            },
            pending_on: None,
        }),
        on_failure: row_link.path.is_none().then_some(LinkUpdate {
            kind,
            id,
            link: Link {
                path: Some(target),
                base: None,
                file_id: None,
            },
            pending_on: None,
        }),
    })
}

/// `project.yaml` with the project's name and the file's description;
/// nothing when it already says so.
fn publish_project(link: &ProjectLink, name: &str, ctx: &PublishContext<'_>) -> PublishPlan {
    let existing = ctx.existing.map(|t| parse_project(t, &link.dir));
    if existing.as_ref().is_some_and(|p| p.name == name) {
        return PublishPlan::default();
    }
    let file = ProjectFile {
        name: name.to_string(),
        description: existing.and_then(|p| p.description),
        dir: link.dir.clone(),
    };
    PublishPlan {
        files: vec![FileOp::Write {
            rel_path: link.project_yaml(),
            text: write_project(&file),
            // The file as read, or none when there was none.
            expect_hash: ctx
                .existing
                .and_then(|t| file_hash(&link.project_yaml(), t))
                .map(|(h, _)| h),
        }],
        on_success: None,
        on_failure: None,
    }
}

/// The directory a first link uses: an existing one whose
/// `project.yaml` name has the project's `name_key`, else a free stem
/// among them. `dirs` holds each directory under `projects/` with
/// the name its `project.yaml` gives (the directory's own without one).
pub fn pick_project_dir(name: &str, dirs: &[(String, String)]) -> String {
    let key = name_key(name);
    if let Some((dir, _)) = dirs.iter().find(|(_, n)| name_key(n) == key) {
        return dir.clone();
    }
    let taken = TakenPaths::from_paths(dirs.iter().map(|(d, _)| d));
    free_path("", &file_stem(name), "", &|p| taken.contains(p))
}
