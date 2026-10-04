//! Finding the helper, checking its folder, starting it, and the handshake
//! (the DuckDB helper plan, Decisions 2, 7, 8 and 9).
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
/// of `dir`) down, and on Unix every one of them owned by this user and
/// writable by nobody else. It doesn't hash the file: anyone who can write
/// that folder can already replace the terminal binary itself. Folders above
/// `<identifier>` aren't checked, and the helper is then started by path,
/// so a folder above it that someone else can write is a gap (Decision 9).
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
    if !owned_and_private(&file) {
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
        if !meta.file_type().is_dir() || !owned_and_private(&meta) {
            return Err(unsafe_permissions());
        }
    }
    Ok(path)
}

/// Owned by this user and writable by nobody else.
#[cfg(unix)]
fn owned_and_private(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: `geteuid` has no preconditions and can't fail.
    let me = unsafe { libc::geteuid() };
    meta.uid() == me && meta.mode() & 0o022 == 0
}

/// Windows: the user's profile ACLs protect the folder (Decision 9).
#[cfg(not(unix))]
fn owned_and_private(_: &std::fs::Metadata) -> bool {
    true
}

/// The helper's process. Dropping it kills the helper (what tokio's
/// `kill_on_drop` does, kept by hand), unless it was [`detached`]: a helper
/// that took `close` and is still closing its database is left to finish
/// (the DuckDB helper plan's probe F3). tokio reaps a dropped child that
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

/// Serializes the spawns of helpers in this process (the review's M2). On
/// macOS std creates a child's pipes with `pipe()` and only then marks them
/// close-on-exec, so a helper spawned on another thread in between would
/// inherit this helper's pipe ends, and this helper would never see EOF on
/// stdin when its connection closes. Spawns elsewhere in the process (an
/// `$EDITOR`) aren't covered; they don't spawn helpers concurrently with
/// one another, and a helper still exits on `close` or a kill.
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
    // out of the helper; everything else is inherited (Decision 2).
    for (key, _) in std::env::vars_os() {
        if let Some(name) = key.to_str() {
            if name.starts_with("SEAQUEL_") && name.contains("_TEST_") {
                command.env_remove(&key);
            }
        }
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
    /// option): the connect's error, as natively.
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

    fn started_error(outcome: Result<Started, DbError>) -> DbError {
        match outcome {
            Ok(_) => panic!("the helper started"),
            Err(e) => e,
        }
    }

    /// A helper that never answers `hello`: a file this process hasn't
    /// started gets one retry with the longer bound, then
    /// `ENGINE_UNAVAILABLE` (not `ENGINE_NOT_INSTALLED`: downloading again
    /// wouldn't help), and both processes are killed (the review's M5).
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

    /// What the helper says goes into an error cut to 64 characters (the
    /// review's M7): a version, a refusal's code.
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

    #[test]
    fn errors_name_no_path() {
        let dir = std::env::temp_dir().join("seaquel-remote-no-such-folder");
        let e = check(&locator(&dir)).unwrap_err();
        assert_eq!(e.code, "ENGINE_NOT_INSTALLED");
        assert!(e.message.contains("2026.1.1"), "{e}");
        assert!(!e.message.contains("seaquel-remote-no-such"), "{e}");
    }
}
