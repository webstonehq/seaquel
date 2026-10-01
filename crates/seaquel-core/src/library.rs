//! The library (phase 5d-1): saved connections, projects, custom labels,
//! saved queries and their versions, written through Core.
//!
//! Each write:
//! 1. checks its input against the entity's rules and the interface's
//!    [`LibraryLimits`] (`seaquel_workspace::library`), before anything is
//!    read;
//! 2. on the desktop, writes the keychain first (Decision 8), outside any
//!    write transaction, after the same checks ran on a pool read, so a
//!    keychain prompt never holds the write lock and a refused call never
//!    touches the keychain;
//! 3. reads, checks and writes in one [`WriteTx`] (Decision 3), taking its
//!    change number while it holds the lock (Decision 17), and commits;
//! 4. emits one `StorageChanged` event (Decision 16), then deletes the
//!    secrets the call gave up (best effort, logged by code).
//!
//! Inside a write, every read goes through the transaction (`&mut tx`),
//! never the pool. Nothing here logs or puts in an error a name, a host, a
//! connection string, query text or a secret: activities, ids, counts and
//! codes only.

use std::collections::HashSet;

use log::debug;
#[cfg(feature = "secrets")]
use log::warn;
use seaquel_storage::{
    app_state, connections, project_labels, projects, query_versions, saved_queries,
    user_credentials, IdName, Reader, Storage, WriteTx,
};
use seaquel_types::storage::{
    ConnectionLabel, PersistedConnection, PersistedProject, PersistedQueryVersion,
    PersistedSavedQuery,
};
use seaquel_workspace::library::{
    self as lib, check_count, find_taken, free_name, name_key, ConnectionDraft, ConnectionPatch,
    LabelDraft, LabelPatch, LabelRemoved, LibraryError, LibraryLimits, ProjectDraft, ProjectPatch,
    ProjectRemoved, SavedQueryDraft, SavedQueryPatch, SavedQueryUpdated, SecretChanges,
    VersionMeta, CONNECTION_ID_PREFIX, CONNECTION_SECRETS, LABEL_ID_PREFIX, LABEL_NOT_FOUND,
    LAST_PROJECT, PROJECT_ID_PREFIX, PROJECT_NOT_FOUND, QUERY_VERSION_ID_PREFIX,
    QUERY_VERSION_LIMIT_KEY, SAVED_QUERY_ID_PREFIX, SAVED_QUERY_NOT_FOUND,
};
use seaquel_workspace::run::iso_timestamp;

use crate::changes::WriteOrigin;
use crate::workspace::SAVED_CONNECTION_NOT_FOUND;
use crate::{Core, CoreError, Seqd, StoredKind, Workspace};

type Result<T> = std::result::Result<T, CoreError>;

fn now(core: &Core) -> Result<String> {
    let executor = core.executor.as_ref().ok_or_else(|| {
        CoreError::new(
            "NOT_SUPPORTED",
            "Saving isn't enabled here (no executor is set)",
        )
    })?;
    Ok(iso_timestamp(executor.unix_time()))
}

pub(crate) fn new_id(prefix: &str) -> String {
    format!("{prefix}{}", uuid::Uuid::new_v4())
}

pub(crate) fn connection_not_found() -> CoreError {
    CoreError::new(SAVED_CONNECTION_NOT_FOUND, "Saved connection not found.")
}

pub(crate) fn project_not_found() -> CoreError {
    CoreError::new(PROJECT_NOT_FOUND, "Project not found.")
}

fn label_not_found() -> CoreError {
    CoreError::new(LABEL_NOT_FOUND, "Label not found.")
}

fn saved_query_not_found() -> CoreError {
    CoreError::new(SAVED_QUERY_NOT_FOUND, "Saved query not found.")
}

/// A read inside the write transaction, or (for the checks before a
/// keychain write) on the pool.
enum Src<'a> {
    Pool(&'a Storage),
    Tx(&'a mut WriteTx),
}

impl Src<'_> {
    fn r(&mut self) -> Reader<'_> {
        match self {
            Src::Pool(st) => Reader::Pool(st),
            Src::Tx(tx) => Reader::Tx(tx),
        }
    }
}

