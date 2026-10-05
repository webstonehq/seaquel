//! Finding the helper, checking its folder, starting it, and the handshake.
//!
//! What can go wrong before the database opens is `ENGINE_NOT_INSTALLED`,
//! at once, so the interface offers the download: no file, a file or folder
//! others can write, a symlink, a helper that doesn't start or answers with
//! another protocol or app version. A helper that doesn't answer `hello` in
//! time is `ENGINE_UNAVAILABLE` instead (a download wouldn't help): within
//! [`BOUNDS`]`.first`, and a file this process hasn't started yet gets one
//! more try within [`BOUNDS`]`.retry` (macOS checks a new binary on its
//! first exec). A helper that fails the handshake is killed and reaped
//! before the error returns. Errors and logs never carry the path, and
//! strings the helper sent are cut to [`MAX_HELPER_TEXT`] characters.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use log::{info, warn};
use seaquel_engine::{ConnectConfig, DbError};
use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::JoinSet;
use tokio::time::{timeout, Instant};

use super::HelperLocator;
use crate::wire::{
    read_frame_async, write_frame_async, FrameKind, OpenParams, Reply, Request, PROTOCOL,
};

/// How long the helper has to start and answer `hello`: `first`, and for
/// a file this process hasn't started yet, one more try within `retry`.
#[derive(Clone, Copy)]
pub(super) struct Bounds {
    pub first: Duration,
    pub retry: Duration,
}

pub(super) const BOUNDS: Bounds = Bounds {
    first: Duration::from_secs(5),
    retry: Duration::from_secs(20),
};

/// The most characters of a string the helper sent (a version, a code)
/// that an error repeats.
const MAX_HELPER_TEXT: usize = 64;

/// A string the helper sent, cut to [`MAX_HELPER_TEXT`] characters.
fn helper_text(text: &str) -> String {
    match text.char_indices().nth(MAX_HELPER_TEXT) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// The helper didn't answer `hello` in time.
fn unavailable() -> DbError {
    DbError {
        message: "The DuckDB helper didn't start in time".to_string(),
        code: "ENGINE_UNAVAILABLE".to_string(),
    }
}

/// How long a helper that was killed has to be reaped before its error
/// returns anyway.
const REAP_WAIT: Duration = Duration::from_secs(1);

/// The engine's name in `ENGINE_NOT_INSTALLED`.
const ENGINE: &str = "DuckDB";

/// The `hello` and `open` call ids. Later calls start after them.
pub(super) const HELLO_CALL: u32 = 0;
pub(super) const OPEN_CALL: u32 = 1;

fn not_installed(locator: &HelperLocator, reason: impl std::fmt::Display) -> DbError {
    DbError::engine_not_installed(ENGINE, &locator.version, reason)
}

/// Where the helper for `locator` is: `<dir>/<version>/seaquel-duckdb`.
pub(super) fn helper_path(locator: &HelperLocator) -> PathBuf {
    locator.dir.join(&locator.version).join(helper_file_name())
}

/// `seaquel-duckdb[.exe]`.
pub(super) fn helper_file_name() -> String {
    format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX)
}

/// The helper's path, once it is there and safe to run: a regular file, no
/// symlink on the way from `<identifier>` (the parent of `bin`, the parent
/// of `dir`) down, and every one of them owned by this user and writable by
/// nobody else. On Unix that is the owner's uid and no group or world
/// write, and the file must be executable by its owner; on Windows the owner is the user,
/// Administrators or SYSTEM, no entry of the DACL lets anyone but the user,
/// SYSTEM or Administrators write, append, delete or change the DACL or
/// owner (on `<identifier>` only delete, delete a child or change the DACL
/// or owner: others may add files there, as Unix allows anything but group
/// and world write), and no level is a symlink or junction
/// (`seaquel_runtime::acl`). On Windows the private version folder also
/// guards the DLL search order: the helper's own folder is searched first
/// for the DLLs it loads, so nobody else can plant one there. It doesn't hash the
/// file: anyone who can write that folder can already replace the terminal
/// binary itself. Folders above `<identifier>` aren't checked, and the
/// helper is then started by path, so a folder above it that someone else
/// can write is a gap.
pub(super) fn check(locator: &HelperLocator) -> Result<PathBuf, DbError> {
    let path = helper_path(locator);
    let unsafe_permissions =
        || not_installed(locator, "the DuckDB helper's folder has unsafe permissions");
    let file = match std::fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(not_installed(locator, "the DuckDB helper isn't there"))
        }
        Err(e) => {
            return Err(not_installed(
                locator,
                format!("the DuckDB helper can't be read ({:?})", e.kind()),
            ))
        }
    };
    if !file.file_type().is_file() {
        // A symlink, or a folder in its place.
        return Err(unsafe_permissions());
    }
    if !owned_and_private(&path, &file, false, false) || !runnable(&file) {
        return Err(unsafe_permissions());
    }
    let version_dir = path.parent().unwrap_or(&locator.dir);
    let bin = locator.dir.parent();
    let identifier = bin.and_then(Path::parent);
    let folders = [Some(version_dir), Some(&*locator.dir), bin, identifier];
    for folder in folders.into_iter().flatten() {
        if folder.as_os_str().is_empty() {
            continue;
        }
        let meta = std::fs::symlink_metadata(folder).map_err(|_| unsafe_permissions())?;
        let root = Some(folder) == identifier;
        if !meta.file_type().is_dir() || !owned_and_private(folder, &meta, true, root) {
            return Err(unsafe_permissions());
        }
    }
    Ok(path)
}

