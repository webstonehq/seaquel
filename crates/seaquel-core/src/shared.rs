//! The `.seaquel` projection of shared projects (phase 5e): linking a
//! project to a repo, the repo list, the sync that reconciles a project's
//! directory with its rows in both directions, and the publish that writes
//! one row's file after a library call changed it.
//!
//! Every decision is `seaquel_workspace::shared`'s (pairing, the three-way
//! rule, file names, the formats); the file I/O is `seaquel-git`'s `tree`
//! (no symlinks, Decision 32). This module orders them:
//!
//! - **Locks.** Each call takes the repo's lock ([`Core::repo_lock`]) first
//!   and the storage's write lock after, never the other way round, and
//!   never reads or writes a file while a `WriteTx` is open.
//! - **Sync** ([`Workspace::shared_sync`]): under the repo lock, read the
//!   index (a conflicted repo changes nothing, Decision 35) and scan the
//!   directory; then, in one `WriteTx`, read the project's rows, plan
//!   ([`plan_sync`] with the limits this Core was built with), apply the row
//!   ops in plan order and store the links that wait for no write; commit;
//!   then write the files (each on its own; a sync never deletes or
//!   renames); then store the links whose write succeeded. One event per
//!   kind and scope (the rows' right after their commit, the late links'
//!   and `sharedRepo` after the files), none when nothing changed.
//! - **Publish** (Decision 36, from the library calls): after the call's
//!   commit, under the repo lock, read the row again, plan
//!   ([`plan_publish`]), apply the files as a sequence (stop at the first
//!   failure), then store `on_success`, or `on_failure` after a failed write
//!   (never after a refusal). A write that finds a teammate's change on
//!   disk (`expect_hash`) writes nothing: the project syncs instead and the
//!   answer is `FILE_CHANGED`.
//! - **Removals** (Decision 37): unsharing or removing a shared row deletes
//!   its file first under the repo lock, keeping the bytes, then writes the
//!   row; a failed row write puts the file back.
//!
//! Only a Core built with [`crate::LocalFiles::Allowed`] does any of this.
//! Logs carry activities, repo and project ids, counts and codes: never a
//! path, a name, a host or a file's content (Decision 50).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use log::{debug, info, warn};
use seaquel_git::tree::{self, ApplyOptions, OpOutcome, ScanBounds};
use seaquel_git::{Git, GitCredentials, GitSyncResult};
use seaquel_storage::{
    connections, dashboards, project_state, projects, saved_queries, shared_repos,
    user_credentials, SharedLink, WriteTx,
};
use seaquel_types::storage::{PersistedConnection, PersistedDashboard, PersistedSavedQuery};
use seaquel_workspace::library::{
    self as lib, ConnectionPatch, LibraryLimits, SavedQueryPatch, SecretChanges,
    CONNECTION_ID_PREFIX, CONNECTION_SECRETS, PROJECT_NOT_FOUND, SAVED_QUERY_ID_PREFIX,
};
use seaquel_workspace::shared::format::parse_template;
use seaquel_workspace::shared::names::{check_component, legacy_stem, TakenPaths};
use seaquel_workspace::shared::plan::template_path;
use seaquel_workspace::shared::{
    pick_project_dir, plan_publish, plan_sync, FileOp, IdSource, Kind, Limits, Link, LinkUpdate,
    LinkedConnection, LinkedDashboard, LinkedQuery, ProjectLink, PublishContext, PublishOutcome,
    PublishStatus, RowChange, RowOp, SharedRows, SEAQUEL_DIR,
};
use seaquel_workspace::shared::{DirScan, SkipReason};
pub use seaquel_workspace::shared_api::{
    ImportedProjects, PreviewProject, PreviewTemplate, ProjectFailure, RepoPatch, RepoPreview,
    SkippedDir, SkippedProject, SyncReport, UnlinkPreview, UnlinkReport,
};
use seaquel_workspace::state::{DashboardPatch, DASHBOARD_ID_PREFIX};
use serde_json::value::RawValue;
use serde_json::Value;

use crate::changes::{SeqTicket, WriteOrigin};
use crate::git::{repo_path_key, RepoLock};
use crate::library::{
    connection_order_in, new_id, now, update_connection_in, update_saved_query_in,
};
use crate::library::{insert_connection_in, insert_linked_project_in, insert_saved_query_in};
use crate::projection::Publish;
use crate::state::{insert_dashboard_in, update_dashboard_in};
use crate::{ChangeSeq, Core, CoreError, Seqd, StoredKind, Workspace};

type Result<T> = std::result::Result<T, CoreError>;

/// No repo is registered with that id or path.
pub const REPO_NOT_FOUND: &str = "REPO_NOT_FOUND";
/// `linkProject` of a project linked to another repo.
pub const PROJECT_ALREADY_LINKED: &str = "PROJECT_ALREADY_LINKED";
/// `repoRemove` of a repo a project still links to.
pub const REPO_IN_USE: &str = "REPO_IN_USE";
/// The project has no repo (`git_repo_path`).
pub const PROJECT_NOT_LINKED: &str = "PROJECT_NOT_LINKED";
/// A link or an import of a repo with conflicted files (a sync answers
/// `conflicted: true` instead).
pub const REPO_CONFLICTED: &str = "REPO_CONFLICTED";
/// A file couldn't be read or written; the message may name its path
/// relative to `.seaquel/` (for the GUI only).
pub const FILE_ERROR: &str = tree::FILE_ERROR;
/// A publish found a teammate's change in the file: nothing was written,
/// the project synced, and the file's content won (Q20).
pub const FILE_CHANGED: &str = "FILE_CHANGED";

/// What `shared.sync` reconciles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncTarget {
    /// One project (activation, startup).
    Project(String),
    /// Every project linked to the repo (after a pull, a commit or a
    /// conflict resolution, Decision 35).
    Repo(String),
}

// ── Helpers ──

fn invalid(message: &str) -> CoreError {
    CoreError::new("INVALID_ARGUMENT", message)
}

fn repo_not_found() -> CoreError {
    CoreError::new(REPO_NOT_FOUND, "Repository not found.")
}

fn not_linked() -> CoreError {
    CoreError::new(
        PROJECT_NOT_LINKED,
        "This project isn't linked to a repository.",
    )
}

fn already_linked() -> CoreError {
    CoreError::new(
        PROJECT_ALREADY_LINKED,
        "This project is linked to another repository. Unlink it first.",
    )
}

fn project_not_found() -> CoreError {
    CoreError::new(PROJECT_NOT_FOUND, "Project not found.")
}

fn failed(code: &str, message: impl Into<String>) -> PublishOutcome {
    PublishOutcome {
        status: PublishStatus::Failed,
        code: Some(code.to_string()),
        message: Some(message.into()),
    }
}

fn status(status: PublishStatus) -> PublishOutcome {
    PublishOutcome {
        status,
        code: None,
        message: None,
    }
}

/// A stored repo row's `id`.
fn repo_id_of(raw: &RawValue) -> Option<String> {
    repo_field(raw, "id")
}

/// A string field of a stored repo row (the last of a repeated key, as
/// `JSON.parse` reads it).
fn repo_field(raw: &RawValue, field: &str) -> Option<String> {
    let value: Value = serde_json::from_str(raw.get()).ok()?;
    value.get(field)?.as_str().map(str::to_string)
}

fn json_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

fn raw_value(text: String) -> Box<RawValue> {
    RawValue::from_string(text)
        .unwrap_or_else(|_| RawValue::from_string("null".to_string()).expect("null is JSON"))
}

/// Core's ids: `<prefix><uuid v4>` for rows, a uuid v4 for files (Q22).
struct CoreIds;

impl IdSource for CoreIds {
    fn row_id(&mut self, kind: Kind) -> String {
        new_id(match kind {
            Kind::SavedQuery => SAVED_QUERY_ID_PREFIX,
            Kind::Dashboard => DASHBOARD_ID_PREFIX,
            Kind::Connection => CONNECTION_ID_PREFIX,
        })
    }

    fn file_id(&mut self) -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

fn shared_link(link: Option<SharedLink>) -> Link {
    let link = link.unwrap_or_default();
    Link {
        path: link.path,
        base: link.base,
        file_id: link.file_id,
    }
}

/// A connection's link: the path in its `shared_connection_id`.
fn connection_link(link: Option<SharedLink>) -> Link {
    let mut link = shared_link(link);
    link.path = link
        .path
        .as_deref()
        .and_then(template_path)
        .map(String::from);
    link
}

/// The stored rows naming one repo folder ([`Workspace::same_repo`]).
#[derive(Default)]
struct SameRepo {
    /// The first repo row whose path names it.
    repo_id: Option<String>,
    /// Every project linked to it, in rowid order.
    project_ids: Vec<String>,
}

/// A repo path's canonical form (review R8): [`tree::lock_key`] of the path
/// without trailing separators.
async fn canonical(path: &str) -> PathBuf {
    tree::lock_key(Path::new(&repo_path_key(path))).await
}

/// Where a project's files are.
#[derive(Clone)]
struct Linked {
    /// As stored in `git_repo_path`.
    repo_path: String,
    root: PathBuf,
    /// `None` when no repo row has the path (an older release's state).
    repo_id: Option<String>,
    dir: String,
    dir_stored: bool,
    project_name: String,
}

impl Linked {
    fn link(&self) -> ProjectLink {
        ProjectLink {
            repo_id: self.repo_id.clone().unwrap_or_default(),
            dir: self.dir.clone(),
        }
    }
}

/// Every kind's touched ids for one sync's events.
#[derive(Default)]
struct Touched {
    rows: Vec<(StoredKind, BTreeSet<String>)>,
    order: bool,
}

impl Touched {
    fn add(&mut self, kind: Kind, id: &str) {
        let kind = match kind {
            Kind::SavedQuery => StoredKind::SavedQuery,
            Kind::Dashboard => StoredKind::Dashboard,
            Kind::Connection => StoredKind::Connection,
        };
        match self.rows.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, ids)) => {
                ids.insert(id.to_string());
            }
            None => self.rows.push((kind, BTreeSet::from([id.to_string()]))),
        }
    }
}

/// An event waiting for its announcement at the end of a call.
struct Pending<'a> {
    ticket: SeqTicket<'a>,
    kind: StoredKind,
    scope: Option<String>,
    ids: Vec<String>,
}

fn row_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::SavedQuery => "savedQuery",
        Kind::Dashboard => "dashboard",
        Kind::Connection => "connection",
    }
}

/// Stores `u` (a connection's path goes to `shared_connection_id` as
/// `<repoId>:<path>`).
async fn store_link(tx: &mut WriteTx, repo_id: &str, u: &LinkUpdate) -> Result<()> {
    let path = match (u.kind, &u.link.path) {
        (Kind::Connection, Some(p)) => Some(format!("{repo_id}:{p}")),
        (_, p) => p.clone(),
    };
    let link = SharedLink {
        path,
        base: u.link.base.clone(),
        file_id: u.link.file_id.clone(),
    };
    match u.kind {
        Kind::SavedQuery => saved_queries::set_link(tx, &u.id, &link).await?,
        Kind::Dashboard => dashboards::set_link(tx, &u.id, &link).await?,
        Kind::Connection => connections::set_link(tx, &u.id, &link).await?,
    };
    Ok(())
}

/// How an unlink treats a connection of the project (Q31).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnlinkClass {
    /// Shared from here (`shared_origin` `exported`): always kept.
    Own,
    /// Brought by the repo (`imported`, or a link with no origin, which
    /// only an older release's template import stored).
    Imported,
}

