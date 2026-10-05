//! The working tree of a shared project (phase 5e): scanning a
//! project's `.seaquel` directory and applying the file operations Core's
//! projection plans, never through a symlink.
//!
//! Every path below the repo root is checked with
//! `seaquel_workspace::shared::names` (`.seaquel/` then `/`-separated
//! components; no `..`, `.`, empty or hidden component, no backslash or
//! control character, no Windows reserved name or trailing dot or space; at
//! most 1,024 bytes), and every component with `symlink_metadata`: a
//! symlink anywhere below the root is skipped on read and refused on write.
//! Files are opened with `O_NOFOLLOW` on Unix. The root itself (the user's
//! repo path) is trusted as given.
//!
//! - [`scan`] reads one project directory: the `.sql`, `.json`, `.yaml` and
//!   `.yml` files, at most [`ScanBounds::max_file_bytes`] each. A symlink,
//!   an unreadable directory, a file that isn't UTF-8, one past the size cap
//!   or with a name the path rules refuse is listed in `skipped`, so the
//!   planner never reads it as missing. Past [`ScanBounds::max_files`] files
//!   or [`ScanBounds::max_total_bytes`], nothing is read and the whole
//!   project is skipped.
//! - [`apply`] runs `FileOp`s: a write goes to a temp file in the same
//!   directory and is renamed over the target (atomic); a delete returns the
//!   bytes it removed, so Core can put them back ([`restore`]). Before
//!   either, the file on disk is checked against the op's `expect_hash`
//!   (`shared::file_hash`; `None` means no file may be there): a different
//!   file is [`OpOutcome::Stale`] and nothing is written.
//!
//! Nothing here logs a path, a name or a file's content.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use seaquel_workspace::shared::format::parse_project;
use seaquel_workspace::shared::names::{check_component, check_rel_path};
use seaquel_workspace::shared::{
    file_hash, DirScan, FileOp, RawFile, SkipReason, Skipped, MAX_PATH_BYTES, SEAQUEL_DIR,
};

use crate::GitError;

/// The code of every failure here: a path refused, a read or write that
/// failed. The message may name a path relative to `.seaquel/` (for the
/// GUI); it never reaches a log line.
pub const FILE_ERROR: &str = "FILE_ERROR";

/// A test hook (the projection replay's failing paths): called with the
/// absolute target before each write and delete; an `Err` fails it as an
/// I/O error would. Never set outside tests.
pub type WriteHook = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

/// The bounds on one scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanBounds {
    /// Files of the four extensions in one project directory.
    pub max_files: usize,
    /// One file's size; a larger one is skipped (`tooLarge`).
    pub max_file_bytes: u64,
    /// All the files read together.
    pub max_total_bytes: u64,
    /// Remove temp files of ours a crash left behind. Only a
    /// caller holding the repo's lock (a sync) sets it, so no write is in
    /// flight.
    pub clear_temp_files: bool,
}

impl Default for ScanBounds {
    fn default() -> Self {
        Self {
            max_files: 20_000,
            max_file_bytes: 16 * 1024 * 1024,
            max_total_bytes: 256 * 1024 * 1024,
            clear_temp_files: false,
        }
    }
}

/// How [`apply`] runs a list of operations.
#[derive(Default, Clone)]
pub struct ApplyOptions {
    /// A publish's sequence: after the first op that isn't done, the rest
    /// are [`OpOutcome::NotRun`]. A sync's writes are independent (false).
    pub stop_at_first_failure: bool,
    pub hook: Option<WriteHook>,
}

impl std::fmt::Debug for ApplyOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplyOptions")
            .field("stop_at_first_failure", &self.stop_at_first_failure)
            .field("hook", &self.hook.is_some())
            .finish()
    }
}

/// What happened to one [`FileOp`]. The messages may name the path relative
/// to `.seaquel/`, for the GUI; `Debug` prints them, so never log one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpOutcome {
    /// Written.
    Done,
    /// Deleted; the bytes it held (empty when there was no file).
    Deleted(Vec<u8>),
    /// The file on disk isn't what the plan expected (`expect_hash`):
    /// nothing was written.
    Stale,
    /// The path rules refused the path (a symlink on it, a bad component).
    Refused(String),
    /// The write or delete failed.
    Failed(String),
    /// Not attempted: an earlier op of a sequence didn't succeed.
    NotRun,
}