/// Owned by this user and writable by nobody else. `dir`: a folder is
/// expected (Windows reads the kind again through its own handle). `root`:
/// `<identifier>`, judged by Windows' narrower root rule (Unix's mode rule
/// is already "no group or world write" at every level).
#[cfg(unix)]
fn owned_and_private(_path: &Path, meta: &std::fs::Metadata, _dir: bool, _root: bool) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: `geteuid` has no preconditions and can't fail.
    let me = unsafe { libc::geteuid() };
    meta.uid() == me && meta.mode() & 0o022 == 0
}

/// The owner may execute the file: a helper that lost the
/// bit (a restore, a copy tool) is refused as unsafe, so the status says
/// `Unsafe` and an install replaces it instead of keeping a file no start
/// can run. Windows has no execute bit.
#[cfg(unix)]
fn runnable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    meta.mode() & 0o100 != 0
}

#[cfg(not(unix))]
fn runnable(_meta: &std::fs::Metadata) -> bool {
    true
}

/// Windows: read through a handle that doesn't follow a reparse
/// point, so what is judged is what is there. Anything that can't be read
/// counts as unsafe.
#[cfg(windows)]
fn owned_and_private(path: &Path, _meta: &std::fs::Metadata, dir: bool, root: bool) -> bool {
    use seaquel_runtime::acl::{self, Level};
    let Ok(user) = acl::current_user() else {
        return false;
    };
    let level = if root { Level::Root } else { Level::Private };
    acl::inspect(path).is_ok_and(|entry| entry.problem(&user, dir, level).is_none())
}

#[cfg(not(any(unix, windows)))]
fn owned_and_private(_path: &Path, _meta: &std::fs::Metadata, _dir: bool, _root: bool) -> bool {
    true
}

/// The helper's process. Dropping it kills the helper (what tokio's
/// `kill_on_drop` does, kept by hand), unless it was [`detached`]: a helper
/// that took `close` and is still closing its database is left to finish.
/// tokio reaps a dropped child that
/// exits later.
///
/// [`detached`]: HelperChild::detach
pub(super) struct HelperChild {
    /// `None` only once detached.
    child: Option<Child>,
}

impl HelperChild {
    pub(super) fn new(child: Child) -> Self {
        HelperChild { child: Some(child) }
    }

    /// Lets the helper go: the child, which no longer kills it when
    /// dropped.
    pub(super) fn detach(mut self) -> Child {
        self.child.take().expect("detached once")
    }
}

impl std::ops::Deref for HelperChild {
    type Target = Child;
    fn deref(&self) -> &Child {
        self.child.as_ref().expect("not detached")
    }
}

impl std::ops::DerefMut for HelperChild {
    fn deref_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("not detached")
    }
}

impl Drop for HelperChild {
    fn drop(&mut self) {
        if let Some(child) = &mut self.child {
            // Nothing to do for a helper that already exited.
            let _ = child.start_kill();
        }
    }
}

/// A helper that answered `hello` and opened the database.
pub(super) struct Started {
    pub child: HelperChild,
    pub stdin: ChildStdin,
    pub stdout: BufReader<ChildStdout>,
    /// The stderr drain, already running.
    pub tasks: JoinSet<()>,
    /// Bytes the helper wrote to stderr (counted, never read).
    pub stderr_bytes: Arc<AtomicU64>,
    pub started: Instant,
}

/// Starts the helper at `path`, checks its `hello` and opens the database.
/// Dropping the future kills the helper ([`HelperChild`]).
pub(super) async fn start(
    path: &Path,
    locator: &HelperLocator,
    config: &ConnectConfig,
) -> Result<Started, DbError> {
    start_with(path, locator, config, BOUNDS).await
}