/// `None` for a connection not linked to a template of `linked`'s repo
/// and directory (`prefix`, its `connections/` directory).
fn unlink_class(linked: &Linked, prefix: &str, c: &PersistedConnection) -> Option<UnlinkClass> {
    let ours = c.shared_connection_id.as_deref().is_some_and(|s| {
        linked
            .repo_id
            .as_deref()
            .is_none_or(|r| s.starts_with(&format!("{r}:")))
            && template_path(s).is_some_and(|p| p.starts_with(prefix))
    });
    if !ours {
        return None;
    }
    Some(
        if c.shared_origin.as_deref() == Some(connections::ORIGIN_EXPORTED) {
            UnlinkClass::Own
        } else {
            UnlinkClass::Imported
        },
    )
}

/// Registers the repo at `path` (or finds it) inside `tx`: idempotent by
/// path. Its id and whether it was new.
async fn register_repo_in(
    tx: &mut WriteTx,
    path: &str,
    name: &str,
    remote_url: &str,
    known: Option<&str>,
) -> Result<(String, bool)> {
    register_repo_as(tx, path, name, remote_url, known, None).await
}

/// [`register_repo_in`], with the id a new row takes when `id` names one
/// no row has (review I3: the id a project's template links carry).
async fn register_repo_as(
    tx: &mut WriteTx,
    path: &str,
    name: &str,
    remote_url: &str,
    known: Option<&str>,
    id: Option<&str>,
) -> Result<(String, bool)> {
    // Review R8: the row another spelling of this folder has, found before
    // the transaction ([`Workspace::same_repo`]).
    if let Some(known) = known {
        if shared_repos::get(&mut *tx, known).await?.is_some() {
            return Ok((known.to_string(), false));
        }
    }
    if let Some(raw) = shared_repos::get_by_path(&mut *tx, path).await? {
        if let Some(id) = repo_id_of(&raw) {
            return Ok((id, false));
        }
    }
    let id = match id {
        Some(id) if shared_repos::get(&mut *tx, id).await?.is_none() => id.to_string(),
        _ => new_id("repo-"),
    };
    let json = format!(
        "{{\"id\":{},\"name\":{},\"path\":{},\"remoteUrl\":{},\"branch\":\"main\",\
         \"lastSyncAt\":null,\"syncStatus\":\"uninitialized\"}}",
        json_string(&id),
        json_string(name),
        json_string(path),
        json_string(remote_url),
    );
    shared_repos::insert(tx, &raw_value(json)).await?;
    Ok((id, true))
}

// ── Reading a project's link ──

impl Workspace {
    /// Where `project_id`'s files are, read from the pool: `None` when the
    /// project has no repo.
    async fn linked(&self, project_id: &str) -> Result<Option<Linked>> {
        let project = projects::get(self.storage(), project_id)
            .await?
            .ok_or_else(project_not_found)?;
        let Some(repo_path) = project.git_repo_path.clone() else {
            return Ok(None);
        };
        let stored = projects::shared_dir(self.storage(), project_id).await?;
        let dir_stored = stored.is_some();
        let dir = stored.unwrap_or_else(|| legacy_stem(&project.name));
        if check_component(&dir).is_err() {
            return Err(CoreError::new(
                FILE_ERROR,
                "The project's folder in the repository isn't a valid name.",
            ));
        }
        let repo_id = self.same_repo(&repo_path).await?.repo_id;
        Ok(Some(Linked {
            root: PathBuf::from(&repo_path),
            repo_path,
            repo_id,
            dir,
            dir_stored,
            project_name: project.name,
        }))
    }

    /// The rows that name the folder `path` names (review R8): repo paths
    /// are compared in the canonical form the repo lock uses
    /// ([`tree::lock_key`]: symlinks resolved, the on-disk case, no trailing
    /// separator), not as spelled. Reads the pool and the disk, so call it
    /// before a write transaction, never inside one.
    async fn same_repo(&self, path: &str) -> Result<SameRepo> {
        let key = canonical(path).await;
        let mut keys: HashMap<String, PathBuf> = HashMap::new();
        let mut out = SameRepo::default();
        for project in projects::load_all(self.storage()).await? {
            let Some(p) = project.git_repo_path else {
                continue;
            };
            if !keys.contains_key(&p) {
                let k = canonical(&p).await;
                keys.insert(p.clone(), k);
            }
            if keys[&p] == key {
                out.project_ids.push(project.id);
            }
        }
        for raw in shared_repos::list(self.storage()).await? {
            let (Some(id), Some(p)) = (repo_id_of(&raw), repo_field(&raw, "path")) else {
                continue;
            };
            if !keys.contains_key(&p) {
                let k = canonical(&p).await;
                keys.insert(p.clone(), k);
            }
            if keys[&p] == key {
                out.repo_id = Some(id);
                break;
            }
        }
        Ok(out)
    }

    /// The repo lock of `project_id`'s repo, with its link read again
    /// under it (the link can change while the lock is awaited).
    async fn lock_linked(
        &self,
        core: &Core,
        project_id: &str,
    ) -> Result<Option<(RepoLock, Linked)>> {
        let Some(mut linked) = self.linked(project_id).await? else {
            return Ok(None);
        };
        for _ in 0..3 {
            let lock = core.repo_lock(&linked.root).await;
            match self.linked(project_id).await? {
                None => return Ok(None),
                Some(now) if now.repo_path == linked.repo_path => return Ok(Some((lock, now))),
                Some(now) => linked = now,
            }
        }
        Err(CoreError::new(
            "STORAGE_ERROR",
            "The project's repository kept changing.",
        ))
    }

    /// Registers the project's repo when no row has its path (an older
    /// release's state), so a template link has a repo id.
    async fn ensure_repo(&self, project_id: &str, linked: &mut Linked) -> Result<()> {
        if linked.repo_id.is_some() {
            return Ok(());
        }
        // Review I3: the id the project's template links already carry, so
        // its connections stay paired with their templates.
        let dir = format!("{SEAQUEL_DIR}/projects/{}/connections/", linked.dir);
        let carried = connections::list_in_project(self.storage(), project_id)
            .await?
            .into_iter()
            .filter_map(|c| c.shared_connection_id)
            .find_map(|link| {
                let path = template_path(&link)?;
                path.starts_with(&dir)
                    .then(|| link[..link.len() - path.len() - 1].to_string())
            })
            .filter(|id| !id.is_empty() && id.len() <= 256);
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let (id, _) = register_repo_as(
            &mut tx,
            &linked.repo_path,
            &linked.project_name,
            "",
            None,
            carried.as_deref(),
        )
        .await?;
        tx.commit().await?;
        self.announce(
            ticket,
            StoredKind::SharedRepo,
            None,
            Some(vec![id.clone()]),
            &WriteOrigin::none(),
        );
        linked.repo_id = Some(id);
        Ok(())
    }
}

// ── Sync ──

impl Workspace {
    /// Reconciles a project's directory with its rows, or every project
    /// linked to a repo (Decisions 34 and 35). A repo with conflicted files
    /// answers `conflicted: true` and nothing is written on either side.
    ///
    /// Errors: `NOT_SUPPORTED` without `LocalFiles`, `PROJECT_NOT_FOUND`,
    /// `PROJECT_NOT_LINKED`, `REPO_NOT_FOUND`, `FILE_ERROR` (the repo's
    /// folder isn't there), the storage codes. A file that can't be read is
    /// a notice, never an error.
    pub async fn shared_sync(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        target: SyncTarget,
    ) -> Result<Seqd<SyncReport>> {
        core.require_local_files()?;
        match target {
            SyncTarget::Project(project_id) => {
                debug!(activity = "shared.sync", project_id = log_id(&project_id); "Sync a project");
                let Some((_lock, linked)) = self.lock_linked(core, &project_id).await? else {
                    return Err(not_linked());
                };
                let (report, seq) = self
                    .sync_locked(core, origin, &project_id, linked, true)
                    .await?;
                Ok(Seqd::new(report, seq))
            }
            SyncTarget::Repo(repo_id) => {
                debug!(activity = "shared.syncRepo", repo_id = log_id(&repo_id); "Sync a repo");
                let path = self.repo_path(&repo_id).await?;
                let mut total = SyncReport::default();
                let mut seq = self.change_seq();
                // Review M5: one project's failure doesn't stop the others;
                // each is named in the report.
                for project_id in self.same_repo(&path).await?.project_ids {
                    let synced = async {
                        let Some((_lock, linked)) = self.lock_linked(core, &project_id).await?
                        else {
                            return Ok(None);
                        };
                        self.sync_locked(core, origin, &project_id, linked, true)
                            .await
                            .map(Some)
                    }
                    .await;
                    match synced {
                        Ok(Some((report, s))) => {
                            total.merge(report);
                            seq = s;
                        }
                        Ok(None) => {}
                        Err(e) => {
                            warn!(activity = "shared.syncRepo", project_id = log_id(&project_id), code = e.code.as_str(); "A project's sync failed");
                            total.failures.push(ProjectFailure {
                                project_id: Some(project_id.clone()),
                                dir: None,
                                code: e.code,
                                message: e.message,
                            });
                        }
                    }
                }
                Ok(Seqd::new(total, seq))
            }
        }
    }

    /// The stored path of the repo `repo_id`.
    async fn repo_path(&self, repo_id: &str) -> Result<String> {
        let raw = shared_repos::get(self.storage(), repo_id)
            .await?
            .ok_or_else(repo_not_found)?;
        repo_field(&raw, "path").ok_or_else(repo_not_found)
    }

    /// One project's sync, with its repo's lock held by the caller.
    /// `remember`: pass the notices through the session's memory (a sync a
    /// publish ran instead of a write doesn't, so the next sync still names
    /// what it found).
    async fn sync_locked(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        linked: Linked,
        remember: bool,
    ) -> Result<(SyncReport, ChangeSeq)> {
        // Boxed (review C1): inlined, these futures made each library
        // call's state machine deep enough to overflow a 2 MiB stack in a
        // debug build.
        Box::pin(self.sync_locked_inner(core, origin, project_id, linked, remember)).await
    }