impl OpOutcome {
    pub fn is_success(&self) -> bool {
        matches!(self, OpOutcome::Done | OpOutcome::Deleted(_))
    }
}

/// A directory under `.seaquel/projects/`, with the name its
/// `project.yaml` (or `project.yml`) gives, else the directory's own.
#[derive(Clone, PartialEq, Eq)]
pub struct ProjectDir {
    pub dir: String,
    pub name: String,
    pub description: Option<String>,
}

impl std::fmt::Debug for ProjectDir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectDir").finish_non_exhaustive()
    }
}

fn file_error(message: impl Into<String>) -> GitError {
    GitError::new(FILE_ERROR, message)
}

/// A path relative to `.seaquel/`, as the GUI's messages name it.
fn shown(rel: &str) -> &str {
    rel.strip_prefix(SEAQUEL_DIR)
        .and_then(|r| r.strip_prefix('/'))
        .unwrap_or(rel)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, GitError> + Send + 'static,
) -> Result<T, GitError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| GitError::new("GIT_TASK_ERROR", format!("File task failed: {e}")))?
}

// ── Opening without following symlinks ──

#[cfg(unix)]
fn nofollow(opts: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    // O_NONBLOCK so a FIFO planted in the repo can't hang the open.
    opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

#[cfg(not(unix))]
fn nofollow(_opts: &mut OpenOptions) {}

/// What is at `path`, without following a symlink.
enum Kind {
    Missing,
    File(u64),
    Dir,
    Symlink,
    Other,
}

fn kind_of(path: &Path) -> std::io::Result<Kind> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Ok(Kind::Symlink),
        Ok(m) if m.is_dir() => Ok(Kind::Dir),
        Ok(m) if m.is_file() => Ok(Kind::File(m.len())),
        Ok(_) => Ok(Kind::Other),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(Kind::Missing),
        Err(e) if e.raw_os_error() == Some(not_a_dir()) => Ok(Kind::Missing),
        Err(e) => Err(e),
    }
}

#[cfg(unix)]
fn not_a_dir() -> i32 {
    libc::ENOTDIR
}

#[cfg(not(unix))]
fn not_a_dir() -> i32 {
    -1
}

/// Why reading a file failed.
enum ReadError {
    Symlink,
    TooLarge,
    Io,
}

/// The file at `path`, opened without following a symlink, at most `max`
/// bytes. `Ok(None)`: no file.
fn read_bytes(path: &Path, max: u64) -> Result<Option<Vec<u8>>, ReadError> {
    match kind_of(path).map_err(|_| ReadError::Io)? {
        Kind::Missing => return Ok(None),
        Kind::Symlink => return Err(ReadError::Symlink),
        Kind::File(len) if len > max => return Err(ReadError::TooLarge),
        Kind::File(_) => {}
        Kind::Dir | Kind::Other => return Err(ReadError::Io),
    }
    let mut opts = OpenOptions::new();
    opts.read(true);
    nofollow(&mut opts);
    let file = match opts.open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        // ELOOP: it became a symlink after the check.
        Err(e) if is_loop(&e) => return Err(ReadError::Symlink),
        Err(_) => return Err(ReadError::Io),
    };
    if !file.metadata().map_err(|_| ReadError::Io)?.is_file() {
        return Err(ReadError::Io);
    }
    let mut bytes = Vec::new();
    file.take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ReadError::Io)?;
    if bytes.len() as u64 > max {
        return Err(ReadError::TooLarge);
    }
    Ok(Some(bytes))
}

#[cfg(unix)]
fn is_loop(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_loop(_e: &std::io::Error) -> bool {
    false
}

/// Every component of `rel` below `root` that exists is a real directory
/// (the last may be anything but a symlink). `Err(message)` names the
/// first symlink.
fn check_no_symlink(root: &Path, rel: &str) -> Result<(), String> {
    let mut cur = root.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        cur.push(part);
        match kind_of(&cur) {
            Ok(Kind::Symlink) => {
                return Err(format!(
                    "{} is a symbolic link, which Seaquel doesn't follow.",
                    shown(&parts[..=i].join("/"))
                ))
            }
            Ok(Kind::Missing) => return Ok(()),
            Ok(Kind::Dir) => {}
            Ok(Kind::File(_) | Kind::Other) if i + 1 == parts.len() => {}
            Ok(Kind::File(_) | Kind::Other) => {
                return Err(format!("{} isn't a folder.", shown(&parts[..=i].join("/"))))
            }
            Err(_) => return Err(format!("{} can't be read.", shown(&parts[..=i].join("/")))),
        }
    }
    Ok(())
}

