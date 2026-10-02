//! Importing connections from TablePlus and DBeaver (phase 5e, Decision
//! 47): finding and reading the other tool's file, the candidates, and the
//! import itself.
//!
//! The mapping, problems and duplicate check are
//! `seaquel_workspace::imports`'. Core reads the file (at most
//! [`MAX_IMPORT_FILE_BYTES`]), decodes TablePlus's plist (its events
//! counted and its nesting bounded before anything is built, so a hostile
//! plist can't exhaust memory or the stack), and writes:
//!
//! - [`Workspace::import_candidates`] answers `{found: false}`, `{found:
//!   true, unreadable}` (Core's own message, never an I/O or plist error,
//!   which can name a path) or the candidates with `duplicateOf` against
//!   the project's connections.
//! - [`Workspace::import_create`] reads the file again, refuses a key whose
//!   candidate has a problem, and imports the rest in one `WriteTx`: the
//!   duplicate check against the connections read inside it and those this
//!   call imported, `connectionCreate`'s rules with `renameIfTaken` and
//!   local-only, and the new ids appended to the project's connection
//!   order in the same transaction (bug 23).
//!
//! The default locations are resolved from [`ImportPaths`], never the
//! process's own home in tests. Nothing here logs a path, a name, a host
//! or a file's content.

use std::collections::HashSet;
use std::fmt;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use log::{debug, info};
use seaquel_storage::{connections, project_state, projects};
use seaquel_workspace::imports::{
    dbeaver_candidates, default_path, mark_duplicates, refused_key, tableplus_candidates,
    ConnectionIdentity, ImportCandidate, ImportCandidates, ImportSource,
};
use seaquel_workspace::library::{self as lib, ConnectionDraft, PROJECT_NOT_FOUND};
pub use seaquel_workspace::shared_api::{ImportKeyOutcome, ImportOutcome};

use crate::changes::WriteOrigin;
use crate::library::{check_engine, connection_order_in, insert_connection_in, new_id, now};
use crate::{Core, CoreError, Seqd, StoredKind, Workspace};

type Result<T> = std::result::Result<T, CoreError>;

/// `importsCreate` when the file can't be read any more.
pub const IMPORT_SOURCE_UNREADABLE: &str = "IMPORT_SOURCE_UNREADABLE";

/// The largest file an import reads. TablePlus's and DBeaver's files are
/// kilobytes; past this the file is `unreadable`.
pub const MAX_IMPORT_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// A TablePlus plist nested deeper than this is `unreadable`: the
/// connections are a list of dicts, two levels with their SSH settings.
const MAX_PLIST_DEPTH: usize = 64;

/// A plist with more events than this is `unreadable`: a binary plist's
/// shared references can expand a small file into an enormous tree.
const MAX_PLIST_EVENTS: usize = 2_000_000;

/// Where the imports look for the other tools' files: their default
/// locations under `home` (`seaquel_workspace::imports::default_path`).
#[derive(Clone)]
pub struct ImportPaths {
    pub home: PathBuf,
}

impl fmt::Debug for ImportPaths {
    // The home dir names the user.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImportPaths").finish_non_exhaustive()
    }
}

impl ImportPaths {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }

    /// The current user's home, for the desktop and the CLI.
    pub fn from_env() -> Option<Self> {
        dirs::home_dir().map(Self::new)
    }
}