    /// [`Workspace::sync_locked`], unboxed.
    async fn sync_locked_inner(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        mut linked: Linked,
        remember: bool,
    ) -> Result<(SyncReport, ChangeSeq)> {
        if tree::conflicted(&linked.root).await? {
            info!(activity = "shared.sync", project_id = log_id(project_id), result = "conflicted"; "The repo has conflicts; nothing synced");
            return Ok((
                SyncReport {
                    conflicted: true,
                    ..SyncReport::default()
                },
                self.change_seq(),
            ));
        }
        self.ensure_repo(project_id, &mut linked).await?;
        let plink = linked.link();
        let repo_id = plink.repo_id.clone();
        let bounds = ScanBounds {
            clear_temp_files: true,
            ..ScanBounds::default()
        };
        let scan = tree::scan(&linked.root, &plink.root(), bounds).await?;
        // Probe fix 7: past the scan's bounds the whole project is skipped.
        let whole_skip = whole_project_skip(&scan, &plink.root());
        let now = now(core)?;
        let limits = Limits {
            library: core.library_limits(),
            state: core.state_limits(),
        };

        // Probe fix 4: the rows are read and the plan made outside the
        // storage's write lock (on a large project that is most of a
        // sync), then a short transaction checks that nothing the plan
        // relied on changed meanwhile ([`PlanStamp`]) and writes it. A
        // changed row means planning again; after `OPTIMISTIC_PLANS`
        // tries the last plan is made inside the transaction, as before.
        let mut attempt = 0;
        let (tx, plan, touched) = loop {
            attempt += 1;
            let fallback = attempt > OPTIMISTIC_PLANS;
            let (mut tx, plan) = if fallback {
                let mut tx = self.storage().write().await?;
                let rows = shared_rows_in(&mut tx, project_id).await?;
                let plan = plan_sync(&plink, &scan, &rows.rows, &limits, &mut CoreIds);
                (tx, plan)
            } else {
                let rows = shared_rows_pool(self, project_id).await?;
                let plan = plan_sync(&plink, &scan, &rows.rows, &limits, &mut CoreIds);
                let stamp = PlanStamp::of(&rows, &plan);
                drop(rows);
                if let Some(hook) = &core.sync_plan_hook {
                    hook().await;
                }
                let mut tx = self.storage().write().await?;
                if !stamp.still_holds(&mut tx, project_id).await? {
                    debug!(activity = "shared.sync", project_id = log_id(project_id), attempt = attempt; "Rows changed while planning; planning again");
                    continue;
                }
                (tx, plan)
            };
            match self
                .write_plan(&mut tx, &plan, project_id, &repo_id, &linked, &now, &limits)
                .await
            {
                Ok(touched) => break (tx, plan, touched),
                // A refusal on stale names or counts: plan again.
                Err(e) if !fallback => {
                    debug!(activity = "shared.sync", project_id = log_id(project_id), code = e.code.as_str(), attempt = attempt; "A planned write was refused; planning again");
                    drop(tx);
                    continue;
                }
                Err(e) => return Err(e),
            }
        };
        let mut pending: Vec<Pending<'_>> = Vec::new();
        for (kind, ids) in &touched.rows {
            pending.push(Pending {
                ticket: self.take_seq(),
                kind: *kind,
                scope: Some(project_id.to_string()),
                ids: ids.iter().cloned().collect(),
            });
        }
        if touched.order {
            pending.push(Pending {
                ticket: self.take_seq(),
                kind: StoredKind::Project,
                scope: None,
                ids: vec![project_id.to_string()],
            });
        }
        tx.commit().await?;
        // Announced now (review I1): the rows are committed, so their
        // events must not wait on the files (a failure there would lose
        // them, and the published sequence would be held meanwhile).
        let mut seq = self.change_seq();
        for p in pending.drain(..) {
            seq = self.announce(p.ticket, p.kind, p.scope, Some(p.ids), origin);
        }

        // The files, each on its own; never a delete or a rename.
        let writes: Vec<FileOp> = plan
            .files
            .iter()
            .filter(|f| matches!(f, FileOp::Write { .. }))
            .cloned()
            .collect();
        let mut done: HashSet<String> = HashSet::new();
        let mut files_written = 0u32;
        if !writes.is_empty() {
            let outcomes = tree::apply(
                &linked.root,
                writes.clone(),
                ApplyOptions {
                    stop_at_first_failure: false,
                    hook: core.file_hook(),
                },
            )
            .await?;
            for (op, outcome) in writes.iter().zip(&outcomes) {
                let FileOp::Write { rel_path, .. } = op else {
                    continue;
                };
                match outcome {
                    OpOutcome::Done => {
                        files_written += 1;
                        done.insert(rel_path.clone());
                    }
                    other => {
                        warn!(activity = "shared.sync", project_id = log_id(project_id), outcome = outcome_kind(other); "A file write didn't happen");
                    }
                }
            }
        }
        // The links that waited for their write.
        let waiting: Vec<&LinkUpdate> = plan
            .links
            .iter()
            .filter(|u| u.pending_on.as_ref().is_some_and(|p| done.contains(p)))
            .collect();
        if !waiting.is_empty() || files_written > 0 {
            let mut tx = self.storage().write().await?;
            let mut late = Touched::default();
            for u in &waiting {
                store_link(&mut tx, &repo_id, u).await?;
                // Re-review R2: announced even when the first transaction
                // already announced the kind, since its readers ran before
                // this link (a row's new `sharedPath`) was stored.
                late.add(u.kind, &u.id);
            }
            for (kind, ids) in &late.rows {
                pending.push(Pending {
                    ticket: self.take_seq(),
                    kind: *kind,
                    scope: Some(project_id.to_string()),
                    ids: ids.iter().cloned().collect(),
                });
            }
            if files_written > 0 {
                pending.push(Pending {
                    ticket: self.take_seq(),
                    kind: StoredKind::SharedRepo,
                    scope: None,
                    ids: vec![repo_id.clone()],
                });
            }
            tx.commit().await?;
        }

        for p in pending {
            seq = self.announce(p.ticket, p.kind, p.scope, Some(p.ids), origin);
        }
        let notices = if remember && whole_skip.is_none() {
            self.notices
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .filter(plan.notices)
        } else {
            // A project skipped whole is named on every sync (probe fix
            // 7): its one notice is all the plan has.
            plan.notices
        };
        info!(
            activity = "shared.sync",
            project_id = log_id(project_id),
            rows = plan.rows.len(),
            files = files_written,
            notices = notices.len(),
            skipped = scan.skipped.len();
            "Synced a project"
        );
        Ok((
            SyncReport {
                conflicted: false,
                rows_changed: u32::try_from(plan.rows.len()).unwrap_or(u32::MAX),
                files_written,
                notices,
                failures: Vec::new(),
                skipped_projects: whole_skip
                    .map(|why| SkippedProject {
                        project_id: project_id.to_string(),
                        why,
                    })
                    .into_iter()
                    .collect(),
            },
            seq,
        ))
    }

    /// A plan's row writes, the links that wait for no file, the order
    /// append and the project's directory, inside `tx`: what a sync commits
    /// before its files.
    #[allow(clippy::too_many_arguments)]
    async fn write_plan(
        &self,
        tx: &mut WriteTx,
        plan: &seaquel_workspace::shared::SyncPlan,
        project_id: &str,
        repo_id: &str,
        linked: &Linked,
        now: &str,
        limits: &Limits,
    ) -> Result<Touched> {
        let mut touched = Touched::default();
        let mut order: Option<Vec<String>> = None;
        for op in &plan.rows {
            self.apply_row_op(tx, op, project_id, now, limits, &mut order)
                .await?;
            let (kind, id) = op_target(op);
            touched.add(kind, id);
        }
        for u in plan.links.iter().filter(|u| u.pending_on.is_none()) {
            store_link(tx, repo_id, u).await?;
            touched.add(u.kind, &u.id);
        }
        if let Some(order) = &order {
            project_state::set_connection_order(tx, project_id, order).await?;
            touched.order = true;
        }
        if !linked.dir_stored {
            projects::set_shared_dir(tx, project_id, Some(&linked.dir)).await?;
        }
        Ok(touched)
    }

    /// One planned row write, as the library writes it (Decision 34: the
    /// file's content with a version, unsharing, a template's connection
    /// appended to the order).
    async fn apply_row_op(
        &self,
        tx: &mut WriteTx,
        op: &RowOp,
        project_id: &str,
        now: &str,
        limits: &Limits,
        order: &mut Option<Vec<String>>,
    ) -> Result<()> {
        // Boxed (re-review R1): each row op inlines the library's write.
        Box::pin(self.apply_row_op_inner(tx, op, project_id, now, limits, order)).await
    }

    /// [`Workspace::apply_row_op`], unboxed.
    async fn apply_row_op_inner(
        &self,
        tx: &mut WriteTx,
        op: &RowOp,
        project_id: &str,
        now: &str,
        limits: &Limits,
        order: &mut Option<Vec<String>>,
    ) -> Result<()> {
        match op {
            RowOp::CreateQuery { id, draft } => {
                let row = lib::saved_query_from_draft(id.clone(), draft, now);
                lib::check_saved_query(&row, &limits.library)?;
                insert_saved_query_in(tx, &row, &limits.library).await?;
            }
            RowOp::UpdateQuery { id, patch } => {
                update_saved_query_in(tx, id, patch, now, &limits.library, |_| Ok(())).await?;
            }
            RowOp::CreateDashboard { id, draft } => {
                let row = seaquel_workspace::state::dashboard_from_draft(id.clone(), draft, now);
                insert_dashboard_in(tx, row, draft.rename_if_taken, &limits.state).await?;
            }
            RowOp::UpdateDashboard { id, patch } => {
                update_dashboard_in(tx, id, patch, now, &limits.state).await?;
            }
            RowOp::CreateConnection { id, draft } => {
                if order.is_none() {
                    *order = Some(connection_order_in(tx, project_id).await?);
                }
                let mut row = lib::connection_from_draft(id.clone(), draft, now);
                insert_connection_in(tx, &mut row, draft.rename_if_taken, &limits.library).await?;
                // Q31: the repo brought it, so an unlink asks before removing it.
                connections::set_origin(tx, id, Some(connections::ORIGIN_IMPORTED)).await?;
                if let Some(order) = order.as_mut() {
                    order.push(id.clone());
                }
            }
            RowOp::UpdateConnection { id, patch } => {
                update_connection_in(tx, id, patch, now, &SecretChanges::default()).await?;
            }
            RowOp::Unshare { kind, id } => match kind {
                Kind::SavedQuery => {
                    let patch = SavedQueryPatch {
                        shared: Some(false),
                        ..Default::default()
                    };
                    update_saved_query_in(tx, id, &patch, now, &limits.library, |_| Ok(())).await?;
                }
                Kind::Dashboard => {
                    let patch = DashboardPatch {
                        shared: Some(false),
                        ..Default::default()
                    };
                    update_dashboard_in(tx, id, &patch, now, &limits.state).await?;
                }
                Kind::Connection => {
                    let patch = ConnectionPatch {
                        is_local_only: Some(true),
                        ..Default::default()
                    };
                    update_connection_in(tx, id, &patch, now, &SecretChanges::default()).await?;
                }
            },
        }
        Ok(())
    }
}

fn op_target(op: &RowOp) -> (Kind, &str) {
    match op {
        RowOp::CreateQuery { id, .. } | RowOp::UpdateQuery { id, .. } => (Kind::SavedQuery, id),
        RowOp::CreateDashboard { id, .. } | RowOp::UpdateDashboard { id, .. } => {
            (Kind::Dashboard, id)
        }
        RowOp::CreateConnection { id, .. } | RowOp::UpdateConnection { id, .. } => {
            (Kind::Connection, id)
        }
        RowOp::Unshare { kind, id } => (*kind, id),
    }
}

fn outcome_kind(o: &OpOutcome) -> &'static str {
    match o {
        OpOutcome::Done => "done",
        OpOutcome::Deleted(_) => "deleted",
        OpOutcome::Stale => "stale",
        OpOutcome::Refused(_) => "refused",
        OpOutcome::Failed(_) => "failed",
        OpOutcome::NotRun => "notRun",
    }
}

/// An id for a log line: one past 128 bytes is left out.
fn log_id(id: &str) -> &str {
    if id.len() <= 128 {
        id
    } else {
        "<long>"
    }
}

/// How many times a sync plans outside the write lock before it plans
/// inside it (probe fix 4).
const OPTIMISTIC_PLANS: usize = 2;

/// Why the scan skipped the project directory `root` whole (past its file
/// count or total size), if it did.
fn whole_project_skip(scan: &DirScan, root: &str) -> Option<SkipReason> {
    match scan.skipped.as_slice() {
        [only]
            if scan.files.is_empty()
                && only.rel_path == root
                && matches!(only.why, SkipReason::TooMany | SkipReason::TooLarge) =>
        {
            Some(only.why)
        }
        _ => None,
    }
}

/// Every row of the project with its link (the planner checks names
/// against all of them), and the raw link columns they were read with.
struct RowsRead {
    rows: SharedRows,
    /// Each kind's `(id, link)` rows: every row of the project, linked or
    /// not, so a row added or removed changes them.
    links: [Vec<seaquel_storage::RowLink>; 3],
}