/// A repo-relative path a conflict resolution may write or delete:
/// relative, `/`-separated, no empty, `.` or `..` part, no backslash
/// or NUL, and no symlink at any component below `root`.
pub(crate) fn check_resolvable(root: &Path, rel: &str) -> Result<(), String> {
    let bad = rel.is_empty()
        || rel.starts_with('/')
        || rel.contains('\\')
        || rel.contains('\0')
        || Path::new(rel).is_absolute()
        || rel
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..");
    if bad {
        return Err("Not a valid path in the repository.".to_string());
    }
    check_no_symlink(root, rel)
}

/// Writes `bytes` to `root/rel` through a temp file opened without
/// following symlinks, renamed over the target. Call
/// [`check_resolvable`] first.
pub(crate) fn write_resolved(root: &Path, rel: &str, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic(&root.join(rel), bytes)
}

// ── Scanning ──

const EXTENSIONS: [&str; 4] = [".sql", ".json", ".yaml", ".yml"];

fn readable_ext(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    EXTENSIONS.iter().any(|e| lower.ends_with(e))
}

struct Walk {
    root: PathBuf,
    bounds: ScanBounds,
    files: Vec<(String, u64)>,
    skipped: Vec<Skipped>,
    entries: usize,
    /// Past a bound: the whole project is skipped with this reason.
    over: Option<SkipReason>,
}

impl Walk {
    fn skip(&mut self, rel: String, why: SkipReason) {
        self.skipped.push(Skipped { rel_path: rel, why });
    }

    fn walk(&mut self, rel_dir: &str) {
        if self.over.is_some() {
            return;
        }
        let entries = match std::fs::read_dir(self.root.join(rel_dir)) {
            Ok(entries) => entries,
            Err(_) => {
                self.skip(rel_dir.to_string(), SkipReason::Unreadable);
                return;
            }
        };
        let mut names: Vec<(String, Option<std::fs::FileType>)> = Vec::new();
        for entry in entries {
            let Ok(entry) = entry else {
                self.skip(rel_dir.to_string(), SkipReason::Unreadable);
                return;
            };
            self.entries += 1;
            if self.entries > self.bounds.max_files.saturating_mul(4) {
                self.over = Some(SkipReason::TooMany);
                return;
            }
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                // Not UTF-8: no row could name it. Skipped and named by
                // its lossy form only when it looks like a shared file.
                let lossy = name.to_string_lossy().into_owned();
                if readable_ext(&lossy) {
                    self.skip(format!("{rel_dir}/{lossy}"), SkipReason::NotUtf8);
                }
                continue;
            };
            // `file_type` doesn't follow a symlink.
            names.push((name.to_string(), entry.file_type().ok()));
        }
        names.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, ty) in names {
            if self.over.is_some() {
                return;
            }
            // Hidden entries (`.DS_Store`, `.git`) aren't part of the
            // projection. A temp file of ours that a crash left behind is
            // removed when the caller holds the repo's lock, so
            // a commit never stages it.
            if name.starts_with('.') {
                if self.bounds.clear_temp_files
                    && name.ends_with(TEMP_SUFFIX)
                    && ty.is_some_and(|t| t.is_file())
                {
                    let _ = std::fs::remove_file(self.root.join(rel_dir).join(&name));
                }
                continue;
            }
            let rel = format!("{rel_dir}/{name}");
            let Some(ty) = ty else {
                self.skip(rel, SkipReason::Unreadable);
                continue;
            };
            if ty.is_symlink() {
                self.skip(rel, SkipReason::Symlink);
                continue;
            }
            let named_ok = check_component(&name).is_ok() && rel.len() <= MAX_PATH_BYTES;
            if ty.is_dir() {
                if !named_ok {
                    self.skip(rel, SkipReason::Invalid);
                    continue;
                }
                self.walk(&rel);
            } else if ty.is_file() && readable_ext(&name) {
                if !named_ok {
                    self.skip(rel, SkipReason::Invalid);
                    continue;
                }
                self.files.push((rel, 0));
                if self.files.len() > self.bounds.max_files {
                    self.over = Some(SkipReason::TooMany);
                    return;
                }
            }
        }
    }
}

