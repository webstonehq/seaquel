//! The remote driver (the DuckDB helper plan, Task 3): DuckDB in a
//! `seaquel-duckdb` process of its own, spoken to over its stdin and stdout
//! in `wire.rs`'s frames. The terminal binaries use it so they don't link
//! DuckDB; nothing in Core knows the driver is remote.
//!
//! - **One helper per open connection** (Q2 A): [`Engine::open`] checks the
//!   install ([`process::check`]), starts the helper, checks its `hello`
//!   (protocol and app version) within 5 s and opens the database; until
//!   `opened`, nothing else is sent. A missing, stale or unsafe helper, or
//!   one that refuses the handshake or ends before answering it, is
//!   `ENGINE_NOT_INSTALLED` before the connect's timeout matters, so the
//!   interface can offer the download; a silent helper, one that doesn't
//!   answer in time (one retry within 20 s for a file this process hasn't
//!   started yet), is `ENGINE_UNAVAILABLE`, for which a download wouldn't
//!   help.
//! - **Calls** ([`conn`]) carry ids; the helper runs the main session's in
//!   turn and the read-only ones beside them. Dropping a call before its
//!   last frame posts a `cancel` at once, from `Drop`, ordered before any
//!   later request; its frames are dropped until its last one. **A stream
//!   that isn't read holds the main session**: once its
//!   credit window is used up the helper waits, and every later `query`,
//!   `stream`, `execute` and `transaction` waits behind it until the stream
//!   is read, dropped or cancelled. Read-only calls run beside it.
//! - **A helper that dies** fails every waiting and later call with
//!   `CONNECTION_CLOSED`, naming the signal or exit code. `close` refuses
//!   new calls, asks the helper to exit (calls in flight get the helper's
//!   last frame for them) and waits 2 s: a helper that took `close` (its
//!   output ended) and is still closing the database is then left to
//!   finish, since its checkpoint can take seconds and the helper bounds
//!   it itself (60 s); one that didn't take it is killed. Dropping the
//!   driver kills it.
//!
//! Logs carry `activity=duckdb.helper` and an `event` (`spawn`, `ready`,
//! `exit`, `crash`), statuses, durations and byte counts: never SQL, values,
//! paths, frame payloads or the helper's stderr.

mod closing;
mod conn;
mod driver;
mod process;

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use log::{info, warn};
use seaquel_engine::{ConnectConfig, DbError, Dialect, Driver, Engine};

use crate::dialect::DuckdbDialect;

/// Where the helper is and which version it must be: the file is
/// `<dir>/<version>/seaquel-duckdb[.exe]`, and `dir` is the install's
/// `bin/duckdb` folder (`<data_local_dir>/<identifier>/bin/duckdb`). Before
/// each start the file and every folder up to `<identifier>` (the parent of
/// `bin`) are checked: no symlinks, and on Unix owned by this user and
/// writable by nobody else. The helper's `hello` must report `version`.
#[derive(Clone)]
pub struct HelperLocator {
    pub dir: PathBuf,
    /// The app version of the binary that runs the helper.
    pub version: String,
}

impl fmt::Debug for HelperLocator {
    /// The version only: the folder is under the user's home.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HelperLocator")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl HelperLocator {
    /// The helper's file, `<dir>/<version>/seaquel-duckdb[.exe]`: where the
    /// install (Core's `duckdb_helper_install`) puts it and where every
    /// start looks.
    pub fn path(&self) -> PathBuf {
        process::helper_path(self)
    }

    /// The helper's file name, `seaquel-duckdb[.exe]`.
    pub fn file_name() -> String {
        process::helper_file_name()
    }

    /// The check every start runs first (Decision 9): the file's path when
    /// it is there, a regular file, and it and its folders up to
    /// `<identifier>` are no symlinks and (Unix) this user's and writable by
    /// nobody else; else `ENGINE_NOT_INSTALLED`. Nothing is started.
    pub fn check(&self) -> Result<PathBuf, DbError> {
        process::check(self)
    }
}

/// The DuckDB engine over a helper process. Its id is `duckdb`; the browser
/// driver has the same id, and no build has both.
pub fn remote_engine(locator: HelperLocator) -> Arc<dyn Engine> {
    Arc::new(RemoteEngine { locator })
}

pub struct RemoteEngine {
    locator: HelperLocator,
}

#[seaquel_runtime::async_trait]
impl Engine for RemoteEngine {
    fn id(&self) -> &'static str {
        "duckdb"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        let path = process::check(&self.locator)?;
        let key = closing::key(config);
        if let Some(key) = &key {
            let waited = closing::wait_for(key).await.inspect_err(|_| {
                warn!(activity = "duckdb.helper", event = "wait_closing", code = "CONNECTION_ERROR"; "this file's closing DuckDB helper outlasted the open's wait");
            })?;
            if waited >= closing::POLL {
                info!(activity = "duckdb.helper", event = "wait_closing", ms = waited.as_millis() as u64; "waited for this file's closing DuckDB helper");
            }
        }
        // A file a live helper of this process holds: refused now (a
        // second open would wait out the lock retry, then be refused).
        let claim = key.as_ref().map(closing::claim).transpose()?;
        // Another process's helper may still be closing the file.
        let deadline = tokio::time::Instant::now() + closing::LOCK_RETRY;
        let started = loop {
            match process::start(&path, &self.locator, config).await {
                Err(e) if closing::is_lock_conflict(&e) => {
                    if tokio::time::Instant::now() >= deadline {
                        warn!(activity = "duckdb.helper", event = "locked"; "the DuckDB file stayed locked by another process");
                        return Err(closing::locked());
                    }
                    tokio::time::sleep(closing::LOCK_RETRY_EVERY).await;
                }
                other => break other?,
            }
        };
        // A file the open created has a key (and a claim) only now. Its
        // helper opened it, so no other helper of this process holds it.
        let key = key.or_else(|| closing::key(config));
        let claim = claim.or_else(|| {
            let key = key.as_ref()?;
            closing::claim(key)
                .map_err(|e| {
                    // Another open of it raced this one (review M2).
                    warn!(activity = "duckdb.helper", event = "claim", code = e.code.as_str(); "a DuckDB file created by this open was already claimed");
                })
                .ok()
        });
        let (conn, tasks) = conn::Conn::run(started, key, claim);
        Ok(Arc::new(driver::RemoteDriver::new(conn, tasks)))
    }

    /// The install check every start runs first ([`process::check`]), so
    /// Core hears `ENGINE_NOT_INSTALLED` before it closes anything a
    /// reconnect would replace (Decision 21).
    fn preflight(&self, _config: &ConnectConfig) -> Result<(), DbError> {
        process::check(&self.locator).map(|_| ())
    }

    /// A file database is one helper's, under DuckDB's lock: a second
    /// connection to it opens only once the first is gone.
    fn exclusive_file(&self, config: &ConnectConfig) -> bool {
        config.path.as_deref().is_some_and(|p| {
            let p = p.trim();
            !p.is_empty() && !p.starts_with(":memory:")
        })
    }

    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&DuckdbDialect)
    }
}