/// The files this process has started and heard `hello` from, by
/// [`identity`]. A silent start of one of them isn't retried.
static ANSWERED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Serializes the spawns of helpers in this process. On
/// macOS std creates a child's pipes with `pipe()` and only then marks them
/// close-on-exec, so a helper spawned on another thread in between would
/// inherit this helper's pipe ends, and this helper would never see EOF on
/// stdin when its connection closes. Spawns elsewhere in the process (an
/// `$EDITOR`) aren't covered; they don't spawn helpers concurrently with
/// one another, and a helper still exits on `close` or a kill. On Windows
/// std already holds a lock of its own around `CreateProcess` and the
/// inheritable pipe handles, so this one is Unix only.
#[cfg(unix)]
static SPAWN: Mutex<()> = Mutex::new(());

/// What tells one install of the file at `path` from another: a changed
/// file is a new install and gets the retry again.
fn identity(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(format!(
            "{}:{}:{}:{}:{}.{}",
            path.display(),
            meta.dev(),
            meta.ino(),
            meta.len(),
            meta.mtime(),
            meta.mtime_nsec()
        ))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Some(format!(
            "{}:{}:{}",
            path.display(),
            meta.len(),
            meta.last_write_time()
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        Some(format!("{}:{}", path.display(), meta.len()))
    }
}

fn answered(identity: &Option<String>) -> bool {
    identity.as_ref().is_some_and(|id| {
        ANSWERED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(id)
    })
}

/// [`start`] with the handshake's bounds.
pub(super) async fn start_with(
    path: &Path,
    locator: &HelperLocator,
    config: &ConnectConfig,
    bounds: Bounds,
) -> Result<Started, DbError> {
    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|_| DbError::connection_error("the DuckDB helper needs a tokio runtime"))?;
    let started = Instant::now();
    let identity = identity(path);
    let greeted = match greet(path, locator, &runtime, bounds.first).await? {
        Some(greeted) => greeted,
        None if !answered(&identity) => {
            info!(activity = "duckdb.helper", event = "spawn", retry = true; "the DuckDB helper's first start was slow; trying once more");
            greet(path, locator, &runtime, bounds.retry)
                .await?
                .ok_or_else(unavailable)?
        }
        None => return Err(unavailable()),
    };
    if let Some(id) = identity {
        let mut seen = ANSWERED.lock().unwrap_or_else(PoisonError::into_inner);
        if !seen.contains(&id) {
            seen.push(id);
        }
    }
    let Greeted {
        mut child,
        mut stdin,
        mut stdout,
        tasks,
        stderr_bytes,
    } = greeted;

    // The database: nothing else is sent until it is open.
    if let Err(e) = open(&mut stdin, &mut stdout, config).await {
        let error = match e {
            Opening::Refused(error) => error,
            Opening::Broken(why) => {
                let status = kill(&mut child).await;
                DbError::connection_error(format!(
                    "the DuckDB helper stopped while opening the database ({why}; {})",
                    describe(&status)
                ))
            }
        };
        // A refused open leaves the helper waiting; it goes with `child`.
        drop(stdin);
        let _ = kill(&mut child).await;
        return Err(error);
    }
    info!(activity = "duckdb.helper", event = "ready", ms = started.elapsed().as_millis() as u64; "DuckDB helper ready");
    Ok(Started {
        child,
        stdin,
        stdout,
        tasks,
        stderr_bytes,
        started,
    })
}

/// A helper that answered `hello`.
struct Greeted {
    child: HelperChild,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    tasks: JoinSet<()>,
    stderr_bytes: Arc<AtomicU64>,
}

