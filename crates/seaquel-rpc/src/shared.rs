//! The `shared` group of the workspace RPC (phase 5e): shared projects'
//! `.seaquel` projection and the repo list, through Core
//! (`Workspace::shared_link_project`, `shared_sync`, …).
//!
//! Wire shape, like the other groups:
//!
//! ```json
//! {"method":"shared","params":{"method":"sync","params":{"projectId":"p"}}}
//! {"method":"shared","result":{"method":"sync","result":{"value":{"conflicted":false,"rowsChanged":1,"filesWritten":0,"notices":[]},"seq":{"epoch":"…","n":9}}}}
//! ```
//!
//! - **Desktop only**: [`crate::dispatch_workspace`] refuses
//!   the group with `NOT_SUPPORTED` on a Core built without
//!   `LocalFiles::Allowed` (the web server's), whatever features Cargo
//!   unified, and so does Core itself. Without the `git` and `storage`
//!   features every method answers `NOT_SUPPORTED`.
//! - `path` is an absolute path to a repo's working tree; `dirs` are
//!   directories under its `.seaquel/projects/`; `share` names the
//!   project's connections the link dialog ticked.
//! - Every write and list answers a `Seqd`; `scan` is read-only and
//!   answers the preview alone. The repo rows cross as their stored text.
//! - Errors keep Core's codes: `REPO_NOT_FOUND`, `REPO_IN_USE`,
//!   `PROJECT_NOT_LINKED`, `PROJECT_ALREADY_LINKED`, `REPO_CONFLICTED`,
//!   `FILE_ERROR` (its message may name a path relative to `.seaquel/`),
//!   `PROJECT_NOT_FOUND`, `INVALID_ARGUMENT` and the storage codes. A repo's
//!   sync and an import of several projects go on past a project that
//!   fails and name it in `failures`.
//!
//! `Debug` shows the method only: a path holds the user's home and project
//! names, and a directory is a project's name.

use std::fmt;

use seaquel_core::domain::shared_api::{
    ImportedProjects, RepoPatch, RepoPreview, SyncReport, UnlinkPreview, UnlinkReport,
};
use seaquel_core::{Core, Seqd, Workspace, WriteOrigin};
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::workspace::RpcError;

/// A `shared` call.
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SharedRequest {
    /// Every stored repo, as stored.
    ReposList,
    /// Register the repo at `path`, or answer the one already there.
    RepoRegister {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        remote_url: Option<String>,
    },
    /// Change the fields `patch` names; the rest stays byte for byte.
    RepoUpdate { id: String, patch: RepoPatch },
    /// Forget a repo no project links to (`REPO_IN_USE` otherwise).
    RepoRemove { id: String },
    /// Link a project to the repo at `path`, export the ticked
    /// connections' templates, and sync.
    LinkProject {
        project_id: String,
        path: String,
        share: Vec<String>,
    },
    /// Unlink a project: the user's own connections stay, unlinked
    /// and local-only; the ones the repo brought go when `remove_imported`
    /// (the user confirmed), and stay like the others otherwise. Its rows'
    /// links are cleared.
    UnlinkProject {
        project_id: String,
        remove_imported: bool,
    },
    /// The connections an unlink with `remove_imported` would remove (the
    /// unlink dialog's list). Read only.
    UnlinkPreview { project_id: String },
    /// The import dialog's preview of a repo: read-only.
    Scan { path: String },
    /// One new project per directory, each linked and synced.
    ImportProjects { path: String, dirs: Vec<String> },
    /// Sync one project.
    Sync { project_id: String },
    /// Sync every project linked to the repo (after a pull, a commit or a
    /// conflict resolution).
    SyncRepo { repo_id: String },
}

impl SharedRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            Self::ReposList => "reposList",
            Self::RepoRegister { .. } => "repoRegister",
            Self::RepoUpdate { .. } => "repoUpdate",
            Self::RepoRemove { .. } => "repoRemove",
            Self::LinkProject { .. } => "linkProject",
            Self::UnlinkProject { .. } => "unlinkProject",
            Self::UnlinkPreview { .. } => "unlinkPreview",
            Self::Scan { .. } => "scan",
            Self::ImportProjects { .. } => "importProjects",
            Self::Sync { .. } => "sync",
            Self::SyncRepo { .. } => "syncRepo",
        }
    }
}

/// The method only: paths, names and directories name the user and their
/// projects.
impl fmt::Debug for SharedRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedRequest")
            .field("method", &self.method())
            .finish_non_exhaustive()
    }
}