/// Where [`read_rows`] reads.
enum RowSrc<'a> {
    Pool(&'a seaquel_storage::Storage),
    Tx(&'a mut WriteTx),
}

impl RowSrc<'_> {
    fn r(&mut self) -> seaquel_storage::Reader<'_> {
        match self {
            RowSrc::Pool(st) => seaquel_storage::Reader::Pool(st),
            RowSrc::Tx(tx) => seaquel_storage::Reader::Tx(tx),
        }
    }
}

async fn read_rows(mut src: RowSrc<'_>, project_id: &str) -> Result<RowsRead> {
    let by_id = |links: &[seaquel_storage::RowLink]| -> HashMap<String, SharedLink> {
        links
            .iter()
            .map(|r| (r.id.clone(), r.link.clone()))
            .collect()
    };
    let qraw = saved_queries::links(src.r(), project_id).await?;
    let queries: Vec<PersistedSavedQuery> = saved_queries::list(src.r(), project_id).await?;
    let draw = dashboards::links(src.r(), project_id).await?;
    let boards: Vec<PersistedDashboard> = dashboards::list(src.r(), project_id).await?;
    let craw = connections::links(src.r(), project_id).await?;
    let conns: Vec<PersistedConnection> = connections::list_in_project(src.r(), project_id).await?;
    let (mut qlinks, mut dlinks, mut clinks) = (by_id(&qraw), by_id(&draw), by_id(&craw));
    Ok(RowsRead {
        rows: SharedRows {
            project_id: project_id.to_string(),
            queries: queries
                .into_iter()
                .map(|row| LinkedQuery {
                    link: shared_link(qlinks.remove(&row.id)),
                    row,
                })
                .collect(),
            dashboards: boards
                .into_iter()
                .map(|row| LinkedDashboard {
                    link: shared_link(dlinks.remove(&row.id)),
                    row,
                })
                .collect(),
            connections: conns
                .into_iter()
                .map(|row| LinkedConnection {
                    link: connection_link(clinks.remove(&row.id)),
                    row,
                })
                .collect(),
        },
        links: [qraw, draw, craw],
    })
}

/// [`read_rows`] inside the transaction (the fallback plan).
async fn shared_rows_in(tx: &mut WriteTx, project_id: &str) -> Result<RowsRead> {
    read_rows(RowSrc::Tx(tx), project_id).await
}

/// [`read_rows`] from the pool, outside any transaction.
async fn shared_rows_pool(ws: &Workspace, project_id: &str) -> Result<RowsRead> {
    read_rows(RowSrc::Pool(ws.storage()), project_id).await
}

/// What a plan made outside the write lock relied on (probe fix 4): every
/// row's link columns (so a row added, removed or relinked shows), and each
/// row the plan changes, whole. Per-row stamps rather than the workspace's
/// change sequence, which counts every write (a setting, another project's
/// query) and would send a busy session's every sync to the fallback.
/// Names of rows the plan doesn't touch aren't stamped: a create or rename
/// that runs into one is refused by the library's own check, and the sync
/// plans again.
///
/// What it compares is the `Persisted*` row JSON plus the link columns
/// `RowLink` reads (review A4). `connections.shared_origin` is in neither:
/// harmless today, since only a write under the repo lock (link, publish)
/// or an unlink changes it, and a sync holds that lock. A column added to
/// the rows later must be added here if a plan reads it.
struct PlanStamp {
    links: [Vec<seaquel_storage::RowLink>; 3],
    rows: Vec<(Kind, String, String)>,
}

/// A row as JSON text, compared whole by [`PlanStamp`].
macro_rules! row_json {
    ($row:expr) => {
        serde_json::to_string($row).unwrap_or_default()
    };
}

impl PlanStamp {
    fn of(read: &RowsRead, plan: &seaquel_workspace::shared::SyncPlan) -> Self {
        let mut ids: HashSet<(Kind, String)> = HashSet::new();
        for op in &plan.rows {
            let (kind, id) = op_target(op);
            ids.insert((kind, id.to_string()));
        }
        for u in &plan.links {
            ids.insert((u.kind, u.id.clone()));
        }
        let mut rows = Vec::new();
        for q in &read.rows.queries {
            if ids.contains(&(Kind::SavedQuery, q.row.id.clone())) {
                rows.push((Kind::SavedQuery, q.row.id.clone(), row_json!(&q.row)));
            }
        }
        for d in &read.rows.dashboards {
            if ids.contains(&(Kind::Dashboard, d.row.id.clone())) {
                rows.push((Kind::Dashboard, d.row.id.clone(), row_json!(&d.row)));
            }
        }
        for c in &read.rows.connections {
            if ids.contains(&(Kind::Connection, c.row.id.clone())) {
                rows.push((Kind::Connection, c.row.id.clone(), row_json!(&c.row)));
            }
        }
        PlanStamp {
            links: read.links.clone(),
            rows,
        }
    }