fn scan_blocking(root: &Path, dir: &str, bounds: ScanBounds) -> Result<DirScan, GitError> {
    check_rel_path(dir).map_err(|_| file_error("The project's folder isn't a valid path."))?;
    match kind_of(root) {
        Ok(Kind::Dir) => {}
        Ok(Kind::Symlink) if root.is_dir() => {}
        _ => return Err(file_error("The repository's folder isn't there.")),
    }
    let mut out = DirScan::default();
    // A symlink on the way to the project's directory: skipped whole.
    if check_no_symlink(root, dir).is_err() {
        out.skipped.push(Skipped {
            rel_path: dir.to_string(),
            why: SkipReason::Symlink,
        });
        return Ok(out);
    }
    match kind_of(&root.join(dir)) {
        Ok(Kind::Dir) => {}
        // No directory yet: nothing in it.
        Ok(Kind::Missing) => return Ok(out),
        Ok(Kind::Symlink) => {
            out.skipped.push(Skipped {
                rel_path: dir.to_string(),
                why: SkipReason::Symlink,
            });
            return Ok(out);
        }
        _ => {
            out.skipped.push(Skipped {
                rel_path: dir.to_string(),
                why: SkipReason::Unreadable,
            });
            return Ok(out);
        }
    }
    let mut walk = Walk {
        root: root.to_path_buf(),
        bounds,
        files: Vec::new(),
        skipped: Vec::new(),
        entries: 0,
        over: None,
    };
    walk.walk(dir);
    let whole = |why| DirScan {
        files: Vec::new(),
        skipped: vec![Skipped {
            rel_path: dir.to_string(),
            why,
        }],
        conflicted: false,
    };
    if let Some(why) = walk.over {
        return Ok(whole(why));
    }
    let mut total: u64 = 0;
    for (rel, _) in std::mem::take(&mut walk.files) {
        match read_bytes(&root.join(&rel), bounds.max_file_bytes) {
            Ok(Some(bytes)) => {
                total += bytes.len() as u64;
                if total > bounds.max_total_bytes {
                    return Ok(whole(SkipReason::TooLarge));
                }
                match String::from_utf8(bytes) {
                    Ok(text) => out.files.push(RawFile {
                        rel_path: rel,
                        text,
                    }),
                    Err(_) => walk.skip(rel, SkipReason::NotUtf8),
                }
            }
            // Gone since the listing: it isn't there.
            Ok(None) => {}
            Err(ReadError::Symlink) => walk.skip(rel, SkipReason::Symlink),
            Err(ReadError::TooLarge) => walk.skip(rel, SkipReason::TooLarge),
            Err(ReadError::Io) => walk.skip(rel, SkipReason::Unreadable),
        }
    }
    out.skipped = walk.skipped;
    Ok(out)
}

/// One project directory (`dir`, repo-relative: `.seaquel/projects/<dir>`)
/// of the repo at `root`, under the path rules (see the module docs).
/// `conflicted` is left false: [`conflicted`] reads it. An error only when
/// `dir` isn't a valid path or the repo's folder isn't there; anything
/// below that can't be read is skipped, never missing.
pub async fn scan(root: &Path, dir: &str, bounds: ScanBounds) -> Result<DirScan, GitError> {
    let (root, dir) = (root.to_path_buf(), dir.to_string());
    blocking(move || scan_blocking(&root, &dir, bounds)).await
}

/// Whether `path` is a folder (following symlinks: the repo path the user
/// chose may be one), on a blocking thread.
pub async fn is_dir(path: &Path) -> bool {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || path.is_dir())
        .await
        .unwrap_or(false)
}

/// The key a repo's lock is kept under: its canonical path,
/// resolved on a blocking thread. `fs::canonicalize` resolves symlinks and,
/// on macOS's case-insensitive volumes, returns each existing component's
/// on-disk case, so every spelling of one repo folder gets one key. A path
/// that doesn't exist is kept as given (nothing can be written there).
pub async fn lock_key(path: &Path) -> PathBuf {
    let given = path.to_path_buf();
    let fallback = given.clone();
    tokio::task::spawn_blocking(move || std::fs::canonicalize(&given).unwrap_or(given))
        .await
        .unwrap_or(fallback)
}

