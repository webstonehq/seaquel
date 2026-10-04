//! Helpers still closing a file database (the DuckDB helper plan's probe
//! F3 and its review's I1).
//!
//! `close` lets a helper that took it go after 2 s, so its close
//! checkpoint can finish (the helper bounds it at 60 s). Until that helper
//! exits, DuckDB's lock on the file is still held, so a new helper for the
//! same file would fail to open it. Two rules cover that:
//!
//! - **In this process**, a detached helper is kept here by its database's
//!   canonical path, and an open of that path first waits for it to exit
//!   ([`wait_for`], bounded at [`DETACHED_WAIT`]; dropping the open stops
//!   the wait).
//! - **Across processes** (a TUI or MCP server restarted while its old
//!   helper closes), an open that meets DuckDB's lock conflict is tried
//!   again for up to [`LOCK_RETRY`], then refused in words that name no
//!   process id or path ([`locked`]).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use seaquel_engine::{ConnectConfig, DbError};
use tokio::process::Child;
use tokio::time::Instant;

/// How long an open waits for this process's helper of the same file to
/// exit: the helper's own close bound (60 s) and a margin.
pub(super) const DETACHED_WAIT: Duration = Duration::from_secs(65);

/// How long an open tries again after DuckDB's lock conflict.
pub(super) const LOCK_RETRY: Duration = Duration::from_secs(10);

/// How often the waits look.
pub(super) const POLL: Duration = Duration::from_millis(20);

/// Between two tries of an open that met the lock conflict.
pub(super) const LOCK_RETRY_EVERY: Duration = Duration::from_millis(250);

/// A helper let go while closing the database at `key`.
struct Detached {
    key: PathBuf,
    child: Child,
    /// Past it the entry is dropped (tokio reaps the child when it exits).
    until: Instant,
}

static DETACHED: Mutex<Vec<Detached>> = Mutex::new(Vec::new());

/// The key of the database `config` opens: its file's canonical path, or
/// `None` for an in-memory database or a file that isn't there (no helper
/// can hold it).
pub(super) fn key(config: &ConnectConfig) -> Option<PathBuf> {
    let path = config.path.as_deref()?.trim();
    if path.is_empty() || path == ":memory:" || path.starts_with(":memory:") {
        return None;
    }
    std::fs::canonicalize(path).ok()
}

/// Keeps `child`, closing the database at `key`, until it exits.
pub(super) fn register(key: PathBuf, child: Child) {
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

/// Whether a helper of this process is still closing the database at `key`.
fn closing(key: &Path) -> bool {
    let mut list = DETACHED.lock().unwrap_or_else(PoisonError::into_inner);
    sweep(&mut list);
    list.iter().any(|d| d.key == key)
}

/// Waits, at most [`DETACHED_WAIT`], until no helper of this process is
/// closing the database at `key`. Returns how long it waited.
pub(super) async fn wait_for(key: &Path) -> Duration {
    let started = Instant::now();
    while closing(key) && started.elapsed() < DETACHED_WAIT {
        tokio::time::sleep(POLL).await;
    }
    started.elapsed()
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
