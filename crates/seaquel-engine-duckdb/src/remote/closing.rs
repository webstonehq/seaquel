//! Helpers still closing a file database.
//!
//! `close` lets a helper that took it go after 2 s, so its close
//! checkpoint can finish (the helper bounds it at 60 s). Until that helper
//! exits, DuckDB's lock on the file is still held, so a new helper for the
//! same file would fail to open it. Two rules cover that:
//!
//! - **In this process**, a detached helper is kept here by its database's
//!   file ([`FileKey`]), and an open of that file first waits for it to exit
//!   ([`wait_for`], bounded at [`OPEN_WAIT`], then [`still_saving`];
//!   dropping the open stops the wait).
//! - **Across processes** (a TUI or MCP server restarted while its old
//!   helper closes), an open that meets DuckDB's lock conflict is tried
//!   again for up to [`LOCK_RETRY`], then refused in words that name no
//!   process id or path ([`locked`]).
//!
//! And a file a live helper of this process holds
//! isn't opened a second time: [`claim`] refuses
//! it at once ([`already_open`]), instead of the lock retry's 10 s and its
//! "another process" wording. The claim is the connection's until its
//! helper is gone or, closing, kept in the list above. Once its
//! connection's `close` begins the claim is marked closing, and an open
//! waits for it like for a detached helper (a disconnect and a
//! reconnect sent at once). Files are told apart by [`FileKey`].

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use seaquel_engine::{ConnectConfig, DbError};
use tokio::process::Child;
use tokio::time::Instant;

/// How long a helper let go while closing stays in the list: the helper's
/// own close bound (60 s) and a margin.
pub(super) const DETACHED_WAIT: Duration = Duration::from_secs(65);

/// How long an open waits for this process's helper of the same file to
/// exit, under Core's 30 s connect timeout so the open's own answer
/// ([`still_saving`]) arrives instead of a `TIMEOUT` (review follow-up 3).
pub(super) const OPEN_WAIT: Duration = Duration::from_secs(25);

/// How long an open tries again after DuckDB's lock conflict.
pub(super) const LOCK_RETRY: Duration = Duration::from_secs(10);

/// How often the waits look.
pub(super) const POLL: Duration = Duration::from_millis(20);

/// Between two tries of an open that met the lock conflict.
pub(super) const LOCK_RETRY_EVERY: Duration = Duration::from_millis(250);

/// Which file a database is: on Unix its device and inode, so a hard link
/// or another spelling of the path is the same file; elsewhere its
/// canonical path. Never logged or shown.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum FileKey {
    #[cfg_attr(not(unix), allow(dead_code))]
    Inode(u64, u64),
    #[cfg_attr(unix, allow(dead_code))]
    Path(std::path::PathBuf),
}

/// A helper let go while closing the database at `key`.
struct Detached {
    key: FileKey,
    child: Child,
    /// Past it the entry is dropped (tokio reaps the child when it exits).
    until: Instant,
}

static DETACHED: Mutex<Vec<Detached>> = Mutex::new(Vec::new());