/// Spawns the helper and checks its `hello` within `bound`. `Ok(None)`: it
/// didn't answer in time (it is killed).
async fn greet(
    path: &Path,
    locator: &HelperLocator,
    runtime: &tokio::runtime::Handle,
    bound: Duration,
) -> Result<Option<Greeted>, DbError> {
    let mut command = Command::new(path);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Seaquel's test hooks (`SEAQUEL_TEST_*`, `SEAQUEL_CLI_TEST_*`, …) stay
    // out of the helper; everything else is inherited.
    for (key, _) in std::env::vars_os() {
        if let Some(name) = key.to_str() {
            if name.starts_with("SEAQUEL_") && name.contains("_TEST_") {
                command.env_remove(&key);
            }
        }
    }
    // Started from an AppImage, the helper gets none of its mount's
    // libraries; logged by name only.
    #[cfg(unix)]
    for (name, value) in appimage_env(&|name| std::env::var_os(name)) {
        info!(activity = "duckdb.helper", event = "spawn", env = name, removed = value.is_none(); "kept the AppImage's libraries from the DuckDB helper");
        match value {
            Some(value) => command.env(name, value),
            None => command.env_remove(name),
        };
    }
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let spawned = {
        #[cfg(unix)]
        let _spawning = SPAWN.lock().unwrap_or_else(PoisonError::into_inner);
        command.spawn()
    };
    let mut child = HelperChild::new(spawned.map_err(|e| {
        warn!(activity = "duckdb.helper", event = "spawn", error_kind = format!("{:?}", e.kind()).as_str(); "the DuckDB helper didn't start");
        not_installed(
            locator,
            format!("the DuckDB helper didn't start ({:?})", e.kind()),
        )
    })?);
    let (Some(mut stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(not_installed(locator, "the DuckDB helper has no pipes"));
    };
    let stderr_bytes = Arc::new(AtomicU64::new(0));
    let mut tasks = JoinSet::new();
    tasks.spawn_on(drain(stderr, stderr_bytes.clone()), runtime);
    let mut stdout = BufReader::with_capacity(64 * 1024, stdout);

    let hello = handshake(&mut stdin, &mut stdout, locator);
    match timeout(bound, hello).await {
        Ok(Ok(())) => Ok(Some(Greeted {
            child,
            stdin,
            stdout,
            tasks,
            stderr_bytes,
        })),
        Ok(Err(reason)) => {
            let status = kill(&mut child).await;
            warn!(activity = "duckdb.helper", event = "spawn", status = describe(&status).as_str(); "the DuckDB helper failed its handshake");
            Err(not_installed(locator, reason))
        }
        Err(_) => {
            let status = kill(&mut child).await;
            warn!(activity = "duckdb.helper", event = "spawn", status = describe(&status).as_str(), ms = bound.as_millis() as u64; "the DuckDB helper didn't answer in time");
            Ok(None)
        }
    }
}

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What to change in the helper's environment when this process runs from
/// an AppImage (`APPIMAGE` set): the entries
/// of `LD_LIBRARY_PATH` and `LD_PRELOAD` under `$APPDIR`, the AppImage's
/// mount, are dropped. The helper links the system's libraries, not the
/// AppImage's, and it can outlive the mount by up to a minute while it
/// checkpoints. Each change is a variable and its new value, `None` to
/// remove it; a variable with nothing under `$APPDIR` is left alone, and
/// so is everything when `APPIMAGE` or `APPDIR` isn't set. `var` reads the
/// environment (a function, for the tests).
#[cfg(unix)]
pub(super) fn appimage_env(
    var: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<(&'static str, Option<std::ffi::OsString>)> {
    use std::ffi::OsString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let (Some(_), Some(appdir)) = (var("APPIMAGE"), var("APPDIR")) else {
        return Vec::new();
    };
    let appdir = PathBuf::from(appdir);
    if appdir.as_os_str().is_empty() || !appdir.is_absolute() {
        return Vec::new();
    }
    // `Path::starts_with` compares whole components: `/tmp/.mount_ab`
    // isn't under `/tmp/.mount_a`.
    let inside = |entry: &[u8]| Path::new(std::ffi::OsStr::from_bytes(entry)).starts_with(&appdir);
    let mut changes = Vec::new();
    // `ld.so` splits `LD_LIBRARY_PATH` on `:` (an empty entry is the
    // current folder, kept), and `LD_PRELOAD` on `:` and spaces.
    for (name, separators) in [("LD_LIBRARY_PATH", &b":"[..]), ("LD_PRELOAD", &b": "[..])] {
        let Some(value) = var(name) else { continue };
        let bytes = value.as_bytes();
        let entries: Vec<&[u8]> = bytes
            .split(|b| separators.contains(b))
            .filter(|e| name == "LD_LIBRARY_PATH" || !e.is_empty())
            .collect();
        if !entries.iter().any(|e| !e.is_empty() && inside(e)) {
            continue;
        }
        let kept: Vec<&[u8]> = entries
            .into_iter()
            .filter(|e| e.is_empty() || !inside(e))
            .collect();
        let kept_any = kept.iter().any(|e| !e.is_empty());
        changes.push((name, kept_any.then(|| OsString::from_vec(kept.join(&b':')))));
    }
    changes
}

/// `hello` and its answer. The `Err` is the reason the helper can't be
/// used, worded for `ENGINE_NOT_INSTALLED`.
async fn handshake(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    locator: &HelperLocator,
) -> Result<(), String> {
    let hello = Request::Hello {
        protocol: PROTOCOL,
        version: locator.version.clone(),
    }
    .encode()
    .map_err(|e| e.message)?;
    write_frame_async(stdin, FrameKind::Control, HELLO_CALL, &hello)
        .await
        .map_err(|e| {
            format!(
                "the DuckDB helper didn't take its first frame ({:?})",
                e.kind()
            )
        })?;
    let frame = match read_frame_async(stdout).await {
        Ok(Some(frame)) => frame,
        Ok(None) => return Err("the DuckDB helper stopped before answering".to_string()),
        Err(e) => {
            return Err(format!(
                "the DuckDB helper's answer couldn't be read ({:?})",
                e.kind()
            ))
        }
    };
    if frame.kind != FrameKind::Control || frame.call != HELLO_CALL {
        return Err("the DuckDB helper answered out of turn".to_string());
    }
    match Reply::decode(&frame.payload) {
        Ok(Reply::HelloOk {
            protocol, version, ..
        }) => {
            if protocol != PROTOCOL {
                Err(format!(
                    "the DuckDB helper speaks protocol {protocol}, not {PROTOCOL}"
                ))
            } else if version != locator.version {
                Err(format!(
                    "the DuckDB helper installed is for Seaquel {}",
                    helper_text(&version)
                ))
            } else {
                Ok(())
            }
        }
        Ok(Reply::Error { error, .. }) => Err(format!(
            "the DuckDB helper refused this version of Seaquel ({})",
            helper_text(&error.code)
        )),
        Ok(_) | Err(_) => Err("the DuckDB helper's answer isn't a hello".to_string()),
    }
}

/// How `open` failed.
enum Opening {
    /// The helper answered with an error (a missing file, a refused
    /// option): the connect's error.
    Refused(DbError),
    /// The wire broke.
    Broken(String),
}

async fn open(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    config: &ConnectConfig,
) -> Result<(), Opening> {
    let open = Request::Open(OpenParams::of(config))
        .encode()
        .map_err(Opening::Refused)?;
    write_frame_async(stdin, FrameKind::Control, OPEN_CALL, &open)
        .await
        .map_err(|e| Opening::Broken(format!("{:?}", e.kind())))?;
    let frame = match read_frame_async(stdout).await {
        Ok(Some(frame)) => frame,
        Ok(None) => return Err(Opening::Broken("no answer".to_string())),
        Err(e) => return Err(Opening::Broken(format!("{:?}", e.kind()))),
    };
    if frame.kind != FrameKind::Control || frame.call != OPEN_CALL {
        return Err(Opening::Broken("an answer out of turn".to_string()));
    }
    match Reply::decode(&frame.payload) {
        Ok(Reply::Opened) => Ok(()),
        Ok(Reply::Error { error, .. }) => Err(Opening::Refused(error)),
        Ok(_) => Err(Opening::Broken("an answer that isn't open's".to_string())),
        Err(e) => Err(Opening::Broken(e.message)),
    }
}

/// Reads the helper's stderr to its end and counts it. The text is never
/// looked at: a panic message can quote DuckDB's error, which can quote SQL.
async fn drain(mut stderr: tokio::process::ChildStderr, bytes: Arc<AtomicU64>) {
    let mut buffer = vec![0u8; 8 * 1024];
    while let Ok(n) = stderr.read(&mut buffer).await {
        if n == 0 {
            break;
        }
        bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
}

/// Kills the helper and reaps it, waiting at most [`REAP_WAIT`].
pub(super) async fn kill(child: &mut Child) -> io::Result<ExitStatus> {
    let _ = child.start_kill();
    match timeout(REAP_WAIT, child.wait()).await {
        Ok(status) => status,
        Err(_) => Err(io::ErrorKind::TimedOut.into()),
    }
}

/// How the helper ended: `signal 9`, `exit code 4`.
pub(super) fn describe(status: &io::Result<ExitStatus>) -> String {
    match status {
        Ok(status) => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if let Some(signal) = status.signal() {
                    return format!("signal {signal}");
                }
            }
            match status.code() {
                Some(code) => format!("exit code {code}"),
                None => "no exit code".to_string(),
            }
        }
        Err(e) => format!("its exit wasn't seen: {:?}", e.kind()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn locator(dir: &Path) -> HelperLocator {
        HelperLocator {
            dir: dir.join("bin").join("duckdb"),
            version: "2026.1.1".to_string(),
        }
    }

    #[test]
    fn the_helper_lives_in_its_version_folder() {
        let l = locator(Path::new("/data"));
        let name = format!("seaquel-duckdb{}", std::env::consts::EXE_SUFFIX);
        assert_eq!(
            helper_path(&l),
            Path::new("/data/bin/duckdb/2026.1.1").join(name)
        );
    }

    /// Spike S3: from an AppImage, library
    /// paths that point into the AppImage's mount (`$APPDIR`) are dropped
    /// from the helper's environment, since the helper links the system's
    /// libraries and outlives the mount by up to a minute. Everything else
    /// stays, and nothing changes outside an AppImage.
    #[cfg(unix)]
    #[test]
    fn appimage_library_paths_are_kept_from_the_helper() {
        use std::ffi::OsString;
        fn env(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
            move |name| {
                vars.iter()
                    .find(|(k, _)| *k == name)
                    .map(|(_, v)| OsString::from(v))
            }
        }
        assert!(
            appimage_env(&env(&[
                ("APPDIR", "/tmp/.mount_SeaqAb"),
                ("LD_LIBRARY_PATH", "/tmp/.mount_SeaqAb/usr/lib"),
            ]))
            .is_empty(),
            "not an AppImage: APPIMAGE isn't set"
        );
        let fixes = appimage_env(&env(&[
            ("APPIMAGE", "/home/u/Seaquel.AppImage"),
            ("APPDIR", "/tmp/.mount_SeaqAb"),
            (
                "LD_LIBRARY_PATH",
                "/tmp/.mount_SeaqAb/usr/lib:/opt/x/lib::/tmp/.mount_SeaqAb:/tmp/.mount_SeaqAbc/lib",
            ),
            (
                "LD_PRELOAD",
                "/tmp/.mount_SeaqAb/usr/lib/libx.so /usr/lib/liby.so",
            ),
        ]));
        assert_eq!(
            fixes,
            vec![
                (
                    "LD_LIBRARY_PATH",
                    Some(OsString::from("/opt/x/lib::/tmp/.mount_SeaqAbc/lib"))
                ),
                ("LD_PRELOAD", Some(OsString::from("/usr/lib/liby.so"))),
            ]
        );
        // Every entry in the mount: the variable goes.
        let fixes = appimage_env(&env(&[
            ("APPIMAGE", "/home/u/Seaquel.AppImage"),
            ("APPDIR", "/tmp/.mount_SeaqAb/"),
            (
                "LD_LIBRARY_PATH",
                "/tmp/.mount_SeaqAb/usr/lib:/tmp/.mount_SeaqAb/lib",
            ),
            ("LD_PRELOAD", "/tmp/.mount_SeaqAb/usr/lib/libx.so"),
        ]));
        assert_eq!(fixes, vec![("LD_LIBRARY_PATH", None), ("LD_PRELOAD", None)]);
        // Nothing in the mount, or no APPDIR to tell: nothing changes.
        assert!(appimage_env(&env(&[
            ("APPIMAGE", "/home/u/Seaquel.AppImage"),
            ("APPDIR", "/tmp/.mount_SeaqAb"),
            ("LD_LIBRARY_PATH", "/opt/x/lib"),
        ]))
        .is_empty());
        assert!(appimage_env(&env(&[
            ("APPIMAGE", "/home/u/Seaquel.AppImage"),
            ("LD_LIBRARY_PATH", "/tmp/.mount_SeaqAb/usr/lib"),
        ]))
        .is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn exits_are_described_by_signal_or_code() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(describe(&Ok(ExitStatus::from_raw(9))), "signal 9");
        assert_eq!(describe(&Ok(ExitStatus::from_raw(4 << 8))), "exit code 4");
    }

    /// See `tests/remote.rs`'s `FRAMES_SH`.
    #[cfg(unix)]
    const FRAMES_SH: &str = r#"
u32() { printf "$(printf '\\%03o\\%03o\\%03o\\%03o' $(($1 & 255)) $((($1 >> 8) & 255)) $((($1 >> 16) & 255)) $((($1 >> 24) & 255)))"; }
send() { u32 $(( ${#2} + 5 )); printf '\000'; u32 "$1"; printf '%s' "$2"; }
num() { dd bs=1 count=4 2>/dev/null | od -An -tu4 | tr -d ' \n'; }
recv() { LEN=$(num); [ -n "$LEN" ] || exit 0; dd bs=1 count=1 2>/dev/null >/dev/null; CALL=$(num); BODY=$(dd bs=1 count=$((LEN - 5)) 2>/dev/null); }
greet() { recv; send 0 "{\"type\":\"helloOk\",\"protocol\":2,\"version\":\"$1\",\"duckdb\":\"fake\"}"; recv; send 1 '{"type":"opened"}'; }
"#;

    /// Short bounds, so a silent helper costs a second.
    #[cfg(unix)]
    const SHORT: Bounds = Bounds {
        first: Duration::from_millis(300),
        retry: Duration::from_millis(600),
    };

    /// A script in a folder of its own; `runs` counts its starts and
    /// `pids` lists them.
    #[cfg(unix)]
    struct Script {
        dir: tempfile::TempDir,
    }

    #[cfg(unix)]
    impl Script {
        fn new(body: &str) -> Script {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("helper");
            let pids = dir.path().join("pids");
            std::fs::write(
                &path,
                format!(
                    "#!/bin/sh\n{FRAMES_SH}\necho $$ >> '{}'\nRUN=$(wc -l < '{}' | tr -d ' ')\n{body}\n",
                    pids.display(),
                    pids.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Script { dir }
        }

        fn path(&self) -> PathBuf {
            self.dir.path().join("helper")
        }

        fn pids(&self) -> Vec<u32> {
            std::fs::read_to_string(self.dir.path().join("pids"))
                .unwrap_or_default()
                .lines()
                .map(|l| l.trim().parse().unwrap())
                .collect()
        }

        async fn start(&self) -> Result<Started, DbError> {
            let config: ConnectConfig =
                serde_json::from_value(serde_json::json!({"driver": "duckdb", "path": ":memory:"}))
                    .unwrap();
            start_with(&self.path(), &locator(self.dir.path()), &config, SHORT).await
        }
    }

    #[cfg(unix)]
    fn running(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    }

    #[cfg(unix)]
    fn started_error(outcome: Result<Started, DbError>) -> DbError {
        match outcome {
            Ok(_) => panic!("the helper started"),
            Err(e) => e,
        }
    }

    /// A helper that never answers `hello`: a file this process hasn't
    /// started gets one retry with the longer bound, then
    /// `ENGINE_UNAVAILABLE` (not `ENGINE_NOT_INSTALLED`: downloading again
    /// wouldn't help), and both processes are killed.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_silent_helper_is_unavailable_after_one_retry() {
        let script = Script::new("exec sleep 30");
        let started = Instant::now();
        let e = timeout(Duration::from_secs(10), script.start())
            .await
            .map(started_error)
            .expect("the start wasn't bounded");
        let took = started.elapsed();
        assert_eq!(e.code, "ENGINE_UNAVAILABLE", "{e}");
        assert_eq!(e.message, "The DuckDB helper didn't start in time");
        assert!(
            took >= SHORT.first + SHORT.retry && took < Duration::from_secs(3),
            "{took:?}"
        );
        let pids = script.pids();
        assert_eq!(pids.len(), 2, "one retry");
        assert!(
            pids.iter().all(|p| !running(*p)),
            "a helper is still running"
        );
    }

    /// The retry is for a slow first start (macOS checks a new binary on
    /// its first exec): once this file has answered, a silent start isn't
    /// retried.
    #[cfg(unix)]
    #[tokio::test]
    async fn only_a_first_start_is_retried() {
        let script = Script::new(
            "if [ \"$RUN\" = 2 ]; then greet 2026.1.1; cat > /dev/null; else exec sleep 30; fi",
        );
        let first = timeout(Duration::from_secs(10), script.start())
            .await
            .unwrap();
        assert!(first.is_ok(), "{:?}", first.err());
        drop(first);
        let e = timeout(Duration::from_secs(10), script.start())
            .await
            .map(started_error)
            .unwrap();
        assert_eq!(e.code, "ENGINE_UNAVAILABLE", "{e}");
        assert_eq!(script.pids().len(), 3, "no second retry");
    }

    /// What the helper says goes into an error cut to 64 characters:
    /// a version, a refusal's code.
    #[cfg(unix)]
    #[tokio::test]
    async fn helper_strings_in_errors_are_capped() {
        let long = "v".repeat(200);
        let script = Script::new(&format!("greet {long}"));
        let e = started_error(script.start().await);
        assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e}");
        assert!(e.message.contains(&"v".repeat(60)), "{e}");
        assert!(!e.message.contains(&"v".repeat(65)), "{e}");
        let script = Script::new(&format!(
            "recv\nsend 0 '{{\"type\":\"error\",\"error\":{{\"message\":\"m\",\"code\":\"{}\"}}}}'",
            "C".repeat(200)
        ));
        let e = started_error(script.start().await);
        assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{e}");
        assert!(!e.message.contains(&"C".repeat(65)), "{e}");
    }

    /// Windows: what every start checks,
    /// on a layout like an install's under `%TEMP%` (whose inherited DACL
    /// is the profile's: the user, SYSTEM and Administrators).
    #[cfg(windows)]
    mod windows_acl {
        use super::*;
        use seaquel_runtime::acl;
        use std::process::Command;

        struct Layout {
            _dir: tempfile::TempDir,
            locator: HelperLocator,
            identifier: PathBuf,
        }

        impl Layout {
            fn new() -> Layout {
                let dir = tempfile::tempdir().unwrap();
                let identifier = dir.path().join("app");
                let locator = locator(&identifier);
                std::fs::create_dir_all(locator.dir.join(&locator.version)).unwrap();
                std::fs::write(helper_path(&locator), b"MZ").unwrap();
                Layout {
                    _dir: dir,
                    locator,
                    identifier,
                }
            }

            /// The file, then each folder up to `<identifier>`.
            fn levels(&self) -> Vec<PathBuf> {
                let version = self.locator.dir.join(&self.locator.version);
                vec![
                    helper_path(&self.locator),
                    version,
                    self.locator.dir.clone(),
                    self.identifier.join("bin"),
                    self.identifier.clone(),
                ]
            }

            /// What an install sets on `bin`, `duckdb`, the version folder
            /// and the file.
            fn make_private(&self) {
                for (i, level) in self.levels()[..4].iter().enumerate() {
                    acl::make_private(level, i > 0).unwrap();
                }
            }
        }

        fn icacls(path: &Path, args: &[&str]) {
            let out = Command::new("icacls")
                .arg(path)
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "icacls {args:?} failed (setting an owner needs the elevated runner): {}",
                String::from_utf8_lossy(&out.stdout)
            );
        }

        fn assert_unsafe(layout: &Layout, what: &str) {
            let e = check(&layout.locator).expect_err(what);
            assert_eq!(e.code, "ENGINE_NOT_INSTALLED", "{what}: {e}");
            assert!(e.message.contains("unsafe permissions"), "{what}: {e}");
        }

        #[test]
        fn a_profile_layout_and_an_installs_dacls_pass() {
            let layout = Layout::new();
            check(&layout.locator).expect("inherited from the profile");
            layout.make_private();
            check(&layout.locator).expect("as an install leaves it");
        }

        /// `Everyone:(M)` at every level refuses, and the
        /// private DACL an install sets repairs it.
        #[test]
        fn everyone_modify_at_any_level_is_refused_until_repaired() {
            let layout = Layout::new();
            layout.make_private();
            for (i, level) in layout.levels().into_iter().enumerate() {
                icacls(&level, &["/grant", "*S-1-1-0:(M)"]);
                assert_unsafe(&layout, &format!("{level:?}"));
                acl::make_private(&level, i > 0).unwrap();
                check(&layout.locator).expect("repaired");
            }
        }

        /// `BUILTIN\Users` allowed to add files to the version folder is a
        /// write too (a shared `SEAQUEL_DATA_DIR`).
        #[test]
        fn users_adding_files_is_refused() {
            let layout = Layout::new();
            layout.make_private();
            let version = layout.locator.dir.join(&layout.locator.version);
            icacls(&version, &["/grant", "*S-1-5-32-545:(WD)"]);
            assert_unsafe(&layout, "Users:(WD)");
        }

        /// The app's folder may let others add files and folders (the
        /// root rule), not delete a child.
        #[test]
        fn the_root_lets_others_add_but_not_delete() {
            let layout = Layout::new();
            layout.make_private();
            icacls(
                &layout.identifier,
                &["/grant", "*S-1-5-11:(OI)(CI)(RX,WD,AD)"],
            );
            check(&layout.locator).expect("adding files to the app's folder");
            icacls(&layout.identifier, &["/grant", "*S-1-5-11:(DC)"]);
            assert_unsafe(&layout, "delete child on the app's folder");
        }

        /// The owner: another principal (`BUILTIN\Users` here) is
        /// refused; Administrators (what an elevated copy makes) and SYSTEM
        /// pass.
        #[test]
        fn the_owner_must_be_the_user_administrators_or_system() {
            let layout = Layout::new();
            layout.make_private();
            let file = helper_path(&layout.locator);
            icacls(&file, &["/setowner", "*S-1-5-32-545"]);
            assert_unsafe(&layout, "owned by Users");
            icacls(&file, &["/setowner", "*S-1-5-32-544"]);
            check(&layout.locator).expect("owned by Administrators");
            icacls(&file, &["/setowner", "*S-1-5-18"]);
            check(&layout.locator).expect("owned by SYSTEM");
            let version = layout.locator.dir.join(&layout.locator.version);
            icacls(&version, &["/setowner", "*S-1-5-32-545"]);
            assert_unsafe(&layout, "a folder owned by Users");
        }

        /// A junction in place of `bin\duckdb` is refused, even pointing at
        /// a private folder holding a helper.
        #[test]
        fn a_junction_on_the_way_is_refused() {
            let layout = Layout::new();
            layout.make_private();
            let real = layout.identifier.join("real");
            std::fs::rename(&layout.locator.dir, &real).unwrap();
            let out = Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&layout.locator.dir)
                .arg(&real)
                .output()
                .unwrap();
            assert!(out.status.success(), "mklink /J");
            assert_unsafe(&layout, "a junction");
        }
    }

    #[test]
    fn errors_name_no_path() {
        let dir = std::env::temp_dir().join("seaquel-remote-no-such-folder");
        let e = check(&locator(&dir)).unwrap_err();
        assert_eq!(e.code, "ENGINE_NOT_INSTALLED");
        assert!(e.message.contains("2026.1.1"), "{e}");
        assert!(!e.message.contains("seaquel-remote-no-such"), "{e}");
    }
}