/// `name`, or `NAME_TAKEN` naming the row of `rows` (other than `except`)
/// with the same key; with `rename`, the first free `"<name> (n)"`
/// instead (Decision 13).
fn resolve_name(
    what: &str,
    name: &str,
    rows: &[IdName],
    rename: bool,
    except: Option<&str>,
) -> std::result::Result<String, LibraryError> {
    let others = rows.iter().filter(|r| Some(r.id.as_str()) != except);
    if rename {
        let taken: HashSet<String> = others.map(|r| name_key(&r.name)).collect();
        return Ok(free_name(name, &taken));
    }
    match find_taken(name, others.map(|r| (r.id.as_str(), r.name.as_str())), None) {
        Some(id) => Err(LibraryError::name_taken(what, id)),
        None => Ok(name.to_string()),
    }
}

/// Where a name must be unique: a project's connections, the projects, or
/// a project folder's saved queries.
enum Names<'a> {
    Connections {
        project_id: &'a str,
    },
    Projects,
    SavedQueries {
        project_id: &'a str,
        folder: Option<&'a str>,
    },
}

impl Names<'_> {
    fn what(&self) -> &'static str {
        match self {
            Names::Connections { .. } => "connection",
            Names::Projects => "project",
            Names::SavedQueries { .. } => "saved query",
        }
    }

    /// The rows whose name has `key`, in rowid order: an index search of
    /// the stored `name_key` (phase 5d-1 probe fix), not a read of every
    /// name in the scope.
    async fn with_key(&self, src: &mut Src<'_>, key: &str) -> Result<Vec<IdName>> {
        Ok(match self {
            Names::Connections { project_id } => {
                connections::with_name_key(src.r(), project_id, key).await?
            }
            Names::Projects => projects::with_name_key(src.r(), key).await?,
            Names::SavedQueries { project_id, folder } => {
                saved_queries::with_name_key_in_folder(src.r(), project_id, *folder, key).await?
            }
        })
    }

    /// The first row other than `except` whose name has `name`'s key.
    async fn taken_by(
        &self,
        src: &mut Src<'_>,
        name: &str,
        except: Option<&str>,
    ) -> Result<Option<String>> {
        Ok(self
            .with_key(src, &name_key(name))
            .await?
            .into_iter()
            .find(|r| Some(r.id.as_str()) != except)
            .map(|r| r.id))
    }

    /// `name`, or `NAME_TAKEN` naming the first other row with its key;
    /// with `rename`, the first free `"<name> (n)"` instead (Decision 13),
    /// one lookup per candidate. As [`resolve_name`], without reading every
    /// name in the scope.
    async fn resolve(
        &self,
        src: &mut Src<'_>,
        name: &str,
        rename: bool,
        except: Option<&str>,
    ) -> Result<String> {
        let Some(holder) = self.taken_by(src, name, except).await? else {
            return Ok(name.to_string());
        };
        if !rename {
            return Err(LibraryError::name_taken(self.what(), holder).into());
        }
        // Each taken candidate is a distinct row, so this ends by the time
        // it has tried one more than the rows in the scope.
        for n in 2u64.. {
            let candidate = format!("{name} ({n})");
            if self.taken_by(src, &candidate, except).await?.is_none() {
                return Ok(candidate);
            }
        }
        unreachable!("a free name exists")
    }
}

fn id_names(labels: &[ConnectionLabel]) -> Vec<IdName> {
    labels
        .iter()
        .map(|l| IdName {
            id: l.id.clone(),
            name: l.name.clone(),
        })
        .collect()
}

/// A new connection's checks that read: its project, its labels against
/// the project's, and its name (renamed for an import).
async fn new_connection_checks(
    src: &mut Src<'_>,
    row: &mut PersistedConnection,
    rename: bool,
    limits: &LibraryLimits,
) -> Result<()> {
    let project = projects::get(src.r(), &row.project_id)
        .await?
        .ok_or_else(project_not_found)?;
    lib::check_connection(row, &project.custom_labels, limits)?;
    let names = Names::Connections {
        project_id: &row.project_id,
    };
    row.name = names.resolve(src, &row.name, rename, None).await?;
    Ok(())
}