    /// Whether the rows read inside `tx` are still what the plan saw.
    async fn still_holds(&self, tx: &mut WriteTx, project_id: &str) -> Result<bool> {
        if saved_queries::links(&mut *tx, project_id).await? != self.links[0]
            || dashboards::links(&mut *tx, project_id).await? != self.links[1]
            || connections::links(&mut *tx, project_id).await? != self.links[2]
        {
            return Ok(false);
        }
        for (kind, id, json) in &self.rows {
            let now = match kind {
                Kind::SavedQuery => saved_queries::get(&mut *tx, id)
                    .await?
                    .map(|r| row_json!(&r)),
                Kind::Dashboard => dashboards::get(&mut *tx, id).await?.map(|r| row_json!(&r)),
                Kind::Connection => connections::get(&mut *tx, id).await?.map(|r| row_json!(&r)),
            };
            if now.as_deref() != Some(json.as_str()) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

// ── Publish (Decision 36) and removals (Decision 37) ──

/// A removal's file already deleted, waiting for the row write
/// ([`Workspace::unpublish_begin`]). It holds the repo lock until
/// [`Workspace::unpublish_end`].
pub(crate) struct Unpublish {
    _lock: RepoLock,
    linked: Linked,
    project_id: String,
    kind: Kind,
    id: String,
    rel: String,
    outcome: OpOutcome,
    /// The row stays (unshared): its link is cleared after the write.
    keeps_row: bool,
}

impl Workspace {
    /// The row `id`'s project, read from the pool.
    async fn project_of(&self, kind: Kind, id: &str) -> Result<Option<String>> {
        Ok(match kind {
            Kind::SavedQuery => saved_queries::get(self.storage(), id)
                .await?
                .map(|r| r.project_id),
            Kind::Dashboard => dashboards::get(self.storage(), id)
                .await?
                .map(|r| r.project_id),
            Kind::Connection => connections::get(self.storage(), id)
                .await?
                .map(|r| r.project_id),
        })
    }

    /// The row's link, kept only when it points into this project's own
    /// directory of this repo: a stored path elsewhere is never written or
    /// deleted (bug 5, bug 7's damage).
    async fn own_link(&self, linked: &Linked, kind: Kind, id: &str) -> Result<Link> {
        let plink = linked.link();
        let dir = format!("{}/", plink.kind_dir(kind));
        let link = match kind {
            Kind::SavedQuery => shared_link(saved_queries::link(self.storage(), id).await?),
            Kind::Dashboard => shared_link(dashboards::link(self.storage(), id).await?),
            Kind::Connection => {
                let raw = connections::link(self.storage(), id).await?;
                let repo_ok = raw
                    .as_ref()
                    .and_then(|l| l.path.as_deref())
                    .is_some_and(|p| p.starts_with(&format!("{}:", plink.repo_id)));
                let link = connection_link(raw);
                if link.path.is_some() && !repo_ok {
                    return Ok(Link::default());
                }
                link
            }
        };
        match &link.path {
            Some(p) if !p.starts_with(&dir) => Ok(Link::default()),
            _ => Ok(link),
        }
    }

    /// Publishes the row a library call just committed (Decision 36):
    /// `None` when there's nothing to write (no link, not shared, nothing
    /// that changes the file, no `LocalFiles`).
    pub(crate) async fn publish_row(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        what: Publish<'_>,
    ) -> Option<PublishOutcome> {
        // Boxed (review C1): inlined, these futures made each library
        // call's state machine deep enough to overflow a 2 MiB stack in a
        // debug build.
        Box::pin(self.publish_row_inner(core, origin, project_id, what)).await
    }

    /// [`Workspace::publish_row`], unboxed.
    async fn publish_row_inner(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        what: Publish<'_>,
    ) -> Option<PublishOutcome> {
        core.local_files()?;
        let locked = match self.lock_linked(core, project_id).await {
            Ok(Some(locked)) => locked,
            Ok(None) => return None,
            Err(e) => {
                warn!(activity = "shared.publish", project_id = log_id(project_id), code = e.code.as_str(); "Publishing failed");
                return Some(failed(&e.code, e.message));
            }
        };
        let (_lock, linked) = locked;
        match self
            .publish_locked(core, origin, project_id, linked, what)
            .await
        {
            Ok(outcome) => outcome,
            Err(e) => {
                warn!(activity = "shared.publish", project_id = log_id(project_id), code = e.code.as_str(); "Publishing failed");
                Some(failed(&e.code, e.message))
            }
        }
    }

    /// The publish, with the repo lock held by the caller.
    async fn publish_locked(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        linked: Linked,
        what: Publish<'_>,
    ) -> Result<Option<PublishOutcome>> {
        // Boxed (review C1): inlined, these futures made each library
        // call's state machine deep enough to overflow a 2 MiB stack in a
        // debug build.
        Box::pin(self.publish_locked_inner(core, origin, project_id, linked, what)).await
    }

    /// [`Workspace::publish_locked`], unboxed.
    async fn publish_locked_inner(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        mut linked: Linked,
        what: Publish<'_>,
    ) -> Result<Option<PublishOutcome>> {
        self.ensure_repo(project_id, &mut linked).await?;
        let plink = linked.link();
        // Q31: sharing a connection from here (the link dialog's ticks, the
        // local-only switch) records it as the user's own.
        let export = matches!(
            what,
            Publish::Connection {
                shared_now: true,
                ..
            }
        );
        // The row as it is now, under the lock.
        enum Row {
            Query(PersistedSavedQuery),
            Dashboard(PersistedDashboard),
            Connection(PersistedConnection),
            Project(String),
        }
        let (row, link, kind) = match what {
            Publish::Query { id, .. } => {
                let Some(r) = saved_queries::get(self.storage(), id).await? else {
                    return Ok(None);
                };
                let link = self.own_link(&linked, Kind::SavedQuery, id).await?;
                (Row::Query(r), link, Some(Kind::SavedQuery))
            }
            Publish::Dashboard { id, .. } => {
                let Some(r) = dashboards::get(self.storage(), id).await? else {
                    return Ok(None);
                };
                let link = self.own_link(&linked, Kind::Dashboard, id).await?;
                (Row::Dashboard(r), link, Some(Kind::Dashboard))
            }
            Publish::Connection { id, .. } => {
                let Some(r) = connections::get(self.storage(), id).await? else {
                    return Ok(None);
                };
                let link = self.own_link(&linked, Kind::Connection, id).await?;
                (Row::Connection(r), link, Some(Kind::Connection))
            }
            Publish::Project => {
                let Some(p) = projects::get(self.storage(), project_id).await? else {
                    return Ok(None);
                };
                (Row::Project(p.name), Link::default(), None)
            }
        };
        let in_project = match &row {
            Row::Query(r) => r.project_id == project_id,
            Row::Dashboard(r) => r.project_id == project_id,
            Row::Connection(r) => r.project_id == project_id,
            Row::Project(_) => true,
        };
        if !in_project {
            return Ok(None);
        }
        let existing_path = match &row {
            Row::Project(_) => Some(plink.project_yaml()),
            _ => link.path.clone(),
        };
        let existing = match &existing_path {
            Some(p) => tree::read_file(&linked.root, p).await.ok().flatten(),
            None => None,
        };
        let taken = TakenPaths::from_paths(
            tree::paths_under(&linked.root, &plink.root())
                .await
                .unwrap_or_default(),
        );
        // Planned in a block of its own: the context's `&dyn Fn` must not
        // live across an await, or the library calls' futures aren't
        // `Send`.
        let planned = {
            let is_taken = |p: &str| taken.contains(p);
            let ctx = PublishContext {
                taken: &is_taken,
                existing: existing.as_deref(),
            };
            let change = match (&row, what) {
                (Row::Query(r), Publish::Query { renamed, .. }) => RowChange::Query {
                    row: Some(r),
                    link: &link,
                    renamed,
                },
                (Row::Dashboard(r), Publish::Dashboard { renamed, .. }) => RowChange::Dashboard {
                    row: Some(r),
                    link: &link,
                    renamed,
                },
                (
                    Row::Connection(r),
                    Publish::Connection {
                        renamed,
                        shared_now,
                        ..
                    },
                ) => RowChange::Connection {
                    row: Some(r),
                    link: &link,
                    renamed,
                    shared_now,
                },
                (Row::Project(name), _) => RowChange::Project { name },
                _ => return Ok(None),
            };
            plan_publish(&plink, &change, &ctx, &mut CoreIds)
        };
        let plan = match planned {
            Ok(plan) => plan,
            Err(e) => return Ok(Some(failed(&e.code, e.message))),
        };
        if plan.files.is_empty() {
            return Ok(None);
        }
        let deletes = plan
            .files
            .iter()
            .all(|f| matches!(f, FileOp::Delete { .. }));
        let outcomes = tree::apply(
            &linked.root,
            plan.files.clone(),
            ApplyOptions {
                stop_at_first_failure: true,
                hook: core.file_hook(),
            },
        )
        .await?;
        let first_bad = outcomes.iter().find(|o| !o.is_success()).cloned();
        let repo_id = plink.repo_id.clone();
        let kind_label = kind.map_or("project", row_kind);
        let outcome = match first_bad {
            None => {
                self.store_publish_link(origin, &repo_id, plan.on_success.as_ref(), true, export)
                    .await?;
                debug!(activity = "shared.publish", project_id = log_id(project_id), kind = kind_label, files = outcomes.len(); "Published a file");
                status(if deletes {
                    PublishStatus::Deleted
                } else {
                    PublishStatus::Written
                })
            }
            Some(OpOutcome::Stale) => {
                // M1: a teammate's change is on disk. Nothing was written;
                // the project syncs, and the file wins (Q20).
                info!(activity = "shared.publish", project_id = log_id(project_id), kind = kind_label, result = "stale"; "The file changed in the repo; syncing instead");
                // Review M3: a rename's new file, already written, is taken
                // back, so no two files carry the file's one id; the old
                // file (the teammate's change) stays and the sync pairs it.
                let undo: Vec<FileOp> = plan
                    .files
                    .iter()
                    .zip(&outcomes)
                    .filter_map(|(op, outcome)| match (op, outcome) {
                        (FileOp::Write { rel_path, text, .. }, OpOutcome::Done) => {
                            Some(FileOp::Delete {
                                rel_path: rel_path.clone(),
                                expect_hash: seaquel_workspace::shared::file_hash(rel_path, text)
                                    .map(|(h, _)| h),
                            })
                        }
                        _ => None,
                    })
                    .collect();
                let mut undone = true;
                if !undo.is_empty() {
                    let out = tree::apply(
                        &linked.root,
                        undo,
                        ApplyOptions {
                            stop_at_first_failure: false,
                            hook: core.file_hook(),
                        },
                    )
                    .await?;
                    undone = out.iter().all(OpOutcome::is_success);
                    if !undone {
                        warn!(activity = "shared.publish", project_id = log_id(project_id), kind = kind_label; "Taking back a renamed file failed");
                    }
                }
                self.store_publish_link(origin, &repo_id, None, !undone, false)
                    .await?;
                let linked = self.linked(project_id).await?.ok_or_else(not_linked)?;
                self.sync_locked(core, origin, project_id, linked, false)
                    .await?;
                failed(
                    FILE_CHANGED,
                    "The shared file changed in the repository, so it wasn't overwritten; \
                     the repository's version is shown, and yours is in its history.",
                )
            }
            Some(OpOutcome::Failed(message)) => {
                // A failed write: a first share keeps its path with no base,
                // so the next sync writes the file.
                self.store_publish_link(
                    origin,
                    &repo_id,
                    plan.on_failure.as_ref(),
                    outcomes.iter().any(OpOutcome::is_success),
                    export,
                )
                .await?;
                warn!(activity = "shared.publish", project_id = log_id(project_id), kind = kind_label, code = FILE_ERROR; "A file write failed");
                failed(FILE_ERROR, message)
            }
            Some(OpOutcome::Refused(message)) => {
                // A refusal (a symlink on the path) stores nothing.
                warn!(activity = "shared.publish", project_id = log_id(project_id), kind = kind_label, code = FILE_ERROR, result = "refused"; "A file write was refused");
                failed(FILE_ERROR, message)
            }
            Some(_) => failed(FILE_ERROR, "The shared file couldn't be written."),
        };
        Ok(Some(outcome))
    }

    /// Stores a publish's link (if any) and announces `sharedRepo` when a
    /// link or a file changed. `export`: a connection shared from here, whose
    /// stored link records it as the user's own (Q31).
    async fn store_publish_link(
        &self,
        origin: &WriteOrigin,
        repo_id: &str,
        link: Option<&LinkUpdate>,
        wrote_file: bool,
        export: bool,
    ) -> Result<()> {
        if link.is_none() && !wrote_file {
            return Ok(());
        }
        let ticket = match link {
            Some(u) => {
                let mut tx = self.storage().write().await?;
                let ticket = self.take_seq();
                store_link(&mut tx, repo_id, u).await?;
                if export && u.kind == Kind::Connection && u.link.path.is_some() {
                    connections::set_origin(&mut tx, &u.id, Some(connections::ORIGIN_EXPORTED))
                        .await?;
                }
                tx.commit().await?;
                ticket
            }
            None => self.take_seq(),
        };
        self.announce(
            ticket,
            StoredKind::SharedRepo,
            None,
            Some(vec![repo_id.to_string()]),
            origin,
        );
        Ok(())
    }

    /// Decision 37's first half: before unsharing or removing the row
    /// `id`, take its repo's lock and delete its file, keeping the bytes.
    /// `None` when there's nothing to delete (no link, no `LocalFiles`).
    pub(crate) async fn unpublish_begin(
        &self,
        core: &Core,
        kind: Kind,
        id: &str,
        keeps_row: bool,
    ) -> Result<Option<Unpublish>> {
        // Boxed (review C1): inlined, these futures made each library
        // call's state machine deep enough to overflow a 2 MiB stack in a
        // debug build.
        Box::pin(self.unpublish_begin_inner(core, kind, id, keeps_row)).await
    }

    /// Whether the row `id` has a stored link (a path), read from the pool.
    async fn has_link(&self, kind: Kind, id: &str) -> bool {
        let link = match kind {
            Kind::SavedQuery => saved_queries::link(self.storage(), id).await,
            Kind::Dashboard => dashboards::link(self.storage(), id).await,
            Kind::Connection => connections::link(self.storage(), id).await,
        };
        link.ok().flatten().is_some_and(|l| l.path.is_some())
    }

    /// [`Workspace::unpublish_begin`], unboxed. Review I2 (decision): a row
    /// with a link whose file can't be deleted, or whose project's link
    /// can't be read, isn't removed: `FILE_ERROR`, before the row write. A
    /// teammate's change in the file (stale) goes on; the sync after the
    /// row write keeps it.
    async fn unpublish_begin_inner(
        &self,
        core: &Core,
        kind: Kind,
        id: &str,
        keeps_row: bool,
    ) -> Result<Option<Unpublish>> {
        if core.local_files().is_none() {
            return Ok(None);
        }
        let Some(project_id) = self.project_of(kind, id).await? else {
            return Ok(None);
        };
        let refuse = |message: String| CoreError::new(FILE_ERROR, message);
        let (lock, linked) = match self.lock_linked(core, &project_id).await {
            Ok(Some(locked)) => locked,
            Ok(None) => return Ok(None),
            Err(e) => {
                warn!(activity = "shared.unpublish", project_id = log_id(&project_id), code = e.code.as_str(); "Reading the project's link failed");
                if self.has_link(kind, id).await {
                    return Err(refuse(format!(
                        "The shared file couldn't be removed, so nothing was changed: {}",
                        e.message
                    )));
                }
                return Ok(None);
            }
        };
        let link = self.own_link(&linked, kind, id).await?;
        let Some(rel) = link.path.clone() else {
            return Ok(None);
        };
        let existing = tree::read_file(&linked.root, &rel).await.ok().flatten();
        // A block of its own: no `&dyn Fn` across an await (`Send`).
        let plan = {
            let none = |_: &str| false;
            let ctx = PublishContext {
                taken: &none,
                existing: existing.as_deref(),
            };
            let change = match kind {
                Kind::SavedQuery => RowChange::Query {
                    row: None,
                    link: &link,
                    renamed: false,
                },
                Kind::Dashboard => RowChange::Dashboard {
                    row: None,
                    link: &link,
                    renamed: false,
                },
                Kind::Connection => RowChange::Connection {
                    row: None,
                    link: &link,
                    renamed: false,
                    shared_now: false,
                },
            };
            plan_publish(&linked.link(), &change, &ctx, &mut CoreIds)
                .map_err(|e| refuse(e.message))?
        };
        let Some(op) = plan.files.into_iter().next() else {
            return Ok(None);
        };
        let outcome = match tree::apply(
            &linked.root,
            vec![op],
            ApplyOptions {
                stop_at_first_failure: true,
                hook: core.file_hook(),
            },
        )
        .await
        {
            Ok(mut out) => out.pop().unwrap_or(OpOutcome::NotRun),
            Err(e) => OpOutcome::Failed(e.message),
        };
        if let OpOutcome::Failed(m) | OpOutcome::Refused(m) = &outcome {
            warn!(activity = "shared.unpublish", project_id = log_id(&project_id), kind = row_kind(kind), code = FILE_ERROR; "A shared file couldn't be deleted; nothing removed");
            return Err(refuse(format!(
                "The shared file couldn't be removed, so nothing was changed: {m}"
            )));
        }
        Ok(Some(Unpublish {
            _lock: lock,
            linked,
            project_id,
            kind,
            id: id.to_string(),
            rel,
            outcome,
            keeps_row,
        }))
    }

    /// Decision 37's second half, after the row write: put the file back if
    /// the write failed; otherwise clear an unshared row's link (and on a
    /// teammate's change, sync instead). Releases the repo lock.
    pub(crate) async fn unpublish_end(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        pending: Option<Unpublish>,
        written: bool,
    ) -> Option<PublishOutcome> {
        // Boxed (review C1): inlined, these futures made each library
        // call's state machine deep enough to overflow a 2 MiB stack in a
        // debug build.
        Box::pin(self.unpublish_end_inner(core, origin, pending, written)).await
    }

    /// [`Workspace::unpublish_end`], unboxed.
    async fn unpublish_end_inner(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        pending: Option<Unpublish>,
        written: bool,
    ) -> Option<PublishOutcome> {
        let p = pending?;
        let kind_label = row_kind(p.kind);
        if !written {
            if let OpOutcome::Deleted(bytes) = &p.outcome {
                if !bytes.is_empty() {
                    if let Err(e) = tree::restore(&p.linked.root, &p.rel, bytes.clone()).await {
                        warn!(activity = "shared.unpublish", project_id = log_id(&p.project_id), kind = kind_label, code = e.code.as_str(); "Putting a file back failed");
                    }
                }
            }
            return None;
        }
        let repo_id = p.linked.repo_id.clone().unwrap_or_default();
        let clear = p.keeps_row.then(|| LinkUpdate {
            kind: p.kind,
            id: p.id.clone(),
            link: Link::default(),
            pending_on: None,
        });
        let result = async {
            Ok::<_, CoreError>(Some(match &p.outcome {
                OpOutcome::Deleted(_) => {
                    self.store_publish_link(origin, &repo_id, clear.as_ref(), true, false)
                        .await?;
                    status(PublishStatus::Deleted)
                }
                OpOutcome::Stale => {
                    self.store_publish_link(origin, &repo_id, clear.as_ref(), false, false)
                        .await?;
                    let linked = self.linked(&p.project_id).await?.ok_or_else(not_linked)?;
                    self.sync_locked(core, origin, &p.project_id, linked, false)
                        .await?;
                    failed(
                        FILE_CHANGED,
                        "The shared file changed in the repository, so it wasn't deleted.",
                    )
                }
                OpOutcome::Failed(m) | OpOutcome::Refused(m) => failed(FILE_ERROR, m.clone()),
                OpOutcome::Done | OpOutcome::NotRun => return Ok(None),
            }))
        }
        .await;
        match result {
            Ok(outcome) => outcome,
            Err(e) => {
                warn!(activity = "shared.unpublish", project_id = log_id(&p.project_id), kind = kind_label, code = e.code.as_str(); "Finishing a removal's file failed");
                Some(failed(&e.code, e.message))
            }
        }
    }
}

// ── Link, unlink, import ──

impl Workspace {
    /// Links a project to the repo at `path` (Decision 40): registers the
    /// repo (or reuses it), picks the directory (one whose `project.yaml`
    /// names the project, else a free one), writes `project.yaml` if
    /// missing, exports the connections `share` names as templates and
    /// links each to its template (Q30), stores the directory, and syncs.
    ///
    /// Errors: `NOT_SUPPORTED`, `PROJECT_NOT_FOUND`, `INVALID_ARGUMENT` (a
    /// `share` id that isn't one of the project's connections),
    /// `FILE_ERROR` (no folder at `path`), `REPO_CONFLICTED`, the storage
    /// codes.
    pub async fn shared_link_project(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        path: &str,
        share: &[String],
    ) -> Result<Seqd<SyncReport>> {
        core.require_local_files()?;
        debug!(activity = "shared.linkProject", project_id = log_id(project_id), share = share.len(); "Link a project");
        let path = repo_path_key(path);
        let project = projects::get(self.storage(), project_id)
            .await?
            .ok_or_else(project_not_found)?;
        let mine: HashSet<String> = connections::ids_in_project(self.storage(), project_id)
            .await?
            .into_iter()
            .collect();
        if share.iter().any(|id| !mine.contains(id)) {
            return Err(invalid(
                "A connection to share isn't one of the project's connections.",
            ));
        }
        // Review I4 (decision): another repo needs an unlink first; the
        // same repo again only shares the connections ticked now.
        if project
            .git_repo_path
            .as_deref()
            .is_some_and(|p| repo_path_key(p) != path)
        {
            return Err(already_linked());
        }
        let root = PathBuf::from(&path);
        if !tree::is_dir(&root).await {
            return Err(CoreError::new(
                FILE_ERROR,
                "The repository's folder isn't there.",
            ));
        }
        let _lock = core.repo_lock(&root).await;
        // Re-review R5: read again under the lock; the project may have been
        // linked, unlinked or moved while the lock was awaited.
        let project = projects::get(self.storage(), project_id)
            .await?
            .ok_or_else(project_not_found)?;
        let relink = match project.git_repo_path.as_deref() {
            Some(p) if repo_path_key(p) == path => true,
            Some(_) => return Err(already_linked()),
            None => false,
        };
        if tree::conflicted(&root).await? {
            return Err(CoreError::new(
                REPO_CONFLICTED,
                "The repository has conflicted files. Resolve them first.",
            ));
        }
        if !relink {
            let dirs: Vec<(String, String)> = tree::project_dirs(&root)
                .await?
                .into_iter()
                .map(|d| (d.dir, d.name))
                .collect();
            let dir = pick_project_dir(&project.name, &dirs);
            let yaml_there = dirs.iter().any(|(d, _)| *d == dir);
            let remote_url = Git::new(None)
                .remote_url(&root)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let now = now(core)?;
            let known = self.same_repo(&path).await?.repo_id;

            let mut tx = self.storage().write().await?;
            let ticket = self.take_seq();
            let (repo_id, new_repo) =
                register_repo_in(&mut tx, &path, &project.name, &remote_url, known.as_deref())
                    .await?;
            let mut row = projects::get(&mut tx, project_id)
                .await?
                .ok_or_else(project_not_found)?;
            if row.git_repo_path.is_some() {
                // Linked by another call since the check above.
                return Err(already_linked());
            }
            row.git_repo_path = Some(path.clone());
            row.updated_at = now;
            projects::update(&mut tx, &row).await?;
            projects::set_shared_dir(&mut tx, project_id, Some(&dir)).await?;
            let repo_ticket = new_repo.then(|| self.take_seq());
            tx.commit().await?;
            self.announce(
                ticket,
                StoredKind::Project,
                None,
                Some(vec![project_id.to_string()]),
                origin,
            );
            if let Some(t) = repo_ticket {
                self.announce(
                    t,
                    StoredKind::SharedRepo,
                    None,
                    Some(vec![repo_id.clone()]),
                    origin,
                );
            }

            let linked = self.linked(project_id).await?.ok_or_else(not_linked)?;
            // `project.yaml`, only when the directory has none.
            let has_yaml = yaml_there
                && (tree::read_file(&root, &format!("{}/project.yaml", linked.link().root()))
                    .await
                    .ok()
                    .flatten()
                    .is_some()
                    || tree::read_file(&root, &format!("{}/project.yml", linked.link().root()))
                        .await
                        .ok()
                        .flatten()
                        .is_some());
            if !has_yaml {
                self.publish_locked(core, origin, project_id, linked.clone(), Publish::Project)
                    .await?;
            }
        }
        let linked = self.linked(project_id).await?.ok_or_else(not_linked)?;
        // Probe fix 1: templates already in the directory that no connection
        // links: a ticked connection adopts one (by its kept file id, then by
        // name and type) instead of writing `<name>-2.yaml`.
        let mut free = self.free_templates(project_id, &linked).await?;
        // The ticked connections become templates, each linked (bug 6).
        for id in share {
            let Some(c) = connections::get(self.storage(), id).await? else {
                continue;
            };
            if c.shared_connection_id.is_some() {
                continue;
            }
            // Re-review I1: the tick is the explicit share (Q30), so a
            // local-only connection (the wizard's default, or one a Q31
            // unlink kept) stops being local-only before it's published.
            if c.is_local_only == Some(true) {
                let now = now(core)?;
                let mut tx = self.storage().write().await?;
                let ticket = self.take_seq();
                let patch = ConnectionPatch {
                    is_local_only: Some(false),
                    ..Default::default()
                };
                update_connection_in(&mut tx, id, &patch, &now, &SecretChanges::default()).await?;
                tx.commit().await?;
                self.announce(
                    ticket,
                    StoredKind::Connection,
                    Some(project_id.to_string()),
                    Some(vec![id.clone()]),
                    origin,
                );
            }
            if self
                .adopt_template(origin, project_id, &linked, &c, &mut free)
                .await?
            {
                continue;
            }
            let outcome = self
                .publish_locked(
                    core,
                    origin,
                    project_id,
                    linked.clone(),
                    Publish::Connection {
                        id,
                        renamed: false,
                        shared_now: true,
                    },
                )
                .await?;
            if let Some(o) = outcome.filter(|o| o.status == PublishStatus::Failed) {
                warn!(activity = "shared.linkProject", project_id = log_id(project_id), code = o.code.as_deref().unwrap_or(""); "Exporting a template failed");
            }
        }
        let linked = self.linked(project_id).await?.ok_or_else(not_linked)?;
        let (report, seq) = self
            .sync_locked(core, origin, project_id, linked, true)
            .await?;
        info!(activity = "shared.linkProject", project_id = log_id(project_id), relink = relink; "Linked a project");
        Ok(Seqd::new(report, seq))
    }

    /// The connections an unlink with `remove_imported` would remove (the
    /// unlink dialog's list, Task 7 re-review): the same test as
    /// [`Workspace::shared_unlink_project`], read without the repo lock.
    /// `PROJECT_NOT_LINKED` for a project without a link.
    pub async fn shared_unlink_preview(
        &self,
        core: &Core,
        project_id: &str,
    ) -> Result<UnlinkPreview> {
        core.require_local_files()?;
        let Some(linked) = self.linked(project_id).await? else {
            return Err(not_linked());
        };
        let prefix = format!("{}/", linked.link().kind_dir(Kind::Connection));
        let imported_connection_ids = connections::list_in_project(self.storage(), project_id)
            .await?
            .into_iter()
            .filter(|c| unlink_class(&linked, &prefix, c) == Some(UnlinkClass::Imported))
            .map(|c| c.id)
            .collect();
        Ok(UnlinkPreview {
            imported_connection_ids,
        })
    }

    /// Unlinks a project (Decision 40 with Q31): the connections linked to
    /// its directory's templates are unlinked and made local-only. The
    /// user's own (shared from here, `shared_origin` `exported`) always stay,
    /// with their secrets; the ones the repo brought (`imported`, or a link
    /// an older release stored) are removed with their secrets when
    /// `remove_imported`, and kept like the others otherwise. Clears its
    /// rows' links and its directory, and forgets the repo when no project
    /// uses it any more. No file is touched.
    pub async fn shared_unlink_project(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        remove_imported: bool,
    ) -> Result<Seqd<UnlinkReport>> {
        core.require_local_files()?;
        debug!(activity = "shared.unlinkProject", project_id = log_id(project_id); "Unlink a project");
        let Some((_lock, linked)) = self.lock_linked(core, project_id).await? else {
            return Err(not_linked());
        };
        let now = now(core)?;
        let plink = linked.link();
        // Review R8: other projects on the same folder, in any spelling,
        // read before the transaction (canonicalizing touches the disk).
        let others_using = self
            .same_repo(&linked.repo_path)
            .await?
            .project_ids
            .iter()
            .any(|id| id != project_id);
        let prefix = format!("{}/", plink.kind_dir(Kind::Connection));
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        for c in connections::list_in_project(&mut tx, project_id).await? {
            match unlink_class(&linked, &prefix, &c) {
                None => {}
                Some(UnlinkClass::Imported) if remove_imported => removed.push(c.id),
                Some(_) => kept.push(c.id),
            }
        }
        // Q31: what stays is unlinked (the origin goes with the link) and
        // local-only, its fields and secrets as they were.
        for id in &kept {
            // Probe fix 1: the template's file id stays (path, base and
            // origin go), so a relink can adopt that template again instead
            // of writing a second one.
            let file_id = connections::link(&mut tx, id)
                .await?
                .and_then(|l| l.file_id);
            let unlinked = SharedLink {
                file_id,
                ..SharedLink::default()
            };
            connections::set_link(&mut tx, id, &unlinked).await?;
            let patch = ConnectionPatch {
                is_local_only: Some(true),
                ..Default::default()
            };
            update_connection_in(&mut tx, id, &patch, &now, &SecretChanges::default()).await?;
        }
        let order: Vec<String> = connection_order_in(&mut tx, project_id)
            .await?
            .into_iter()
            .filter(|id| !removed.contains(id))
            .collect();
        for id in &removed {
            connections::delete(&mut tx, id).await?;
            user_credentials::remove_all_for_key_in(&mut tx, id).await?;
        }
        project_state::set_connection_order(&mut tx, project_id, &order).await?;
        let none = SharedLink::default();
        for r in saved_queries::links(&mut tx, project_id).await? {
            if r.link != none {
                saved_queries::set_link(&mut tx, &r.id, &none).await?;
            }
        }
        for r in dashboards::links(&mut tx, project_id).await? {
            if r.link != none {
                dashboards::set_link(&mut tx, &r.id, &none).await?;
            }
        }
        projects::set_shared_dir(&mut tx, project_id, None).await?;
        let mut row = projects::get(&mut tx, project_id)
            .await?
            .ok_or_else(project_not_found)?;
        row.git_repo_path = None;
        row.updated_at = now;
        projects::update(&mut tx, &row).await?;
        let still_used = others_using;
        let mut repo_removed = false;
        if !still_used {
            if let Some(repo_id) = &linked.repo_id {
                repo_removed = shared_repos::delete(&mut tx, repo_id).await?;
            }
        }
        let conn_ticket = (!removed.is_empty() || !kept.is_empty()).then(|| self.take_seq());
        let repo_ticket = repo_removed.then(|| self.take_seq());
        tx.commit().await?;
        self.announce(
            ticket,
            StoredKind::Project,
            None,
            Some(vec![project_id.to_string()]),
            origin,
        );
        if let Some(t) = conn_ticket {
            self.announce(
                t,
                StoredKind::Connection,
                Some(project_id.to_string()),
                Some(removed.iter().chain(&kept).cloned().collect()),
                origin,
            );
        }
        let mut seq = self.change_seq();
        if let (Some(t), Some(repo_id)) = (repo_ticket, &linked.repo_id) {
            seq = self.announce(
                t,
                StoredKind::SharedRepo,
                None,
                Some(vec![repo_id.clone()]),
                origin,
            );
        }
        for id in &removed {
            self.delete_connection_secrets(id).await;
        }
        info!(activity = "shared.unlinkProject", project_id = log_id(project_id), removed = removed.len(), kept = kept.len(), repo_removed = repo_removed; "Unlinked a project");
        Ok(Seqd::new(
            UnlinkReport {
                removed_connection_ids: removed,
                kept_connection_ids: kept,
                repo_removed,
            },
            seq,
        ))
    }

    #[cfg(feature = "secrets")]
    async fn delete_connection_secrets(&self, id: &str) {
        let Some(store) = self.secrets() else {
            return;
        };
        for prefix in CONNECTION_SECRETS {
            if let Err(e) = store.delete(&format!("{prefix}{id}")).await {
                warn!(activity = "shared.unlinkProject", code = e.code(); "Deleting a connection secret failed");
            }
        }
    }

    #[cfg(not(feature = "secrets"))]
    async fn delete_connection_secrets(&self, _id: &str) {
        let _ = CONNECTION_SECRETS;
    }

    /// What the repo at `path` holds, for the import dialog: its project
    /// directories with their names, templates and file counts. Reads only;
    /// a workspace on read-only storage answers it.
    pub async fn shared_scan(&self, core: &Core, path: &str) -> Result<RepoPreview> {
        core.require_local_files()?;
        debug!(activity = "shared.scan"; "Scan a repo");
        let path = repo_path_key(path);
        let root = PathBuf::from(&path);
        if !tree::is_dir(&root).await {
            return Err(CoreError::new(
                FILE_ERROR,
                "The repository's folder isn't there.",
            ));
        }
        let conflicted = tree::conflicted(&root).await?;
        let linked_ids = self.same_repo(&path).await?.project_ids;
        let mut by_dir: HashMap<String, Vec<String>> = HashMap::new();
        for id in linked_ids {
            if let Ok(Some(l)) = self.linked(&id).await {
                by_dir.entry(l.dir).or_default().push(id);
            }
        }
        let listing = tree::project_dirs_listing(&root).await?;
        let mut out = RepoPreview {
            conflicted,
            projects: Vec::new(),
            skipped_dirs: listing
                .skipped
                .into_iter()
                .map(|s| SkippedDir {
                    dir: s.rel_path,
                    why: s.why,
                })
                .collect(),
        };
        for d in listing.dirs {
            let rel = format!("{SEAQUEL_DIR}/projects/{}", d.dir);
            let scan = tree::scan(&root, &rel, ScanBounds::default()).await?;
            let (mut queries, mut boards) = (0u32, 0u32);
            let mut templates = Vec::new();
            for f in &scan.files {
                let lower = f.rel_path.to_ascii_lowercase();
                if lower.contains("/queries/") && lower.ends_with(".sql") {
                    queries += 1;
                } else if lower.contains("/dashboards/") && lower.ends_with(".json") {
                    boards += 1;
                } else if lower.contains("/connections/")
                    && (lower.ends_with(".yaml") || lower.ends_with(".yml"))
                {
                    if let Some(t) = parse_template(&f.text) {
                        templates.push(PreviewTemplate {
                            path: f
                                .rel_path
                                .strip_prefix(&format!("{SEAQUEL_DIR}/"))
                                .unwrap_or(&f.rel_path)
                                .to_string(),
                            name: t.name,
                            ty: t.ty,
                        });
                    }
                }
            }
            out.projects.push(PreviewProject {
                linked_project_ids: by_dir.remove(&d.dir).unwrap_or_default(),
                dir: d.dir,
                name: d.name,
                description: d.description,
                queries,
                dashboards: boards,
                templates,
                skipped: u32::try_from(scan.skipped.len()).unwrap_or(u32::MAX),
            });
        }
        Ok(out)
    }

    /// Imports the repo's project directories `dirs` (Decision 40): one
    /// project each, under the name its `project.yaml` gives (the first
    /// free `"<name> (n)"` when taken), linked to `path` with that
    /// directory stored (bug 8), then synced, which imports that
    /// directory's templates (bug 7). No file is written. The new project
    /// ids, in order.
    pub async fn shared_import_projects(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        path: &str,
        dirs: &[String],
    ) -> Result<Seqd<ImportedProjects>> {
        core.require_local_files()?;
        debug!(activity = "shared.importProjects", dirs = dirs.len(); "Import shared projects");
        let path = repo_path_key(path);
        let root = PathBuf::from(&path);
        if !tree::is_dir(&root).await {
            return Err(CoreError::new(
                FILE_ERROR,
                "The repository's folder isn't there.",
            ));
        }
        if dirs.iter().any(|d| check_component(d).is_err()) {
            return Err(invalid("A project folder isn't a valid name."));
        }
        let _lock = core.repo_lock(&root).await;
        if tree::conflicted(&root).await? {
            return Err(CoreError::new(
                REPO_CONFLICTED,
                "The repository has conflicted files. Resolve them first.",
            ));
        }
        let found = tree::project_dirs(&root).await?;
        let mut wanted = Vec::new();
        for dir in dirs {
            let d = found
                .iter()
                .find(|d| d.dir == *dir)
                .ok_or_else(|| invalid("A project folder isn't in the repository."))?;
            wanted.push(d.clone());
        }
        let remote_url = Git::new(None)
            .remote_url(&root)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        let limits = core.library_limits();
        // Probe fix 8: a directory a project here already links isn't
        // imported again (read under the lock, so a link can't race it).
        let mut linked_dirs = HashSet::new();
        for id in self.same_repo(&path).await?.project_ids {
            if let Ok(Some(l)) = self.linked(&id).await {
                linked_dirs.insert(l.dir);
            }
        }
        let mut out = ImportedProjects::default();
        for d in wanted {
            if linked_dirs.contains(&d.dir) {
                warn!(activity = "shared.importProjects", code = PROJECT_ALREADY_LINKED; "A project folder is already linked here");
                out.failures.push(ProjectFailure {
                    project_id: None,
                    dir: Some(d.dir.clone()),
                    code: PROJECT_ALREADY_LINKED.to_string(),
                    message: "A project here is already linked to this folder.".to_string(),
                });
                continue;
            }
            // Review M5: each directory is imported whole or not at all, and
            // one that fails doesn't stop the others.
            match self
                .import_one(core, origin, &path, &remote_url, &d, &limits)
                .await
            {
                Ok(id) => out.project_ids.push(id),
                Err(e) => {
                    warn!(activity = "shared.importProjects", code = e.code.as_str(); "A project's import failed");
                    out.failures.push(ProjectFailure {
                        project_id: None,
                        dir: Some(d.dir.clone()),
                        code: e.code,
                        message: e.message,
                    });
                }
            }
        }
        info!(activity = "shared.importProjects", projects = out.project_ids.len(), failures = out.failures.len(); "Imported shared projects");
        Ok(Seqd::new(out, self.change_seq()))
    }

    /// One directory of [`Workspace::shared_import_projects`], with the
    /// repo's lock held. If its sync fails, the project is removed again,
    /// with everything its sync stored, and so is a repo row it registered
    /// that no other project uses: an import leaves the whole project or
    /// nothing.
    async fn import_one(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        path: &str,
        remote_url: &str,
        d: &tree::ProjectDir,
        limits: &LibraryLimits,
    ) -> Result<String> {
        let now = now(core)?;
        let known = self.same_repo(path).await?.repo_id;
        // Re-review R3: the project (and a repo row it registers) is
        // announced only once its sync succeeded, through the after-commit
        // path, so no other window is told of a project the undo below may
        // remove. The rows its sync writes are announced as the sync goes,
        // scoped to a project no window knows yet.
        let mut tx = self.storage().write().await?;
        let project = insert_linked_project_in(&mut tx, &d.name, path, &now, limits).await?;
        projects::set_shared_dir(&mut tx, &project.id, Some(&d.dir)).await?;
        let (repo_id, new_repo) =
            register_repo_in(&mut tx, path, &project.name, remote_url, known.as_deref()).await?;
        tx.commit().await?;
        let synced = async {
            let linked = self.linked(&project.id).await?.ok_or_else(not_linked)?;
            self.sync_locked(core, origin, &project.id, linked, true)
                .await
        }
        .await;
        let e = match synced {
            Ok(_) => {
                self.record_storage_write(
                    origin,
                    StoredKind::Project,
                    None,
                    Some(vec![project.id.clone()]),
                );
                if new_repo {
                    self.record_storage_write(
                        origin,
                        StoredKind::SharedRepo,
                        None,
                        Some(vec![repo_id.clone()]),
                    );
                }
                return Ok(project.id);
            }
            Err(e) => e,
        };
        // Taken back: the project and what its sync stored cascade; the
        // repo row goes if this import made it and nothing else uses it.
        // Nothing was announced, so nothing is announced now.
        let others_using = match self.same_repo(path).await {
            Ok(same) => same.project_ids.iter().any(|id| *id != project.id),
            Err(_) => true,
        };
        let undone = async {
            let mut tx = self.storage().write().await?;
            projects::delete_with_orphans(&mut tx, &project.id).await?;
            if new_repo && !others_using {
                shared_repos::delete(&mut tx, &repo_id).await?;
            }
            tx.commit().await?;
            Ok::<_, CoreError>(())
        }
        .await;
        if let Err(u) = undone {
            warn!(activity = "shared.importProjects", code = u.code.as_str(); "Taking back a failed import failed");
        }
        Err(e)
    }
}

/// A template in a linked project's `connections/` that no connection
/// links (probe fix 1).
struct FreeTemplate {
    path: String,
    name_key: String,
    ty: String,
    file_id: Option<String>,
    hash: String,
}

impl Workspace {
    /// The project directory's templates no connection of the project links,
    /// read with the repo lock held.
    async fn free_templates(&self, project_id: &str, linked: &Linked) -> Result<Vec<FreeTemplate>> {
        let plink = linked.link();
        let dir = format!("{}/", plink.kind_dir(Kind::Connection));
        let claimed: HashSet<String> = connections::list_in_project(self.storage(), project_id)
            .await?
            .into_iter()
            .filter_map(|c| c.shared_connection_id)
            .filter_map(|l| template_path(&l).map(seaquel_workspace::shared::names::path_key))
            .collect();
        let scan = tree::scan(&linked.root, &plink.root(), ScanBounds::default()).await?;
        Ok(scan
            .files
            .into_iter()
            .filter(|f| f.rel_path.starts_with(&dir))
            .filter(|f| !claimed.contains(&seaquel_workspace::shared::names::path_key(&f.rel_path)))
            .filter_map(|f| {
                let t = parse_template(&f.text)?;
                let (hash, _) = seaquel_workspace::shared::file_hash(&f.rel_path, &f.text)?;
                Some(FreeTemplate {
                    name_key: lib::name_key(&t.name),
                    ty: t.ty,
                    file_id: t.file_id,
                    path: f.rel_path,
                    hash,
                })
            })
            .collect())
    }

    /// Links `c` to a free template that is its own (probe fix 1): the one
    /// carrying the file id `c` kept from its last link, else the one with
    /// its name (`name_key`) and type, and only when exactly one matches.
    /// The base is the file's hash, so the sync that ends the link writes
    /// the row's values into it when they differ (the tick is the share,
    /// Q30). Whether it adopted one.
    async fn adopt_template(
        &self,
        origin: &WriteOrigin,
        project_id: &str,
        linked: &Linked,
        c: &PersistedConnection,
        free: &mut Vec<FreeTemplate>,
    ) -> Result<bool> {
        let kept_id = connections::link(self.storage(), &c.id)
            .await?
            .and_then(|l| l.file_id);
        let by_id = kept_id
            .as_deref()
            .and_then(|fid| free.iter().position(|t| t.file_id.as_deref() == Some(fid)));
        let at = by_id.or_else(|| {
            let key = lib::name_key(&c.name);
            let named: Vec<usize> = free
                .iter()
                .enumerate()
                .filter(|(_, t)| t.name_key == key && t.ty == c.ty)
                .map(|(i, _)| i)
                .collect();
            (named.len() == 1).then(|| named[0])
        });
        let Some(at) = at else {
            return Ok(false);
        };
        let t = free.remove(at);
        // Review A1: the file is the base only when the row already says
        // the same. Otherwise no base, so the link's sync applies Q27 (the
        // template wins, the notice lists what it replaced) and nothing of
        // the user's is pushed over a teammate's template, by name or by a
        // kept file id whose file a teammate edited after the unlink.
        let row_hash = seaquel_workspace::shared::plan::row_hash(&RowChange::Connection {
            row: Some(c),
            link: &Link::default(),
            renamed: false,
            shared_now: false,
        });
        let base = (row_hash.as_deref() == Some(t.hash.as_str())).then_some(t.hash);
        let repo_id = linked.repo_id.clone().unwrap_or_default();
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let link = SharedLink {
            path: Some(format!("{repo_id}:{}", t.path)),
            base,
            file_id: t.file_id,
        };
        connections::set_link(&mut tx, &c.id, &link).await?;
        connections::set_origin(&mut tx, &c.id, Some(connections::ORIGIN_EXPORTED)).await?;
        tx.commit().await?;
        self.announce(
            ticket,
            StoredKind::Connection,
            Some(project_id.to_string()),
            Some(vec![c.id.clone()]),
            origin,
        );
        Ok(true)
    }
}

// ── The repo list (Decision 43) ──

impl Workspace {
    /// Every stored repo, as stored.
    pub async fn shared_repos(&self, core: &Core) -> Result<Seqd<Vec<Box<RawValue>>>> {
        core.require_local_files()?;
        let seq = self.change_seq();
        let value = shared_repos::list(self.storage()).await?;
        Ok(Seqd::new(value, seq))
    }

    /// Registers the repo at `path`, or answers the one already there
    /// (idempotent by path). Core makes the id (`repo-<uuid>`).
    pub async fn shared_repo_register(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        path: &str,
        name: Option<String>,
        remote_url: Option<String>,
    ) -> Result<Seqd<Box<RawValue>>> {
        core.require_local_files()?;
        debug!(activity = "shared.repoRegister"; "Register a repo");
        let path = repo_path_key(path);
        if path.is_empty() || path.contains('\0') {
            return Err(invalid("The repository's path is empty."));
        }
        let name = name.unwrap_or_else(|| {
            Path::new(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone())
        });
        let known = self.same_repo(&path).await?.repo_id;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let (id, new) = register_repo_in(
            &mut tx,
            &path,
            &name,
            remote_url.as_deref().unwrap_or(""),
            known.as_deref(),
        )
        .await?;
        let raw = shared_repos::get(&mut tx, &id)
            .await?
            .ok_or_else(repo_not_found)?;
        if !new {
            drop(tx);
            drop(ticket);
            return Ok(Seqd::new(raw, self.change_seq()));
        }
        tx.commit().await?;
        let seq = self.announce(ticket, StoredKind::SharedRepo, None, Some(vec![id]), origin);
        Ok(Seqd::new(raw, seq))
    }

    /// Changes the fields `patch` names, the rest byte for byte.
    pub async fn shared_repo_update(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        patch: RepoPatch,
    ) -> Result<Seqd<Box<RawValue>>> {
        core.require_local_files()?;
        debug!(activity = "shared.repoUpdate", repo_id = log_id(id); "Update a repo");
        let mut fields: Vec<(&str, Box<RawValue>)> = Vec::new();
        if let Some(v) = &patch.name {
            fields.push(("name", raw_value(json_string(v))));
        }
        if let Some(v) = &patch.remote_url {
            fields.push(("remoteUrl", raw_value(json_string(v))));
        }
        if let Some(v) = &patch.branch {
            fields.push(("branch", raw_value(json_string(v))));
        }
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let refs: Vec<(&str, &RawValue)> = fields.iter().map(|(k, v)| (*k, v.as_ref())).collect();
        if !refs.is_empty() && !shared_repos::update_json(&mut tx, id, &refs).await? {
            return Err(repo_not_found());
        }
        let raw = shared_repos::get(&mut tx, id)
            .await?
            .ok_or_else(repo_not_found)?;
        if refs.is_empty() {
            drop(tx);
            drop(ticket);
            return Ok(Seqd::new(raw, self.change_seq()));
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::SharedRepo,
            None,
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd::new(raw, seq))
    }

    /// Forgets a repo. Its files and the projects linked to it stay.
    pub async fn shared_repo_remove(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<()>> {
        core.require_local_files()?;
        debug!(activity = "shared.repoRemove", repo_id = log_id(id); "Remove a repo");
        // Review R8: whether a project uses the folder, in any spelling,
        // read before the transaction (canonicalizing touches the disk).
        let in_use = match shared_repos::get(self.storage(), id).await? {
            Some(raw) => match repo_field(&raw, "path") {
                Some(path) => !self.same_repo(&path).await?.project_ids.is_empty(),
                None => false,
            },
            None => false,
        };
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let raw = shared_repos::get(&mut tx, id)
            .await?
            .ok_or_else(repo_not_found)?;
        // Review I3 (decision): a repo a project links to stays; forgetting
        // it would make the next sync register it again under a new id and
        // import its templates a second time.
        if let Some(path) = repo_field(&raw, "path") {
            if in_use
                || !projects::ids_with_repo_path(&mut tx, &path)
                    .await?
                    .is_empty()
            {
                return Err(CoreError::new(
                    REPO_IN_USE,
                    "A project is still linked to this repository. Unlink it first.",
                ));
            }
        }
        if !shared_repos::delete(&mut tx, id).await? {
            return Err(repo_not_found());
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::SharedRepo,
            None,
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd::new((), seq))
    }
}

// ── Git under the repo lock (Decision 38) ──

impl Workspace {
    /// Pulls the repo at `path` under its lock. A successful pull sets the
    /// repo's `lastSyncAt` (Decision 43). The caller then syncs the repo
    /// (`SyncTarget::Repo`); a conflicted pull's sync answers `conflicted`.
    pub async fn shared_git_pull(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        git: &Git,
        path: &str,
        credentials: Option<GitCredentials>,
    ) -> Result<GitSyncResult> {
        core.require_local_files()?;
        let path = repo_path_key(path);
        let _lock = core.repo_lock(Path::new(&path)).await;
        let result = git.pull_repo(&path, credentials).await?;
        if result.success {
            self.record_last_sync_best_effort(core, origin, &path).await;
        }
        Ok(result)
    }

    /// Pushes the repo at `path`; a successful push sets `lastSyncAt`.
    pub async fn shared_git_push(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        git: &Git,
        path: &str,
        credentials: Option<GitCredentials>,
    ) -> Result<GitSyncResult> {
        core.require_local_files()?;
        let path = repo_path_key(path);
        let _lock = core.repo_lock(Path::new(&path)).await;
        let result = git.push_repo(&path, credentials).await?;
        if result.success {
            self.record_last_sync_best_effort(core, origin, &path).await;
        }
        Ok(result)
    }

    /// Commits everything in the repo at `path` under its lock.
    pub async fn shared_git_commit(
        &self,
        core: &Core,
        git: &Git,
        path: &str,
        message: &str,
    ) -> Result<String> {
        core.require_local_files()?;
        let path = repo_path_key(path);
        let _lock = core.repo_lock(Path::new(&path)).await;
        Ok(git.commit_changes(&path, message).await?)
    }

    /// Resolves a conflicted file under the repo's lock: with `resolution`'s
    /// text, or (`None`) by deleting it.
    pub async fn shared_git_resolve(
        &self,
        core: &Core,
        git: &Git,
        path: &str,
        file_path: &str,
        resolution: Option<&str>,
    ) -> Result<()> {
        core.require_local_files()?;
        let path = repo_path_key(path);
        let _lock = core.repo_lock(Path::new(&path)).await;
        // `None` keeps the side that deleted the file (probe fix 5).
        match resolution {
            Some(text) => git.resolve_conflict(&path, file_path, text).await?,
            None => git.resolve_conflict_deleted(&path, file_path).await?,
        }
        Ok(())
    }

    /// [`Workspace::record_last_sync`] after a pull or push that already
    /// changed the tree or the remote (Task 6 review): a failure is logged
    /// by code, never with the path, and the call still answers its
    /// result. `lastSyncAt` is bookkeeping; the next pull or push sets it.
    async fn record_last_sync_best_effort(&self, core: &Core, origin: &WriteOrigin, path: &str) {
        if let Err(e) = self.record_last_sync(core, origin, path).await {
            warn!(activity = "shared.lastSync", code = e.code.as_str(); "Couldn't record the repo's lastSyncAt");
        }
    }

    /// Splices `lastSyncAt` into the stored repo at `path` (when one is).
    async fn record_last_sync(&self, core: &Core, origin: &WriteOrigin, path: &str) -> Result<()> {
        let now = now(core)?;
        let Some(id) = self.same_repo(path).await?.repo_id else {
            return Ok(());
        };
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let value = raw_value(json_string(&now));
        shared_repos::update_json(&mut tx, &id, &[("lastSyncAt", &value)]).await?;
        tx.commit().await?;
        self.announce(ticket, StoredKind::SharedRepo, None, Some(vec![id]), origin);
        Ok(())
    }
}