/// Whether the repo at `root` has conflicted files in its index.
/// A folder that isn't a repo has none.
pub async fn conflicted(root: &Path) -> Result<bool, GitError> {
    let root = root.to_path_buf();
    blocking(move || {
        let repo = match git2::Repository::open(&root) {
            Ok(repo) => repo,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(false),
            Err(e) => {
                return Err(GitError::new(
                    "REPO_OPEN_ERROR",
                    format!("Failed to open repository: {e}"),
                ))
            }
        };
        if repo.is_bare() {
            return Ok(false);
        }
        let index = repo
            .index()
            .map_err(|e| GitError::new("INDEX_ERROR", format!("Failed to get index: {e}")))?;
        Ok(index.has_conflicts())
    })
    .await
}

const PROJECTS: &str = ".seaquel/projects";

/// The directories under `.seaquel/projects/` (no symlinks, names Decision
/// 32 accepts), each with its `project.yaml`'s (or `project.yml`'s) name
/// and description, sorted by directory. None without that folder.
pub async fn project_dirs(root: &Path) -> Result<Vec<ProjectDir>, GitError> {
    Ok(project_dirs_listing(root).await?.dirs)
}

/// [`project_dirs`], and the entries of `.seaquel/projects` it left out
/// because they are symlinks, `rel_path` being the entry's
/// name, so the scan's answer can name them.
#[derive(Debug, Default)]
pub struct ProjectDirs {
    pub dirs: Vec<ProjectDir>,
    pub skipped: Vec<Skipped>,
}

/// See [`ProjectDirs`].
pub async fn project_dirs_listing(root: &Path) -> Result<ProjectDirs, GitError> {
    let root = root.to_path_buf();
    blocking(move || {
        let mut out = ProjectDirs::default();
        if check_no_symlink(&root, PROJECTS).is_err() {
            return Ok(out);
        }
        let entries = match std::fs::read_dir(root.join(PROJECTS)) {
            Ok(entries) => entries,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(out),
            Err(_) => return Err(file_error("The repository's projects can't be read.")),
        };
        for entry in entries.flatten() {
            let Ok(ty) = entry.file_type() else { continue };
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if check_component(&name).is_err() {
                continue;
            }
            if ty.is_symlink() {
                out.skipped.push(Skipped {
                    rel_path: name,
                    why: SkipReason::Symlink,
                });
                continue;
            }
            if !ty.is_dir() {
                continue;
            }
            let mut text = None;
            for file in ["project.yaml", "project.yml"] {
                let path = root.join(PROJECTS).join(&name).join(file);
                if let Ok(Some(bytes)) = read_bytes(&path, 1024 * 1024) {
                    text = String::from_utf8(bytes).ok();
                    if text.is_some() {
                        break;
                    }
                }
            }
            let p = parse_project(text.as_deref().unwrap_or(""), &name);
            out.dirs.push(ProjectDir {
                dir: name,
                name: p.name,
                description: p.description,
            });
        }
        out.dirs.sort_by(|a, b| a.dir.cmp(&b.dir));
        out.skipped.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(out)
    })
    .await
}

/// The text of the file at `rel` (repo-relative), or `None`
/// when there's none. A symlink on the path, a file that isn't UTF-8 or
/// one past 16 MiB is `FILE_ERROR`.
pub async fn read_file(root: &Path, rel: &str) -> Result<Option<String>, GitError> {
    let (root, rel) = (root.to_path_buf(), rel.to_string());
    blocking(move || {
        check_rel_path(&rel).map_err(|_| file_error("Not a valid path in the repository."))?;
        check_no_symlink(&root, &rel).map_err(file_error)?;
        match read_bytes(&root.join(&rel), ScanBounds::default().max_file_bytes) {
            Ok(None) => Ok(None),
            Ok(Some(bytes)) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| file_error(format!("{} isn't UTF-8 text.", shown(&rel)))),
            Err(ReadError::Symlink) => Err(file_error(format!(
                "{} is a symbolic link, which Seaquel doesn't follow.",
                shown(&rel)
            ))),
            Err(ReadError::TooLarge) => Err(file_error(format!("{} is too large.", shown(&rel)))),
            Err(ReadError::Io) => Err(file_error(format!("{} can't be read.", shown(&rel)))),
        }
    })
    .await
}