/// A patched connection's checks that read: only what the patch changed,
/// so a stored row with an old label id can still be renamed.
async fn patched_connection_checks(
    src: &mut Src<'_>,
    before: &PersistedConnection,
    row: &PersistedConnection,
    patch: &ConnectionPatch,
) -> Result<()> {
    if patch.label_ids.is_some() {
        let labels = project_labels::list(src.r(), &row.project_id).await?;
        lib::check_labels(&row.label_ids, &labels)?;
    }
    if patch.name.is_some() && name_key(&row.name) != name_key(&before.name) {
        let names = Names::Connections {
            project_id: &row.project_id,
        };
        names.resolve(src, &row.name, false, Some(&row.id)).await?;
    }
    Ok(())
}

/// `ENGINE_NOT_AVAILABLE` for a type this Core has no engine for (the web
/// server's SQLite and DuckDB; Decision 7).
fn check_engine(core: &Core, ty: &str) -> Result<()> {
    if core.engine_ids().contains(&lib::engine_of(ty)) {
        Ok(())
    } else {
        Err(CoreError::new(
            lib::ENGINE_NOT_AVAILABLE,
            "Connections of this type aren't available here.",
        ))
    }
}

// ── Secrets ──

/// How [`Workspace::set_secrets`] undoes a partly failed write.
#[derive(Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(feature = "secrets"), allow(dead_code))]
enum Undo {
    /// A new connection: delete what was set.
    Delete,
    /// An existing one: put back what was there.
    Restore,
}

impl Workspace {
    /// `NOT_SUPPORTED` for secrets on a workspace without a store (the
    /// web's vault stays in the browser; Decision 8).
    fn check_secret_support(&self, secrets: &SecretChanges) -> Result<()> {
        if secrets.is_empty() || self.has_secret_store() {
            Ok(())
        } else {
            Err(CoreError::new(
                "NOT_SUPPORTED",
                "Saving passwords with the connection isn't available here.",
            ))
        }
    }

    #[cfg(feature = "secrets")]
    pub(crate) fn has_secret_store(&self) -> bool {
        self.secrets().is_some()
    }

    #[cfg(not(feature = "secrets"))]
    pub(crate) fn has_secret_store(&self) -> bool {
        false
    }