/// A `shared` call's result, as `{"method": …, "result": …}`.
// Not `Deserialize`: results only go out.
#[derive(Serialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum SharedResponse {
    ReposList(
        #[cfg_attr(
            feature = "ts",
            ts(as = "Seqd<Vec<seaquel_types::storage::PersistedSharedQueryRepo>>")
        )]
        Seqd<Vec<Box<RawValue>>>,
    ),
    RepoRegister(
        #[cfg_attr(
            feature = "ts",
            ts(as = "Seqd<seaquel_types::storage::PersistedSharedQueryRepo>")
        )]
        Seqd<Box<RawValue>>,
    ),
    RepoUpdate(
        #[cfg_attr(
            feature = "ts",
            ts(as = "Seqd<seaquel_types::storage::PersistedSharedQueryRepo>")
        )]
        Seqd<Box<RawValue>>,
    ),
    RepoRemove(Seqd<()>),
    LinkProject(Seqd<SyncReport>),
    UnlinkProject(Seqd<UnlinkReport>),
    UnlinkPreview(UnlinkPreview),
    Scan(RepoPreview),
    ImportProjects(Seqd<ImportedProjects>),
    Sync(Seqd<SyncReport>),
    SyncRepo(Seqd<SyncReport>),
}

/// The method, and the sequence where there is one.
impl fmt::Debug for SharedResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (method, seq) = match self {
            Self::ReposList(r) => ("reposList", Some(&r.seq)),
            Self::RepoRegister(r) => ("repoRegister", Some(&r.seq)),
            Self::RepoUpdate(r) => ("repoUpdate", Some(&r.seq)),
            Self::RepoRemove(r) => ("repoRemove", Some(&r.seq)),
            Self::LinkProject(r) => ("linkProject", Some(&r.seq)),
            Self::UnlinkProject(r) => ("unlinkProject", Some(&r.seq)),
            Self::UnlinkPreview(_) => ("unlinkPreview", None),
            Self::Scan(_) => ("scan", None),
            Self::ImportProjects(r) => ("importProjects", Some(&r.seq)),
            Self::Sync(r) => ("sync", Some(&r.seq)),
            Self::SyncRepo(r) => ("syncRepo", Some(&r.seq)),
        };
        f.debug_struct("SharedResponse")
            .field("method", &method)
            .field("seq", &seq)
            .finish_non_exhaustive()
    }
}

#[cfg(not(all(feature = "git", feature = "storage")))]
pub(crate) async fn shared(
    _: &Core,
    _: &Workspace,
    _: SharedRequest,
    _: &WriteOrigin,
) -> Result<SharedResponse, RpcError> {
    Err(RpcError::not_supported("Shared projects"))
}

/// Serve a `shared` call on `ws`. The caller checked `LocalFiles`; Core
/// checks it again.
#[cfg(all(feature = "git", feature = "storage"))]
pub(crate) async fn shared(
    core: &Core,
    ws: &Workspace,
    req: SharedRequest,
    origin: &WriteOrigin,
) -> Result<SharedResponse, RpcError> {
    use seaquel_core::SyncTarget;
    use SharedRequest as Q;
    use SharedResponse as R;

    Ok(match req {
        Q::ReposList => R::ReposList(ws.shared_repos(core).await?),
        Q::RepoRegister {
            path,
            name,
            remote_url,
        } => R::RepoRegister(
            ws.shared_repo_register(core, origin, &path, name, remote_url)
                .await?,
        ),
        Q::RepoUpdate { id, patch } => {
            R::RepoUpdate(ws.shared_repo_update(core, origin, &id, patch).await?)
        }
        Q::RepoRemove { id } => R::RepoRemove(ws.shared_repo_remove(core, origin, &id).await?),
        Q::LinkProject {
            project_id,
            path,
            share,
        } => R::LinkProject(
            ws.shared_link_project(core, origin, &project_id, &path, &share)
                .await?,
        ),
        Q::UnlinkProject {
            project_id,
            remove_imported,
        } => R::UnlinkProject(
            ws.shared_unlink_project(core, origin, &project_id, remove_imported)
                .await?,
        ),
        Q::UnlinkPreview { project_id } => {
            R::UnlinkPreview(ws.shared_unlink_preview(core, &project_id).await?)
        }
        Q::Scan { path } => R::Scan(ws.shared_scan(core, &path).await?),
        Q::ImportProjects { path, dirs } => R::ImportProjects(
            ws.shared_import_projects(core, origin, &path, &dirs)
                .await?,
        ),
        Q::Sync { project_id } => R::Sync(
            ws.shared_sync(core, origin, SyncTarget::Project(project_id))
                .await?,
        ),
        Q::SyncRepo { repo_id } => R::SyncRepo(
            ws.shared_sync(core, origin, SyncTarget::Repo(repo_id))
                .await?,
        ),
    })
}
