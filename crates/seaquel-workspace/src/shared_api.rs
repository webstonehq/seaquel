//! The answers and patches of Core's shared projection and imports calls
//! (phase 5e): `shared.sync`, `linkProject`, `unlinkProject`, `scan`,
//! `repoUpdate` and `importsCreate`. Core builds them; `seaquel-rpc`
//! serves them. No `Debug` shows a name, a path or a host.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::shared::{SkipReason, SyncNotice};

/// What a sync did (`shared.sync`, `syncRepo`, and the link and import
/// calls that end with one).
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SyncReport {
    /// The repo has conflicted files: nothing was read or written.
    pub conflicted: bool,
    pub rows_changed: u32,
    pub files_written: u32,
    /// After the once-per-session rule.
    pub notices: Vec<SyncNotice>,
    /// A repo's sync goes on past a project that fails; each
    /// failure is named here.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<ProjectFailure>>", optional))]
    pub failures: Vec<ProjectFailure>,
    /// Projects the scan skipped whole (past its file count or size):
    /// nothing in them was read or written. Named on every
    /// sync, outside the once-per-session rule.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<SkippedProject>>", optional))]
    pub skipped_projects: Vec<SkippedProject>,
}

/// A project a sync skipped whole, and why (`tooMany` or `tooLarge`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SkippedProject {
    pub project_id: String,
    pub why: SkipReason,
}

/// One project a multi-project call (a repo's sync, an import) couldn't
/// finish. The message may name a path relative to `.seaquel/`; never
/// logged.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ProjectFailure {
    /// The project (a sync's), when it exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// The repo's project directory (an import's).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    pub code: String,
    pub message: String,
}

impl fmt::Debug for ProjectFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectFailure")
            .field("project_id", &self.project_id)
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

/// `shared.importProjects`' answer: each directory is imported
/// whole or not at all.
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ImportedProjects {
    /// The new projects, in the order of the directories asked for.
    pub project_ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<ProjectFailure>>", optional))]
    pub failures: Vec<ProjectFailure>,
}

impl fmt::Debug for ImportedProjects {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportedProjects")
            .field("project_ids", &self.project_ids)
            .field("failures", &self.failures)
            .finish()
    }
}

impl fmt::Debug for SyncReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SyncReport")
            .field("conflicted", &self.conflicted)
            .field("rows_changed", &self.rows_changed)
            .field("files_written", &self.files_written)
            .field("notices", &self.notices.len())
            .field("failures", &self.failures)
            .field("skipped_projects", &self.skipped_projects)
            .finish()
    }
}

impl SyncReport {
    /// Adds another project's report (a repo's sync).
    pub fn merge(&mut self, other: SyncReport) {
        self.conflicted |= other.conflicted;
        self.rows_changed += other.rows_changed;
        self.files_written += other.files_written;
        self.notices.extend(other.notices);
        self.failures.extend(other.failures);
        self.skipped_projects.extend(other.skipped_projects);
    }
}

/// What `shared.unlinkPreview` answers: the
/// connections an unlink with `removeImported` would remove, so the unlink
/// dialog lists exactly those. Read only.
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UnlinkPreview {
    /// The connections the repo brought, linked to this project's
    /// templates in its repo and directory.
    pub imported_connection_ids: Vec<String>,
}

impl fmt::Debug for UnlinkPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlinkPreview")
            .field("imported_connection_ids", &self.imported_connection_ids)
            .finish()
    }
}

/// What `shared.unlinkProject` removed and kept.
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct UnlinkReport {
    /// The connections the repo brought (imported from the project's
    /// templates), removed with their secrets because the call asked to.
    pub removed_connection_ids: Vec<String>,
    /// The linked connections that stay, unlinked and local-only, with their
    /// secrets: the user's own, and the imported ones when the call didn't
    /// ask to remove them.
    pub kept_connection_ids: Vec<String>,
    /// The repo was forgotten: no project uses it any more.
    pub repo_removed: bool,
}

impl fmt::Debug for UnlinkReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UnlinkReport")
            .field("removed_connection_ids", &self.removed_connection_ids)
            .field("kept_connection_ids", &self.kept_connection_ids)
            .field("repo_removed", &self.repo_removed)
            .finish()
    }
}

/// A template as the import dialog shows it: no credentials (the reader
/// drops them).
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PreviewTemplate {
    /// Relative to `.seaquel/`.
    pub path: String,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

impl fmt::Debug for PreviewTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewTemplate")
            .field("ty", &self.ty)
            .finish_non_exhaustive()
    }
}

/// One project directory of a repo, for the import dialog
/// (`shared.scan`).
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct PreviewProject {
    pub dir: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub queries: u32,
    pub dashboards: u32,
    pub templates: Vec<PreviewTemplate>,
    /// Files the scan didn't read.
    pub skipped: u32,
    /// The local projects already linked to this directory.
    pub linked_project_ids: Vec<String>,
}

impl fmt::Debug for PreviewProject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreviewProject")
            .field("queries", &self.queries)
            .field("dashboards", &self.dashboards)
            .field("templates", &self.templates.len())
            .field("skipped", &self.skipped)
            .finish_non_exhaustive()
    }
}

/// What a repo holds (`shared.scan`): read-only.
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RepoPreview {
    pub conflicted: bool,
    pub projects: Vec<PreviewProject>,
    /// Project directories the scan didn't offer (a symlink).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "ts", ts(as = "Option<Vec<SkippedDir>>", optional))]
    pub skipped_dirs: Vec<SkippedDir>,
}

/// A directory under `.seaquel/projects` the scan skipped, and why.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SkippedDir {
    pub dir: String,
    pub why: SkipReason,
}

impl fmt::Debug for SkippedDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SkippedDir")
            .field("why", &self.why)
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for RepoPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RepoPreview")
            .field("conflicted", &self.conflicted)
            .field("projects", &self.projects.len())
            .field("skipped_dirs", &self.skipped_dirs)
            .finish()
    }
}

/// `shared.repoUpdate`'s patch: only the fields it names change, the rest
/// of the stored JSON stays byte for byte. `lastSyncAt` is
/// Core's (a pull or push sets it), `syncStatus` is the GUI's view.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct RepoPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

impl fmt::Debug for RepoPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RepoPatch")
            .field("name", &self.name.is_some())
            .field("remote_url", &self.remote_url.is_some())
            .field("branch", &self.branch.is_some())
            .finish()
    }
}

/// What happened to one key of an `importsCreate`.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, optional_fields))]
pub struct ImportKeyOutcome {
    pub key: String,
    /// `imported`, `duplicate` (a saved connection, or one this call just
    /// imported, has the same type, host, port, database and user) or
    /// `notFound` (no candidate has the key any more).
    #[cfg_attr(
        feature = "ts",
        ts(type = "\"imported\" | \"duplicate\" | \"notFound\"")
    )]
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
}

impl fmt::Debug for ImportKeyOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportKeyOutcome")
            .field("status", &self.status)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// `importsCreate`'s answer, one outcome per key in the order given.
#[derive(Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ImportOutcome {
    pub results: Vec<ImportKeyOutcome>,
}

impl fmt::Debug for ImportOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportOutcome")
            .field("results", &self.results)
            .finish()
    }
}