    /// Set `sets` under the connection `id`. On a failure it fails with the
    /// store's code and undoes the ones it already set: a create
    /// ([`Undo::Delete`]) deletes them (the connection is new, so nothing
    /// was there), and an update ([`Undo::Restore`]) puts back what each
    /// entry held before, read first, so a half-done update never loses a
    /// working password.
    #[cfg(feature = "secrets")]
    async fn set_secrets(&self, id: &str, sets: &[(&'static str, &str)], undo: Undo) -> Result<()> {
        let Some(store) = self.secrets() else {
            return Ok(());
        };
        let mut before: Vec<Option<String>> = Vec::with_capacity(sets.len());
        if undo == Undo::Restore {
            for (prefix, _) in sets {
                before.push(store.get(&format!("{prefix}{id}")).await.map_err(|e| {
                    warn!(activity = "library.secrets", code = e.code(); "Reading a connection secret failed");
                    CoreError::from(e)
                })?);
            }
        }
        for (i, (prefix, value)) in sets.iter().enumerate() {
            if let Err(e) = store.set(&format!("{prefix}{id}"), value).await {
                warn!(activity = "library.secrets", code = e.code(); "Saving a connection secret failed");
                for (j, (done, _)) in sets[..i].iter().enumerate() {
                    let key = format!("{done}{id}");
                    let undone = match before.get(j).cloned().flatten() {
                        Some(old) => store.set(&key, &old).await,
                        None => store.delete(&key).await,
                    };
                    if let Err(u) = undone {
                        warn!(activity = "library.secrets", code = u.code(); "Undoing a connection secret failed");
                    }
                }
                return Err(e.into());
            }
        }
        Ok(())
    }

    #[cfg(not(feature = "secrets"))]
    async fn set_secrets(
        &self,
        _id: &str,
        _sets: &[(&'static str, &str)],
        _undo: Undo,
    ) -> Result<()> {
        Ok(())
    }

    /// Delete the connection `id`'s secrets with these key prefixes, best
    /// effort: a failure is logged by its code.
    #[cfg(feature = "secrets")]
    async fn delete_secrets(&self, id: &str, prefixes: &[&str]) {
        let Some(store) = self.secrets() else {
            return;
        };
        for prefix in prefixes {
            if let Err(e) = store.delete(&format!("{prefix}{id}")).await {
                warn!(activity = "library.secrets", code = e.code(); "Deleting a connection secret failed");
            }
        }
    }

    #[cfg(not(feature = "secrets"))]
    async fn delete_secrets(&self, _id: &str, _prefixes: &[&str]) {}
}

// ── Connections ──

impl Workspace {
    /// Every saved connection, with the sequence it's at least as new as.
    pub async fn list_connections(&self) -> Result<Seqd<Vec<PersistedConnection>>> {
        let seq = self.change_seq();
        let value = connections::load_all(self.storage()).await?;
        Ok(Seqd { value, seq })
    }

    /// Save a new connection (`connectionCreate`) with a Core id, and its
    /// secrets first on the desktop (Decision 8). The connection string is
    /// stored without secrets.
    ///
    /// Errors: `INVALID_ARGUMENT`, `ENGINE_NOT_AVAILABLE`,
    /// `PROJECT_NOT_FOUND`, `LABEL_NOT_FOUND`, `NAME_TAKEN` (unless the
    /// draft says `renameIfTaken`), `NOT_SUPPORTED` (secrets without a
    /// store), `SECRET_STORE_ERROR` (nothing is stored), the storage codes.
    pub async fn create_connection(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        draft: ConnectionDraft,
        secrets: SecretChanges,
    ) -> Result<Seqd<PersistedConnection>> {
        debug!(activity = "library.connectionCreate", labels = draft.label_ids.len(); "Create a connection");
        let limits = core.library_limits();
        lib::check_connection_draft(&draft, &limits)?;
        lib::check_secret_values(&secrets)?;
        self.check_secret_support(&secrets)?;
        check_engine(core, &draft.ty)?;
        let now = now(core)?;
        let id = new_id(CONNECTION_ID_PREFIX);
        let mut row = lib::connection_from_draft(id.clone(), &draft, &now);
        lib::check_secret_flags(&secrets, &row)?;

        let sets = secrets.sets();
        if !sets.is_empty() {
            let mut probe = row.clone();
            let mut src = Src::Pool(self.storage());
            new_connection_checks(&mut src, &mut probe, draft.rename_if_taken, &limits).await?;
            self.set_secrets(&id, &sets, Undo::Delete).await?;
        }

        let written = async {
            let mut tx = self.storage().write().await?;
            let ticket = self.take_seq();
            let count = connections::count(&mut tx).await?;
            check_count(count, limits.max_connections, "max_connections")?;
            let mut src = Src::Tx(&mut tx);
            new_connection_checks(&mut src, &mut row, draft.rename_if_taken, &limits).await?;
            connections::insert(&mut tx, &row).await?;
            let stored = connections::get(&mut tx, &id)
                .await?
                .ok_or_else(connection_not_found)?;
            tx.commit().await?;
            Ok::<_, CoreError>((stored, ticket))
        }
        .await;
        match written {
            Ok((stored, ticket)) => {
                let seq = self.announce(
                    ticket,
                    StoredKind::Connection,
                    Some(stored.project_id.clone()),
                    Some(vec![id]),
                    origin,
                );
                Ok(Seqd { value: stored, seq })
            }
            Err(e) => {
                if !sets.is_empty() {
                    let prefixes: Vec<&str> = sets.iter().map(|(p, _)| *p).collect();
                    self.delete_secrets(&id, &prefixes).await;
                }
                Err(e)
            }
        }
    }

    /// Change a saved connection (`connectionUpdate`): only the patch's
    /// fields (Decision 2). Secrets are set first; a secret whose save flag
    /// the patch turned off, or that `secrets` deletes, is deleted after the
    /// commit.
    ///
    /// If setting one secret fails, the ones already set get their previous
    /// values back. Once every secret is set, a transaction that then fails
    /// (a refusal read inside it, a storage error) keeps the new keychain
    /// values: they're the user's latest, and the row's save flags still
    /// decide whether they're read. Only a row removed meanwhile loses its
    /// secrets.
    ///
    /// Errors: as [`Workspace::create_connection`], and
    /// `CONNECTION_NOT_FOUND` for an id that isn't saved here.
    pub async fn update_connection(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        patch: ConnectionPatch,
        secrets: SecretChanges,
    ) -> Result<Seqd<PersistedConnection>> {
        debug!(activity = "library.connectionUpdate", connection_id = id; "Update a connection");
        let limits = core.library_limits();
        lib::check_id(id, "connection id", &limits)?;
        lib::check_connection_patch(&patch, &limits)?;
        lib::check_secret_values(&secrets)?;
        self.check_secret_support(&secrets)?;
        if let Some(ty) = &patch.ty {
            check_engine(core, ty)?;
        }
        let now = now(core)?;

        let sets = secrets.sets();
        if !sets.is_empty() {
            let before = connections::get(self.storage(), id)
                .await?
                .ok_or_else(connection_not_found)?;
            let mut row = before.clone();
            lib::apply_connection_patch(&mut row, &patch, &now);
            lib::check_secret_flags(&secrets, &row)?;
            let mut src = Src::Pool(self.storage());
            patched_connection_checks(&mut src, &before, &row, &patch).await?;
            self.set_secrets(id, &sets, Undo::Restore).await?;
        }

        let written = async {
            let mut tx = self.storage().write().await?;
            let ticket = self.take_seq();
            let before = connections::get(&mut tx, id)
                .await?
                .ok_or_else(connection_not_found)?;
            let mut row = before.clone();
            lib::apply_connection_patch(&mut row, &patch, &now);
            lib::check_secret_flags(&secrets, &row)?;
            let mut src = Src::Tx(&mut tx);
            patched_connection_checks(&mut src, &before, &row, &patch).await?;
            if !connections::update(&mut tx, &row).await? {
                return Err(connection_not_found());
            }
            let stored = connections::get(&mut tx, id)
                .await?
                .ok_or_else(connection_not_found)?;
            tx.commit().await?;
            Ok::<_, CoreError>((before, stored, ticket))
        }
        .await;
        let (before, stored, ticket) = match written {
            Ok(done) => done,
            Err(e) => {
                if e.code == SAVED_CONNECTION_NOT_FOUND && !sets.is_empty() {
                    // Removed since the check: its secrets go with it.
                    self.delete_secrets(id, &CONNECTION_SECRETS).await;
                }
                return Err(e);
            }
        };
        let seq = self.announce(
            ticket,
            StoredKind::Connection,
            Some(stored.project_id.clone()),
            Some(vec![id.to_string()]),
            origin,
        );
        let mut gone: Vec<&str> = secrets.deletes();
        for prefix in CONNECTION_SECRETS {
            if lib::keeps_secret(&before, prefix)
                && !lib::keeps_secret(&stored, prefix)
                && !gone.contains(&prefix)
            {
                gone.push(prefix);
            }
        }
        if !gone.is_empty() {
            self.delete_secrets(id, &gone).await;
        }
        Ok(Seqd { value: stored, seq })
    }

    /// Remove a saved connection (`connectionRemove`). Its history, chats
    /// and labels cascade, and its web vault rows go in the same
    /// transaction; its keychain entries are deleted after the commit. It
    /// doesn't close a Core connection opened from it (Decision 9).
    pub async fn remove_connection(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<()>> {
        debug!(activity = "library.connectionRemove", connection_id = id; "Remove a connection");
        lib::check_id(id, "connection id", &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let row = connections::get(&mut tx, id)
            .await?
            .ok_or_else(connection_not_found)?;
        connections::delete(&mut tx, id).await?;
        user_credentials::remove_all_for_key_in(&mut tx, id).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Connection,
            Some(row.project_id),
            Some(vec![id.to_string()]),
            origin,
        );
        self.delete_secrets(id, &CONNECTION_SECRETS).await;
        Ok(Seqd { value: (), seq })
    }
}

// ── Projects ──

impl Workspace {
    pub async fn list_projects(&self) -> Result<Seqd<Vec<PersistedProject>>> {
        let seq = self.change_seq();
        let value = projects::load_all(self.storage()).await?;
        Ok(Seqd { value, seq })
    }

    /// Save a new project (`projectCreate`).
    pub async fn create_project(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        draft: ProjectDraft,
    ) -> Result<Seqd<PersistedProject>> {
        debug!(activity = "library.projectCreate"; "Create a project");
        let limits = core.library_limits();
        lib::check_project_draft(&draft, &limits)?;
        let now = now(core)?;
        let id = new_id(PROJECT_ID_PREFIX);
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let count = projects::count(&mut tx).await?;
        check_count(count, limits.max_projects, "max_projects")?;
        let name = Names::Projects
            .resolve(
                &mut Src::Tx(&mut tx),
                &draft.name,
                draft.rename_if_taken,
                None,
            )
            .await?;
        let row = lib::project_from_draft(id.clone(), &draft, name, &now);
        projects::insert(&mut tx, &row).await?;
        tx.commit().await?;
        let seq = self.announce(ticket, StoredKind::Project, None, Some(vec![id]), origin);
        Ok(Seqd { value: row, seq })
    }

    /// Make the default project on a file with no project
    /// (`projectEnsureDefault`), and list the projects.
    pub async fn ensure_default_project(
        &self,
        core: &Core,
        origin: &WriteOrigin,
    ) -> Result<Seqd<Vec<PersistedProject>>> {
        debug!(activity = "library.projectEnsureDefault"; "Ensure the default project");
        let now = now(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let inserted = if projects::count(&mut tx).await? == 0 {
            let inserted =
                projects::insert_if_missing(&mut tx, &lib::default_project(&now)).await?;
            tx.commit().await?;
            inserted
        } else {
            drop(tx);
            false
        };
        // The ticket is published or dropped before the list is read, so a
        // number in flight never holds back the published sequence.
        let seq = if inserted {
            self.announce(
                ticket,
                StoredKind::Project,
                None,
                Some(vec![lib::DEFAULT_PROJECT_ID.to_string()]),
                origin,
            )
        } else {
            drop(ticket);
            self.change_seq()
        };
        let value = projects::load_all(self.storage()).await?;
        Ok(Seqd { value, seq })
    }

    /// Change a project (`projectUpdate`); `updatedAt` becomes now.
    pub async fn update_project(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        patch: ProjectPatch,
    ) -> Result<Seqd<PersistedProject>> {
        debug!(activity = "library.projectUpdate", project_id = id; "Update a project");
        let limits = core.library_limits();
        lib::check_id(id, "project id", &limits)?;
        lib::check_project_patch(&patch, &limits)?;
        let now = now(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let mut row = projects::get(&mut tx, id)
            .await?
            .ok_or_else(project_not_found)?;
        let before = row.name.clone();
        lib::apply_project_patch(&mut row, &patch, &now);
        if patch.name.is_some() && name_key(&row.name) != name_key(&before) {
            Names::Projects
                .resolve(&mut Src::Tx(&mut tx), &row.name, false, Some(id))
                .await?;
        }
        projects::update(&mut tx, &row).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Project,
            None,
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd { value: row, seq })
    }

    /// Remove a project and everything in it (`projectRemove`, Decision 9),
    /// in one transaction, then its connections' keychain entries. The last
    /// project can't go (`LAST_PROJECT`).
    pub async fn remove_project(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<ProjectRemoved>> {
        debug!(activity = "library.projectRemove", project_id = id; "Remove a project");
        lib::check_id(id, "project id", &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        if projects::get(&mut tx, id).await?.is_none() {
            return Err(project_not_found());
        }
        if projects::count(&mut tx).await? <= 1 {
            return Err(CoreError::new(
                LAST_PROJECT,
                "The last project can't be removed.",
            ));
        }
        let connection_ids = projects::delete_with_orphans(&mut tx, id)
            .await?
            .ok_or_else(project_not_found)?;
        for connection_id in &connection_ids {
            user_credentials::remove_all_for_key_in(&mut tx, connection_id).await?;
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Project,
            None,
            Some(vec![id.to_string()]),
            origin,
        );
        for connection_id in &connection_ids {
            self.delete_secrets(connection_id, &CONNECTION_SECRETS)
                .await;
        }
        Ok(Seqd {
            value: ProjectRemoved { connection_ids },
            seq,
        })
    }
}

// ── Labels ──

impl Workspace {
    /// Add a custom label to a project (`labelCreate`). Only its
    /// `project_labels` row is written (Decision 10).
    pub async fn create_label(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        draft: LabelDraft,
    ) -> Result<Seqd<ConnectionLabel>> {
        debug!(activity = "library.labelCreate", project_id = project_id; "Create a label");
        let limits = core.library_limits();
        lib::check_id(project_id, "project id", &limits)?;
        lib::check_label_draft(&draft, &limits)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let project = projects::get(&mut tx, project_id)
            .await?
            .ok_or_else(project_not_found)?;
        list_room(project.custom_labels.len(), &limits)?;
        let name = resolve_name(
            "label",
            &draft.name,
            &id_names(&project.custom_labels),
            false,
            None,
        )?;
        let label = ConnectionLabel {
            id: new_id(LABEL_ID_PREFIX),
            name,
            is_predefined: false,
            color: draft.color,
        };
        project_labels::insert(&mut tx, project_id, &label).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Label,
            Some(project_id.to_string()),
            Some(vec![label.id.clone()]),
            origin,
        );
        Ok(Seqd { value: label, seq })
    }

    /// Rename or recolour a custom label (`labelUpdate`).
    pub async fn update_label(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        label_id: &str,
        patch: LabelPatch,
    ) -> Result<Seqd<ConnectionLabel>> {
        debug!(activity = "library.labelUpdate", project_id = project_id, label_id = label_id; "Update a label");
        let limits = core.library_limits();
        lib::check_id(project_id, "project id", &limits)?;
        lib::check_custom_label_id(label_id, &limits)?;
        lib::check_label_patch(&patch, &limits)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let project = projects::get(&mut tx, project_id)
            .await?
            .ok_or_else(project_not_found)?;
        let mut label = project
            .custom_labels
            .iter()
            .find(|l| l.id == label_id)
            .cloned()
            .ok_or_else(label_not_found)?;
        let before = label.name.clone();
        lib::apply_label_patch(&mut label, &patch);
        if patch.name.is_some() && name_key(&label.name) != name_key(&before) {
            resolve_name(
                "label",
                &label.name,
                &id_names(&project.custom_labels),
                false,
                Some(label_id),
            )?;
        }
        if !project_labels::update(&mut tx, project_id, &label).await? {
            return Err(label_not_found());
        }
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Label,
            Some(project_id.to_string()),
            Some(vec![label_id.to_string()]),
            origin,
        );
        Ok(Seqd { value: label, seq })
    }

    /// Remove a custom label (`labelRemove`), and take it off every
    /// connection that has it, in the same transaction (Decision 10). A
    /// predefined id is refused before anything is read.
    pub async fn remove_label(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        project_id: &str,
        label_id: &str,
    ) -> Result<Seqd<LabelRemoved>> {
        debug!(activity = "library.labelRemove", project_id = project_id, label_id = label_id; "Remove a label");
        let limits = core.library_limits();
        lib::check_id(project_id, "project id", &limits)?;
        lib::check_custom_label_id(label_id, &limits)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        if projects::get(&mut tx, project_id).await?.is_none() {
            return Err(project_not_found());
        }
        if !project_labels::delete(&mut tx, project_id, label_id).await? {
            return Err(label_not_found());
        }
        let connection_ids = project_labels::strip_from_connections(&mut tx, label_id).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::Label,
            Some(project_id.to_string()),
            Some(vec![label_id.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: LabelRemoved { connection_ids },
            seq,
        })
    }
}

/// A project's labels are a list too (`max_list_items`).
fn list_room(len: usize, limits: &LibraryLimits) -> Result<()> {
    check_count(len as u64, limits.max_list_items, "max_list_items").map_err(Into::into)
}

// ── Saved queries ──

impl Workspace {
    /// A project's saved queries. The id is size-checked first, as a
    /// write's is.
    pub async fn list_saved_queries(
        &self,
        core: &Core,
        project_id: &str,
    ) -> Result<Seqd<Vec<PersistedSavedQuery>>> {
        lib::check_id(project_id, "project id", &core.library_limits())?;
        let seq = self.change_seq();
        let value = saved_queries::load_by_project(self.storage(), project_id).await?;
        Ok(Seqd { value, seq })
    }

    /// Every version of a project's saved queries. The id is size-checked
    /// first, as a write's is.
    pub async fn list_query_versions(
        &self,
        core: &Core,
        project_id: &str,
    ) -> Result<Seqd<Vec<PersistedQueryVersion>>> {
        lib::check_id(project_id, "project id", &core.library_limits())?;
        let seq = self.change_seq();
        let value = query_versions::load_by_project(self.storage(), project_id).await?;
        Ok(Seqd { value, seq })
    }

    /// Save a new saved query (`savedQueryCreate`).
    pub async fn create_saved_query(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        draft: SavedQueryDraft,
    ) -> Result<Seqd<PersistedSavedQuery>> {
        debug!(activity = "library.savedQueryCreate", project_id = draft.project_id.as_str(); "Create a saved query");
        let limits = core.library_limits();
        lib::check_saved_query_draft(&draft, &limits)?;
        let now = now(core)?;
        let id = new_id(SAVED_QUERY_ID_PREFIX);
        let row = lib::saved_query_from_draft(id.clone(), &draft, &now);
        lib::check_saved_query(&row, &limits)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let count = saved_queries::count(&mut tx).await?;
        check_count(count, limits.max_saved_queries, "max_saved_queries")?;
        if projects::get(&mut tx, &row.project_id).await?.is_none() {
            return Err(project_not_found());
        }
        Names::SavedQueries {
            project_id: &row.project_id,
            folder: row.folder.as_deref(),
        }
        .resolve(&mut Src::Tx(&mut tx), &row.name, false, None)
        .await?;
        saved_queries::insert(&mut tx, &row).await?;
        let stored = saved_queries::get(&mut tx, &id)
            .await?
            .ok_or_else(saved_query_not_found)?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::SavedQuery,
            Some(stored.project_id.clone()),
            Some(vec![id]),
            origin,
        );
        Ok(Seqd { value: stored, seq })
    }

    /// Change a saved query (`savedQueryUpdate`, Decision 11). A changed
    /// text appends a keyframe of the previous text, numbered inside the
    /// transaction, then prunes to `query_version_limit` keeping back to the
    /// nearest keyframe. An unchanged text adds no version.
    pub async fn update_saved_query(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
        patch: SavedQueryPatch,
    ) -> Result<Seqd<SavedQueryUpdated>> {
        debug!(activity = "library.savedQueryUpdate", saved_query_id = id; "Update a saved query");
        let limits = core.library_limits();
        lib::check_id(id, "saved query id", &limits)?;
        lib::check_saved_query_patch(&patch, &limits)?;
        let now = now(core)?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let mut row = saved_queries::get(&mut tx, id)
            .await?
            .ok_or_else(saved_query_not_found)?;
        let change = lib::apply_saved_query_patch(&mut row, &patch, &now);
        if change.renamed {
            Names::SavedQueries {
                project_id: &row.project_id,
                folder: row.folder.as_deref(),
            }
            .resolve(&mut Src::Tx(&mut tx), &row.name, false, Some(id))
            .await?;
        }
        if !saved_queries::update(&mut tx, &row).await? {
            return Err(saved_query_not_found());
        }
        let mut version = None;
        let mut pruned_version_ids = Vec::new();
        if let Some(previous) = &change.previous_text {
            let v = query_versions::append_keyframe(
                &mut tx,
                &new_id(QUERY_VERSION_ID_PREFIX),
                id,
                previous,
                &now,
            )
            .await?;
            let limit = app_state::get(&mut tx, QUERY_VERSION_LIMIT_KEY).await?;
            let keep = lib::parse_version_limit(limit.as_deref());
            let metas: Vec<VersionMeta> = query_versions::list_meta(&mut tx, id)
                .await?
                .into_iter()
                .map(|m| VersionMeta {
                    id: m.id,
                    version: m.version,
                    keyframe: m.keyframe,
                    bytes: m.bytes,
                })
                .collect();
            pruned_version_ids = lib::version_prune(&metas, keep, limits.max_version_bytes);
            query_versions::delete_ids(&mut tx, id, &pruned_version_ids).await?;
            version = Some(v);
        }
        let stored = saved_queries::get(&mut tx, id)
            .await?
            .ok_or_else(saved_query_not_found)?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::SavedQuery,
            Some(stored.project_id.clone()),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd {
            value: SavedQueryUpdated {
                query: stored,
                version,
                pruned_version_ids,
            },
            seq,
        })
    }

    /// Remove a saved query (`savedQueryRemove`); its versions cascade.
    pub async fn remove_saved_query(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        id: &str,
    ) -> Result<Seqd<()>> {
        debug!(activity = "library.savedQueryRemove", saved_query_id = id; "Remove a saved query");
        lib::check_id(id, "saved query id", &core.library_limits())?;
        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let row = saved_queries::get(&mut tx, id)
            .await?
            .ok_or_else(saved_query_not_found)?;
        saved_queries::delete(&mut tx, id).await?;
        tx.commit().await?;
        let seq = self.announce(
            ticket,
            StoredKind::SavedQuery,
            Some(row.project_id),
            Some(vec![id.to_string()]),
            origin,
        );
        Ok(Seqd { value: (), seq })
    }
}