/// What reading the source found.
enum Source {
    NotFound,
    /// Core's own message, naming no path.
    Unreadable(&'static str),
    Bytes(Vec<u8>),
}

fn read_capped(path: &Path) -> Source {
    match std::fs::metadata(path) {
        Ok(m) if !m.is_file() => Source::Unreadable("The file can't be read."),
        Ok(m) if m.len() > MAX_IMPORT_FILE_BYTES => {
            Source::Unreadable("The file is too large to import.")
        }
        Ok(_) => {
            use std::io::Read;
            let Ok(file) = std::fs::File::open(path) else {
                return Source::Unreadable("The file can't be read.");
            };
            let mut bytes = Vec::new();
            match file.take(MAX_IMPORT_FILE_BYTES + 1).read_to_end(&mut bytes) {
                Ok(_) if bytes.len() as u64 > MAX_IMPORT_FILE_BYTES => {
                    Source::Unreadable("The file is too large to import.")
                }
                Ok(_) => Source::Bytes(bytes),
                Err(_) => Source::Unreadable("The file can't be read."),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Source::NotFound,
        Err(_) => Source::Unreadable("The file can't be read."),
    }
}

/// TablePlus's plist as the JSON `src-tauri` handed over
/// (`serde_json::to_value` of `plist::Value`), or `None` when it doesn't
/// decode or passes [`MAX_PLIST_DEPTH`] or [`MAX_PLIST_EVENTS`]. The bounds
/// are checked on the event stream (iterative) before the value is built,
/// so building and dropping it recurse at most that deep.
pub(crate) fn plist_json(bytes: &[u8]) -> Option<serde_json::Value> {
    let mut depth = 0usize;
    let mut events = 0usize;
    for event in plist::stream::Reader::new(Cursor::new(bytes)) {
        events += 1;
        if events > MAX_PLIST_EVENTS {
            return None;
        }
        match event.ok()? {
            plist::stream::Event::StartArray(_) | plist::stream::Event::StartDictionary(_) => {
                depth += 1;
                if depth > MAX_PLIST_DEPTH {
                    return None;
                }
            }
            plist::stream::Event::EndCollection => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    let value = plist::Value::from_reader(Cursor::new(bytes)).ok()?;
    serde_json::to_value(value).ok()
}

/// [`candidates_of`] on a blocking thread (review M2: the plist decode can
/// take a while on a large file).
async fn decode(source: ImportSource, bytes: Vec<u8>) -> ImportCandidates {
    tokio::task::spawn_blocking(move || candidates_of(source, &bytes))
        .await
        .unwrap_or_else(|_| ImportCandidates::unreadable("The file can't be read."))
}

/// The candidates in `bytes` for `source`, or why they can't be read.
fn candidates_of(source: ImportSource, bytes: &[u8]) -> ImportCandidates {
    match source {
        ImportSource::Tableplus => match plist_json(bytes) {
            Some(json) => ImportCandidates::from_result(tableplus_candidates(&json)),
            None => ImportCandidates::unreadable("The file isn't a TablePlus connections file."),
        },
        ImportSource::Dbeaver => ImportCandidates::from_result(dbeaver_candidates(bytes)),
    }
}

/// A candidate as `connectionCreate` takes it: local-only, renamed when its
/// name is taken, no password (the other tools' files bring none).
fn draft_of(c: &ImportCandidate, project_id: &str) -> ConnectionDraft {
    ConnectionDraft {
        project_id: project_id.to_string(),
        name: c.name.clone(),
        ty: c.ty.clone(),
        host: c.host.clone(),
        port: f64::from(c.port),
        database_name: c.database_name.clone(),
        username: c.username.clone(),
        ssl_mode: c.ssl_mode.clone(),
        connection_string: None,
        ssh_tunnel: c.ssh_tunnel.clone(),
        save_password: false,
        save_ssh_password: false,
        save_ssh_key_passphrase: false,
        label_ids: Vec::new(),
        is_local_only: Some(true),
        shared_connection_id: None,
        ai_share_schema: None,
        ai_share_data: None,
        active_ai_provider_id: None,
        active_ai_model: None,
        connected: false,
        rename_if_taken: true,
    }
}

impl Workspace {
    /// The file `path` names, else the source's default location under the
    /// Core's [`ImportPaths`] (none without them, or on a platform the tool
    /// doesn't run on).
    async fn import_source(&self, core: &Core, source: ImportSource, path: Option<&str>) -> Source {
        let file = match path {
            Some(p) => PathBuf::from(p),
            None => {
                let (Some(paths), Some(rel)) = (
                    core.import_paths.as_ref(),
                    default_path(source, std::env::consts::OS),
                ) else {
                    return Source::NotFound;
                };
                paths.home.join(rel)
            }
        };
        // Review M2: the read off the async threads.
        tokio::task::spawn_blocking(move || read_capped(&file))
            .await
            .unwrap_or(Source::Unreadable("The file can't be read."))
    }

    async fn project_must_exist(&self, project_id: &str) -> Result<()> {
        match projects::get(self.storage(), project_id).await? {
            Some(_) => Ok(()),
            None => Err(CoreError::new(PROJECT_NOT_FOUND, "Project not found.")),
        }
    }

    /// The import dialog's list (`importsCandidates`): read-only, so a
    /// workspace on read-only storage answers it.
    ///
    /// Errors: `NOT_SUPPORTED` without `LocalFiles`, `PROJECT_NOT_FOUND`,
    /// the storage codes. A missing or unreadable file is an answer.
    pub async fn import_candidates(
        &self,
        core: &Core,
        source: ImportSource,
        project_id: &str,
        path: Option<&str>,
    ) -> Result<ImportCandidates> {
        core.require_local_files()?;
        debug!(activity = "imports.candidates", source = source_name(source); "Read import candidates");
        self.project_must_exist(project_id).await?;
        let mut found = match self.import_source(core, source, path).await {
            Source::NotFound => return Ok(ImportCandidates::not_found()),
            Source::Unreadable(message) => return Ok(ImportCandidates::unreadable(message)),
            Source::Bytes(bytes) => decode(source, bytes).await,
        };
        if let Some(candidates) = found.candidates.as_mut() {
            let existing: Vec<ConnectionIdentity> =
                connections::list_in_project(self.storage(), project_id)
                    .await?
                    .iter()
                    .map(ConnectionIdentity::of)
                    .collect();
            mark_duplicates(candidates, &existing);
        }
        Ok(found)
    }

    /// Imports the candidates `keys` name (`importsCreate`). The file is
    /// read again, so a key is the file's, not the caller's.
    ///
    /// Errors: `NOT_SUPPORTED`, `PROJECT_NOT_FOUND`,
    /// `IMPORT_SOURCE_UNREADABLE` (the file is gone or doesn't read),
    /// `INVALID_ARGUMENT` (a key whose candidate has a problem, naming the
    /// key, or a candidate `connectionCreate` refuses), `ENGINE_NOT_AVAILABLE`,
    /// the storage codes (`STORAGE_READ_ONLY` on the CLI's storage). Nothing
    /// is stored on an error.
    pub async fn import_create(
        &self,
        core: &Core,
        origin: &WriteOrigin,
        source: ImportSource,
        project_id: &str,
        keys: &[String],
        path: Option<&str>,
    ) -> Result<Seqd<ImportOutcome>> {
        core.require_local_files()?;
        debug!(activity = "imports.create", source = source_name(source), keys = keys.len(); "Import connections");
        self.project_must_exist(project_id).await?;
        let found = match self.import_source(core, source, path).await {
            Source::NotFound => {
                return Err(CoreError::new(
                    IMPORT_SOURCE_UNREADABLE,
                    "The file to import from isn't there any more.",
                ))
            }
            Source::Unreadable(message) => {
                return Err(CoreError::new(IMPORT_SOURCE_UNREADABLE, message))
            }
            Source::Bytes(bytes) => decode(source, bytes).await,
        };
        let Some(candidates) = found.candidates else {
            return Err(CoreError::new(
                IMPORT_SOURCE_UNREADABLE,
                found
                    .unreadable
                    .unwrap_or_else(|| "The file can't be read.".to_string()),
            ));
        };
        if let Some((key, problem)) = refused_key(&candidates, keys) {
            return Err(CoreError::new(
                "INVALID_ARGUMENT",
                format!(
                    "The connection {key:?} can't be imported ({}).",
                    problem.as_str()
                ),
            ));
        }
        let limits = core.library_limits();
        let mut seen = HashSet::new();
        let mut wanted: Vec<(&str, Option<(&ImportCandidate, ConnectionDraft)>)> = Vec::new();
        for key in keys {
            if !seen.insert(key.as_str()) {
                continue;
            }
            let entry = candidates.iter().find(|c| c.key == *key).map(|c| {
                let draft = draft_of(c, project_id);
                (c, draft)
            });
            if let Some((_, draft)) = &entry {
                lib::check_connection_draft(draft, &limits)?;
                check_engine(core, &draft.ty)?;
            }
            wanted.push((key.as_str(), entry));
        }
        let now = now(core)?;

        let mut tx = self.storage().write().await?;
        let ticket = self.take_seq();
        let mut existing: Vec<ConnectionIdentity> =
            connections::list_in_project(&mut tx, project_id)
                .await?
                .iter()
                .map(ConnectionIdentity::of)
                .collect();
        let mut order = connection_order_in(&mut tx, project_id).await?;
        let mut results = Vec::with_capacity(wanted.len());
        let mut imported = Vec::new();
        for (key, entry) in wanted {
            let Some((candidate, draft)) = entry else {
                results.push(ImportKeyOutcome {
                    key: key.to_string(),
                    status: "notFound".to_string(),
                    id: None,
                    duplicate_of: None,
                });
                continue;
            };
            let mut probe = [candidate.clone()];
            mark_duplicates(&mut probe, &existing);
            if let Some(dup) = probe[0].duplicate_of.clone() {
                results.push(ImportKeyOutcome {
                    key: key.to_string(),
                    status: "duplicate".to_string(),
                    id: None,
                    duplicate_of: Some(dup),
                });
                continue;
            }
            let id = new_id(lib::CONNECTION_ID_PREFIX);
            let mut row = lib::connection_from_draft(id.clone(), &draft, &now);
            insert_connection_in(&mut tx, &mut row, true, &limits).await?;
            existing.push(ConnectionIdentity::of_candidate(id.clone(), candidate));
            order.push(id.clone());
            imported.push(id.clone());
            results.push(ImportKeyOutcome {
                key: key.to_string(),
                status: "imported".to_string(),
                id: Some(id),
                duplicate_of: None,
            });
        }
        if imported.is_empty() {
            drop(tx);
            drop(ticket);
            return Ok(Seqd::new(ImportOutcome { results }, self.change_seq()));
        }
        project_state::set_connection_order(&mut tx, project_id, &order).await?;
        let project_ticket = self.take_seq();
        tx.commit().await?;
        self.announce(
            ticket,
            StoredKind::Connection,
            Some(project_id.to_string()),
            Some(imported.clone()),
            origin,
        );
        let seq = self.announce(
            project_ticket,
            StoredKind::Project,
            None,
            Some(vec![project_id.to_string()]),
            origin,
        );
        info!(activity = "imports.create", source = source_name(source), imported = imported.len(), results = results.len(); "Imported connections");
        Ok(Seqd::new(ImportOutcome { results }, seq))
    }
}

fn source_name(source: ImportSource) -> &'static str {
    match source {
        ImportSource::Tableplus => "tableplus",
        ImportSource::Dbeaver => "dbeaver",
    }
}