/// The key of the database `config` opens ([`FileKey`]), or `None` for an
/// in-memory database or a file that isn't there (no helper can hold it).
pub(super) fn key(config: &ConnectConfig) -> Option<FileKey> {
    let path = config.path.as_deref()?.trim();
    if path.is_empty() || path == ":memory:" || path.starts_with(":memory:") {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(FileKey::Inode(meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        std::fs::canonicalize(path).ok().map(FileKey::Path)
    }
}

/// Keeps `child`, closing the database at `key`, until it exits.
pub(super) fn register(key: FileKey, child: Child) {
    let mut list = DETACHED.lock().unwrap_or_else(PoisonError::into_inner);
    sweep(&mut list);
    list.push(Detached {
        key,
        child,
        until: Instant::now() + DETACHED_WAIT,
    });
}

/// Drops the entries whose helper exited (reaping it) or whose time is up.
fn sweep(list: &mut Vec<Detached>) {
    let now = Instant::now();
    list.retain_mut(|d| matches!(d.child.try_wait(), Ok(None)) && now < d.until);
}

/// Whether a helper of this process is still closing the database at
/// `key`: let go after `close` (the list above), or its connection's
/// `close` has begun and its claim is marked closing ([`Claim::closing`]).
fn closing(key: &FileKey) -> bool {
    let mut list = DETACHED.lock().unwrap_or_else(PoisonError::into_inner);
    sweep(&mut list);
    list.iter().any(|d| d.key == *key)
        || LIVE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|c| c.key == *key && c.closing)
}

/// Waits, at most [`OPEN_WAIT`], until no helper of this process is
/// closing the database at `key`, and returns how long it waited; past
/// that, [`still_saving`].
pub(super) async fn wait_for(key: &FileKey) -> Result<Duration, DbError> {
    let started = Instant::now();
    while closing(key) {
        if started.elapsed() >= OPEN_WAIT {
            return Err(still_saving());
        }
        tokio::time::sleep(POLL).await;
    }
    Ok(started.elapsed())
}

/// This process's helper is still closing the file (a long checkpoint)
/// after [`OPEN_WAIT`]. Names no path.
pub(super) fn still_saving() -> DbError {
    DbError {
        message: "DuckDB is still saving this file. Try again in a few seconds.".to_string(),
        code: "CONNECTION_ERROR".to_string(),
    }
}

/// DuckDB's refusal to open a file another process holds.
pub(super) fn is_lock_conflict(e: &DbError) -> bool {
    e.message.contains("Could not set lock on file")
}

/// The refusal once [`LOCK_RETRY`] has passed: DuckDB's message names the
/// other process's id and the file, which stay out.
pub(super) fn locked() -> DbError {
    DbError {
        message: "DuckDB is still closing this file in another Seaquel process. Try again in a \
                  few seconds."
            .to_string(),
        code: "CONNECTION_ERROR".to_string(),
    }
}

/// One live helper's hold on its file.
struct Held {
    id: u64,
    key: FileKey,
    /// Its connection's `close` has begun: an open waits for it.
    closing: bool,
}

/// The files live helpers of this process hold; one entry per [`Claim`].
static LIVE: Mutex<Vec<Held>> = Mutex::new(Vec::new());

static NEXT_CLAIM: AtomicU64 = AtomicU64::new(0);

/// A live helper's hold on its file: dropping it frees the file for the
/// next open.
pub(super) struct Claim {
    id: u64,
}

impl Claim {
    /// Its connection is closing: from now on an open of the
    /// file waits for the helper to go, as for a detached one, instead of
    /// being refused.
    pub(super) fn closing(&self) {
        let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = live.iter_mut().find(|h| h.id == self.id) {
            held.closing = true;
        }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = live.iter().position(|h| h.id == self.id) {
            live.swap_remove(at);
        }
    }
}

/// Claims the file at `key` for a new helper, or refuses at once when a
/// live helper of this process holds it (one that is closing too: the
/// caller waited for those first, [`wait_for`]).
pub(super) fn claim(key: &FileKey) -> Result<Claim, DbError> {
    let mut live = LIVE.lock().unwrap_or_else(PoisonError::into_inner);
    if live.iter().any(|h| h.key == *key) {
        return Err(already_open());
    }
    let id = NEXT_CLAIM.fetch_add(1, Ordering::Relaxed);
    live.push(Held {
        id,
        key: key.clone(),
        closing: false,
    });
    Ok(Claim { id })
}

/// A second open of a file a live helper of this process holds. DuckDB
/// would refuse it with its lock error; the in-process native driver (gone
/// since) opened it and lost writes. Names no
/// path.
pub(super) fn already_open() -> DbError {
    DbError {
        message: "This DuckDB file is already open in another connection. Disconnect it first."
            .to_string(),
        code: "CONNECTION_ERROR".to_string(),
    }
}