/// Every path below `dir` (files, folders and symlinks, not followed),
/// repo-relative, for picking a free file name. At most 100,000.
pub async fn paths_under(root: &Path, dir: &str) -> Result<Vec<String>, GitError> {
    let (root, dir) = (root.to_path_buf(), dir.to_string());
    blocking(move || {
        check_rel_path(&dir).map_err(|_| file_error("Not a valid path in the repository."))?;
        let mut out = Vec::new();
        if check_no_symlink(&root, &dir).is_err() {
            return Ok(out);
        }
        let mut stack = vec![dir];
        while let Some(rel) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(root.join(&rel)) else {
                continue;
            };
            for entry in entries.flatten() {
                if out.len() >= 100_000 {
                    return Ok(out);
                }
                let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                    continue;
                };
                let path = format!("{rel}/{name}");
                if entry
                    .file_type()
                    .is_ok_and(|t| t.is_dir() && !t.is_symlink())
                {
                    stack.push(path.clone());
                }
                out.push(path);
            }
        }
        Ok(out)
    })
    .await
}

// ── Writing ──

/// The file at `rel` is what the op expects: one with hash `want`, or none
/// for `None`.
fn as_expected(root: &Path, rel: &str, want: Option<&str>) -> Result<bool, OpOutcome> {
    let bytes = match read_bytes(&root.join(rel), ScanBounds::default().max_file_bytes) {
        Ok(bytes) => bytes,
        Err(ReadError::Symlink) => {
            return Err(OpOutcome::Refused(format!(
                "{} is a symbolic link, which Seaquel doesn't follow.",
                shown(rel)
            )))
        }
        // A file too large or unreadable isn't the one expected.
        Err(_) => return Ok(false),
    };
    Ok(match (bytes, want) {
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
        (Some(bytes), Some(want)) => String::from_utf8(bytes)
            .ok()
            .and_then(|text| file_hash(rel, &text))
            .is_some_and(|(h, _)| h == want),
    })
}

/// Creates the missing folders of `rel`'s parent, one at a time, each
/// checked not to be a symlink.
fn make_parents(root: &Path, rel: &str) -> Result<(), OpOutcome> {
    let parts: Vec<&str> = rel.split('/').collect();
    let mut cur = root.to_path_buf();
    for (i, part) in parts[..parts.len() - 1].iter().enumerate() {
        cur.push(part);
        match kind_of(&cur) {
            Ok(Kind::Dir) => {}
            Ok(Kind::Missing) => match std::fs::create_dir(&cur) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::AlreadyExists && cur.is_dir() => {}
                Err(_) => {
                    return Err(OpOutcome::Failed(format!(
                        "The folder {} couldn't be made.",
                        shown(&parts[..=i].join("/"))
                    )))
                }
            },
            Ok(Kind::Symlink) => {
                return Err(OpOutcome::Refused(format!(
                    "{} is a symbolic link, which Seaquel doesn't follow.",
                    shown(&parts[..=i].join("/"))
                )))
            }
            _ => {
                return Err(OpOutcome::Failed(format!(
                    "{} isn't a folder.",
                    shown(&parts[..=i].join("/"))
                )))
            }
        }
    }
    Ok(())
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The suffix of [`write_atomic`]'s temp files.
const TEMP_SUFFIX: &str = ".seaquel-tmp";

/// Flushes a folder's entries after a rename, so the new name survives a
/// crash (Unix; best effort).
#[cfg(unix)]
fn sync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) {}

/// Writes `bytes` to a new temp file next to `target`, then renames it over
/// `target`.
fn write_atomic(target: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = target.parent().unwrap_or(Path::new("."));
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (tmp, mut file) = loop {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.{}-{n}{TEMP_SUFFIX}", std::process::id()));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        nofollow(&mut opts);
        match opts.open(&tmp) {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    };
    // A rewrite keeps the file's mode (only a regular file's;
    // the target was checked not to be a symlink).
    let mode = std::fs::symlink_metadata(target)
        .ok()
        .filter(std::fs::Metadata::is_file)
        .map(|m| m.permissions());
    let written = (|| {
        file.write_all(bytes)?;
        if let Some(mode) = mode {
            file.set_permissions(mode)?;
        }
        file.sync_all()?;
        drop(file);
        std::fs::rename(&tmp, target)?;
        sync_dir(dir);
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

fn apply_one(root: &Path, op: &FileOp, hook: Option<&WriteHook>) -> OpOutcome {
    let rel = match op {
        FileOp::Write { rel_path, .. } | FileOp::Delete { rel_path, .. } => rel_path,
    };
    if check_rel_path(rel).is_err() {
        return OpOutcome::Refused("Not a valid path in the repository.".to_string());
    }
    if let Err(message) = check_no_symlink(root, rel) {
        return OpOutcome::Refused(message);
    }
    let expect = match op {
        FileOp::Write { expect_hash, .. } | FileOp::Delete { expect_hash, .. } => {
            expect_hash.as_deref()
        }
    };
    match as_expected(root, rel, expect) {
        Ok(true) => {}
        Ok(false) => return OpOutcome::Stale,
        Err(outcome) => return outcome,
    }
    let target = root.join(rel);
    match op {
        FileOp::Write { text, .. } => {
            if let Err(outcome) = make_parents(root, rel) {
                return outcome;
            }
            if let Some(hook) = hook {
                if let Err(e) = hook(&target) {
                    return OpOutcome::Failed(format!("{} couldn't be written: {e}", shown(rel)));
                }
            }
            match write_atomic(&target, text.as_bytes()) {
                Ok(()) => OpOutcome::Done,
                Err(e) => {
                    OpOutcome::Failed(format!("{} couldn't be written: {}", shown(rel), e.kind()))
                }
            }
        }
        FileOp::Delete { .. } => {
            if let Some(hook) = hook {
                if let Err(e) = hook(&target) {
                    return OpOutcome::Failed(format!("{} couldn't be deleted: {e}", shown(rel)));
                }
            }
            let bytes = match read_bytes(&target, ScanBounds::default().max_file_bytes) {
                Ok(bytes) => bytes.unwrap_or_default(),
                Err(_) => {
                    return OpOutcome::Failed(format!("{} can't be read.", shown(rel)));
                }
            };
            match std::fs::remove_file(&target) {
                Ok(()) => OpOutcome::Deleted(bytes),
                Err(e) if e.kind() == ErrorKind::NotFound => OpOutcome::Deleted(Vec::new()),
                Err(e) => {
                    OpOutcome::Failed(format!("{} couldn't be deleted: {}", shown(rel), e.kind()))
                }
            }
        }
    }
}

/// Runs `ops` against the repo at `root` (see the module docs), one
/// outcome per op, in order.
pub async fn apply(
    root: &Path,
    ops: Vec<FileOp>,
    options: ApplyOptions,
) -> Result<Vec<OpOutcome>, GitError> {
    let root = root.to_path_buf();
    blocking(move || {
        let mut out = Vec::with_capacity(ops.len());
        let mut stopped = false;
        for op in &ops {
            if stopped {
                out.push(OpOutcome::NotRun);
                continue;
            }
            let outcome = apply_one(&root, op, options.hook.as_ref());
            if options.stop_at_first_failure && !outcome.is_success() {
                stopped = true;
            }
            out.push(outcome);
        }
        Ok(out)
    })
    .await
}

/// Puts back the bytes a delete removed (a row write that
/// failed after its file was deleted). Atomic, under the same path rules.
pub async fn restore(root: &Path, rel: &str, bytes: Vec<u8>) -> Result<(), GitError> {
    let (root, rel) = (root.to_path_buf(), rel.to_string());
    blocking(move || {
        check_rel_path(&rel).map_err(|_| file_error("Not a valid path in the repository."))?;
        check_no_symlink(&root, &rel).map_err(file_error)?;
        if let Err(outcome) = make_parents(&root, &rel) {
            return Err(file_error(format!("{outcome:?}")));
        }
        write_atomic(&root.join(&rel), &bytes).map_err(|e| {
            file_error(format!(
                "{} couldn't be put back: {}",
                shown(&rel),
                e.kind()
            ))
        })
    })
    .await
}
