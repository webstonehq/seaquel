//! Opening the metadata file: the pre-open checks, the pool, the baseline,
//! the numbered migrations (under a write lock) and the data steps; or, for
//! a read-only open, the check that none of those has anything to do.

#[cfg(not(target_arch = "wasm32"))]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::db::{self, SqlitePool};
#[cfg(not(target_arch = "wasm32"))]
use crate::db::{
    ConnectOptions, Connection, Migrator, SqliteConnectOptions, SqliteConnection,
    SqliteJournalMode, SqlitePoolOptions,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::error::LEGACY_JSON_FILES;
#[cfg(not(target_arch = "wasm32"))]
use crate::schema;
use crate::StorageError;

/// The numbered migrations in `migrations/`, run after the baseline.
///
/// Migrations a newer build applied and this one doesn't know are ignored,
/// so going back to an older release still opens the file. That holds only
/// because migrations are expand-only (see `migrations/README.md`).
#[cfg(not(target_arch = "wasm32"))]
fn migrator() -> Migrator {
    let mut migrator = db::embedded_migrator();
    migrator.set_ignore_missing(true);
    migrator
}

/// How long a connection waits for another's write lock before failing, as
/// both TypeScript backends set it.
#[cfg(not(target_arch = "wasm32"))]
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// The first 16 bytes of every SQLite database file.
pub(crate) const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// How [`Storage::open`] opens the file and sizes its pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageOptions {
    /// The most connections the pool opens at once. At least 1.
    pub max_connections: u32,
    /// Close a connection after it has been idle this long. `None` keeps
    /// idle connections open. The web server sets it so idle users don't hold
    /// file handles.
    pub idle_timeout: Option<Duration>,
    /// Open the file without ever writing it, for a second process such as
    /// `seaquel-cli mcp` reading the desktop app's file:
    /// - every connection is opened read-only (`SQLITE_OPEN_READONLY`), so
    ///   any write through the pool fails with `SQLITE_READONLY`;
    /// - the journal mode isn't set: a WAL file stays WAL, and reads don't
    ///   wait for the app's writes;
    /// - no baseline, migration or data step runs. If any of them has work
    ///   to do, the open fails with [`StorageError::NeedsUpgrade`]
    ///   (`STORAGE_NEEDS_UPGRADE`), and the file is left as it was;
    /// - a missing file fails with [`StorageError::NotFound`]
    ///   (`STORAGE_NOT_FOUND`), and nothing is created.
    pub read_only: bool,
    /// The most bytes the file may grow to (phase 5d-2 review: the web's
    /// per-user backstop). Every connection runs `PRAGMA max_page_count`
    /// for it, at [`CAP_PAGE_SIZE`]-byte pages, and a write past it fails
    /// with [`StorageError::code`] `STORAGE_FULL`. `None` (the default, and
    /// the desktop's): no cap. `Storage::open`'s own schema work
    /// (baseline, migrations, data steps, the name-key refill) runs
    /// uncapped, so a file at or over the cap still opens; it stays
    /// readable, and deletes still work so its user can make room.
    pub max_bytes: Option<u64>,
    /// wasm32 only (phase 8 Decision 4): the file to start from, a
    /// snapshot the page kept ([`Storage::snapshot`]). `None` starts an
    /// empty file. Set it with [`StorageOptions::in_memory`].
    #[cfg(target_arch = "wasm32")]
    pub image: Option<Image>,
}

/// The bytes of a metadata file the browser kept. Its `Debug` gives the
/// length only.
#[cfg(target_arch = "wasm32")]
#[derive(Clone, PartialEq, Eq)]
pub struct Image(pub Vec<u8>);

#[cfg(target_arch = "wasm32")]
impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({} bytes)", self.0.len())
    }
}

#[cfg(target_arch = "wasm32")]
impl StorageOptions {
    /// The browser's open (phase 8 Decision 4): SQLite's memory database,
    /// starting from `image` when there is one. [`Storage::open`] then runs
    /// the baseline, the migrations and the data steps as it does on a
    /// file. The path it's given only names the file in errors.
    pub fn in_memory(image: Option<Vec<u8>>) -> Self {
        Self {
            image: image.map(Image),
            ..Self::default()
        }
    }
}

/// The page size [`StorageOptions::max_bytes`] is counted in: SQLite's
/// default, which every Seaquel file uses.
pub const CAP_PAGE_SIZE: u64 = 4096;

impl Default for StorageOptions {
    fn default() -> Self {
        Self {
            max_connections: 4,
            idle_timeout: None,
            read_only: false,
            max_bytes: None,
            #[cfg(target_arch = "wasm32")]
            image: None,
        }
    }
}

/// An open metadata file: a SQLite pool whose connections all run with WAL,
/// `busy_timeout = 5000` and `foreign_keys = ON`, over a schema the baseline
/// and the migrations have brought up to date.
#[derive(Debug, Clone)]
pub struct Storage {
    pool: SqlitePool,
    path: PathBuf,
    /// Opened with [`StorageOptions::read_only`]: [`Storage::write`] refuses.
    read_only: bool,
    /// Serialises this process's writers before they take a pool
    /// connection (see [`crate::WriteTx`]). Clones share it.
    write_lock: Arc<crate::lock::Mutex<()>>,
    /// How long [`Storage::write`] waits for this process's earlier writers.
    write_wait: Duration,
    /// The executor whose `sleep` times that wait ([`Storage::with_executor`];
    /// phase 8 Decision 5). `None`: tokio's timer natively, the page's
    /// `setTimeout` through `WasmExecutor` on wasm32.
    clock: Option<Clock>,
}

/// The interface's executor, for the write turn's wait. Its `Debug` names
/// no more than that it's there.
#[derive(Clone)]
pub(crate) struct Clock(pub(crate) Arc<dyn seaquel_runtime::Executor>);

impl std::fmt::Debug for Clock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<executor>")
    }
}

/// How long [`Storage::write`] waits for the write mutex before failing:
/// long enough for any real write, short enough that a nested write (a
/// deadlock) fails instead of hanging.
pub const WRITE_WAIT: Duration = Duration::from_secs(30);

impl Storage {
    /// Open the metadata file at `path` (the desktop's
    /// `<data dir>/seaquel.db`, or a web user's `meta.db`), creating it and
    /// its directory if they don't exist.
    ///
    /// - When `path` doesn't exist and its directory holds a legacy JSON
    ///   store (`database_connections.json`, `projects.json` or
    ///   `app_state.json`), it fails with [`StorageError::Legacy`] and creates
    ///   nothing. Next to an existing file those JSON files are ignored.
    /// - A file that isn't SQLite fails with [`StorageError::Corrupt`] and is
    ///   left byte for byte as it was: a file without SQLite's header is never
    ///   opened, and one with it is first read through a read-only connection
    ///   that doesn't change the journal mode. SQLite reporting the file
    ///   corrupt later (in the baseline or a migration) gives the same error,
    ///   but by then the file is in WAL mode.
    /// - The baseline ([`schema::baseline`]) runs in one transaction, so if
    ///   it fails the file's schema and rows are as they were.
    /// - Then the numbered migrations run, under one `BEGIN IMMEDIATE`
    ///   lock and only when one is pending (see `migrate` in this file), and then the
    ///   data steps this file hasn't had (`data_steps.rs`). A step that
    ///   fails is rolled back and logged, and `open` still succeeds.
    ///
    /// With [`StorageOptions::read_only`] none of that writes: see there.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open(
        path: impl AsRef<Path>,
        options: StorageOptions,
    ) -> Result<Self, StorageError> {
        Self::open_with_migrator(path, options, migrator()).await
    }

    /// [`Storage::open`] with `migrator` in place of the migrations in
    /// `migrations/`, for tests that need a pending migration. Missing
    /// versions are ignored, as in [`Storage::open`].
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn open_with_migrator(
        path: impl AsRef<Path>,
        options: StorageOptions,
        mut migrator: Migrator,
    ) -> Result<Self, StorageError> {
        migrator.set_ignore_missing(true);
        let path = path.as_ref().to_path_buf();
        if options.read_only {
            return open_read_only(path, options, &migrator).await;
        }
        if preflight(&path, true)? == Existing::NonEmpty {
            probe(&path).await?;
        }

        let connect = sqlite_options()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(BUSY_TIMEOUT)
            .foreign_keys(true);
        let pool_options = || {
            SqlitePoolOptions::new()
                .max_connections(options.max_connections.max(1))
                .idle_timeout(options.idle_timeout)
        };
        // The schema work (baseline, migrations, data steps, the name-key
        // refill) runs uncapped: a file at or over `max_bytes` must still
        // open, or every call would fail with `STORAGE_FULL`. The cap
        // applies to the pool that serves calls afterwards.
        let pool = pool_options()
            .connect_with(connect.clone())
            .await
            .map_err(|e| classify(&path, e, false))?;

        if let Err(e) = prepare(&pool, &migrator).await {
            pool.close().await;
            return Err(match e {
                StorageError::Sqlx(e) => classify(&path, e, false),
                StorageError::Migrate(
                    db::MigrateError::Execute(e) | db::MigrateError::ExecuteMigration(e, _),
                ) if is_corrupt(&e) => classify(&path, e, false),
                other => other,
            });
        }
        let Some(max) = options.max_bytes else {
            return Ok(Self::new(pool, path, false));
        };
        if let Err(e) = crate::refill_name_keys(&Self::new(pool.clone(), path.clone(), false)).await
        {
            log::warn!(activity = "storage.open", code = e.code(); "Refilling name keys failed");
        }
        if let Err(e) = crate::refill_list_meta(&Self::new(pool.clone(), path.clone(), false)).await
        {
            log::warn!(activity = "storage.open", code = e.code(); "Refilling list metadata failed");
        }
        pool.close().await;
        let pages = (max / CAP_PAGE_SIZE).max(1);
        let capped = pool_options()
            .connect_with(connect.pragma("max_page_count", pages.to_string()))
            .await
            .map_err(|e| classify(&path, e, false))?;
        Ok(Self::new(capped, path, false))
    }

    pub(crate) fn new(pool: SqlitePool, path: PathBuf, read_only: bool) -> Self {
        Self {
            pool,
            path,
            read_only,
            write_lock: Arc::default(),
            write_wait: WRITE_WAIT,
            clock: None,
        }
    }

    /// Times the write turn's wait ([`WRITE_WAIT`]) with `executor`'s
    /// `sleep` instead of the default timer (phase 8 Decision 5). Core
    /// passes its own when it opens a workspace, so the browser's waits run
    /// on the page's clock and a test's on its own.
    #[must_use]
    pub fn with_executor(mut self, executor: Arc<dyn seaquel_runtime::Executor>) -> Self {
        self.clock = Some(Clock(executor));
        self
    }

    pub(crate) fn clock(&self) -> Option<&Clock> {
        self.clock.as_ref()
    }

    /// [`WRITE_WAIT`] replaced, for tests.
    #[doc(hidden)]
    pub fn with_write_wait(mut self, wait: Duration) -> Self {
        self.write_wait = wait;
        self
    }

    pub(crate) fn write_wait(&self) -> Duration {
        self.write_wait
    }

    /// Whether it was opened with [`StorageOptions::read_only`].
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    pub(crate) fn write_lock(&self) -> Arc<crate::lock::Mutex<()>> {
        Arc::clone(&self.write_lock)
    }

    /// The pool, for the query modules.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// The file this storage was opened on.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Close every connection. Calls made after this fail.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// What [`preflight`] found at the path.
#[derive(Debug, PartialEq, Eq)]
enum Existing {
    /// Nothing, or an empty file: a new database.
    Empty,
    /// A file that starts with SQLite's header.
    NonEmpty,
}

#[cfg(not(target_arch = "wasm32"))]
/// The checks that run before anything opens `path`. A missing file's
/// directory is created when `create` is set; otherwise a missing file is
/// [`StorageError::NotFound`].
fn preflight(path: &Path, create: bool) -> Result<Existing, StorageError> {
    let io = |source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    };
    match std::fs::metadata(path) {
        Ok(_) => check_header(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let dir = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let legacy: Vec<String> = LEGACY_JSON_FILES
                .iter()
                .filter(|name| dir.join(name).is_file())
                .map(|name| name.to_string())
                .collect();
            if !legacy.is_empty() {
                return Err(StorageError::Legacy {
                    dir: dir.to_path_buf(),
                    files: legacy,
                });
            }
            if !create {
                return Err(StorageError::NotFound {
                    path: path.to_path_buf(),
                });
            }
            std::fs::create_dir_all(dir).map_err(|source| StorageError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
            Ok(Existing::Empty)
        }
        Err(e) => Err(io(e)),
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// An empty file is an empty database. Anything else has to start with
/// SQLite's header.
fn check_header(path: &Path) -> Result<Existing, StorageError> {
    let io = |source| StorageError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = std::fs::File::open(path).map_err(io)?;
    let mut header = Vec::with_capacity(SQLITE_MAGIC.len());
    (&mut file)
        .take(SQLITE_MAGIC.len() as u64)
        .read_to_end(&mut header)
        .map_err(io)?;
    if header.is_empty() {
        Ok(Existing::Empty)
    } else if header == SQLITE_MAGIC {
        Ok(Existing::NonEmpty)
    } else {
        Err(StorageError::Corrupt {
            path: path.to_path_buf(),
            reason: "the file doesn't start with SQLite's header".to_string(),
            untouched: true,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Options every connection to the metadata file starts from. sqlx logs
/// each statement at DEBUG (and, at WARN, one slower than a second) with its
/// whole SQL; storage's SQL is its own, but nothing here should log SQL.
fn sqlite_options() -> SqliteConnectOptions {
    SqliteConnectOptions::new().disable_statement_logging()
}

#[cfg(not(target_arch = "wasm32"))]
/// Read the schema through a read-only connection that sets no journal mode,
/// so a file with SQLite's header over something else is refused before the
/// pool switches it to WAL (which rewrites the header).
async fn probe(path: &Path) -> Result<(), StorageError> {
    let mut conn = sqlite_options()
        .filename(path)
        .read_only(true)
        .busy_timeout(BUSY_TIMEOUT)
        .connect()
        .await
        .map_err(|e| classify(path, e, true))?;
    let read = db::query("SELECT COUNT(*) FROM sqlite_master")
        .execute(&mut conn)
        .await;
    // A failed close can't change a read-only file.
    let _ = conn.close().await;
    read.map(drop).map_err(|e| classify(path, e, true))
}

#[cfg(not(target_arch = "wasm32"))]
/// The baseline in one transaction, then the numbered migrations, then the
/// data steps.
async fn prepare(pool: &SqlitePool, migrator: &Migrator) -> Result<(), StorageError> {
    // Only an open with schema work waits past one busy timeout for the
    // write lock (another pool's migration can hold it that long). A file
    // that is up to date, which is every open but the first after an
    // upgrade, fails after one, as before, so a lock held by another
    // process doesn't stall startup for a minute (5d-2 Task 7 re-review).
    let pending = {
        let mut conn = pool.acquire().await?;
        !schema::is_current(&mut conn).await?
            || migration_state(&mut conn, migrator).await? != MigrationState::Current
    };
    // IMMEDIATE takes the write lock up front, so a second process opening
    // the same file waits (busy_timeout) instead of failing halfway.
    let mut tx = if pending {
        begin_immediate_waiting(pool).await?
    } else {
        pool.begin_with("BEGIN IMMEDIATE").await?
    };
    schema::baseline(&mut tx).await?;
    tx.commit().await?;
    migrate(pool, migrator).await?;
    // A failed step is logged and retried on the next open, not an error.
    crate::data_steps::run(pool).await;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
/// Run the pending migrations, one pool or process at a time.
///
/// sqlx's migrate lock does nothing on SQLite, so two pools on one file
/// (the web server's evicted workspace next to a fresh one, or the app next
/// to another process) could both apply a pending migration, and the second
/// failed on the first's changes. So:
/// 1. read what's applied, without a lock. A file with nothing to do (every
///    open but the first after an upgrade) stops here and takes no lock;
/// 2. otherwise take the write lock with `BEGIN IMMEDIATE`, waiting up to
///    the busy timeout for another migrator to finish;
/// 3. run the migrator on that same connection, inside the lock. It reads
///    what's applied again, so a migration another pool applied while this
///    one waited is skipped. Each migration's own transaction becomes a
///    savepoint of the lock's (sqlx nests `begin` that way), so a failed
///    migration still rolls back alone, and then the lock's rollback undoes
///    the ones before it in this open too.
///
/// Holding the lock on a second connection instead would deadlock: the
/// migrator's writes would wait for that very lock.
async fn migrate(pool: &SqlitePool, migrator: &Migrator) -> Result<(), StorageError> {
    let state = {
        let mut conn = pool.acquire().await?;
        migration_state(&mut conn, migrator).await?
    };
    if state == MigrationState::Current {
        return Ok(());
    }
    let mut tx = begin_immediate_waiting(pool).await?;
    migrator.run_direct(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
/// How many busy timeouts an open with schema work to do (the baseline or
/// a migration) waits for the write lock before it fails: another pool's
/// migration can hold it longer than one (`0003`'s
/// fill runs about 3 s per GB, ~6.5 s on a 2 GiB web file), so an open
/// racing it (the web server's evicted workspace next to a fresh one)
/// waits up to about a minute (5d-2 Task 7 review).
const MIGRATION_WAIT_ATTEMPTS: u32 = 12;

#[cfg(not(target_arch = "wasm32"))]
/// `BEGIN IMMEDIATE`, tried again while another connection holds the
/// write lock (`SQLITE_BUSY` after the busy timeout), up to
/// [`MIGRATION_WAIT_ATTEMPTS`] times. Any other error fails at once.
async fn begin_immediate_waiting(pool: &SqlitePool) -> Result<db::Transaction, db::Error> {
    let mut attempt = 1;
    loop {
        match pool.begin_with("BEGIN IMMEDIATE").await {
            Err(db::Error::Database(e)) if attempt < MIGRATION_WAIT_ATTEMPTS && is_busy(&*e) => {
                attempt += 1;
            }
            other => return other,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// `SQLITE_BUSY` or one of its extended codes (their low byte is 5).
fn is_busy(e: &dyn db::DatabaseError) -> bool {
    e.code()
        .and_then(|c| c.parse::<i32>().ok())
        .is_some_and(|n| n & 0xff == 5)
}

#[cfg(not(target_arch = "wasm32"))]
/// What the migrator would do to a file, read without writing.
#[derive(Debug, PartialEq, Eq)]
enum MigrationState {
    /// Nothing: `_sqlx_migrations` exists and holds every known migration,
    /// with matching checksums.
    Current,
    /// `_sqlx_migrations` doesn't exist yet (the migrator creates it).
    NoTable,
    /// This known migration isn't applied.
    Pending(i64),
    /// A migration is recorded as failed; the migrator refuses the file.
    Dirty(i64),
    /// An applied migration's checksum differs from this build's file; the
    /// migrator refuses the file.
    Mismatch(i64),
}

#[cfg(not(target_arch = "wasm32"))]
async fn migration_state(
    conn: &mut SqliteConnection,
    migrator: &Migrator,
) -> Result<MigrationState, db::Error> {
    let exists: Option<String> = db::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(&mut *conn)
    .await?;
    if exists.is_none() {
        return Ok(MigrationState::NoTable);
    }
    let applied: Vec<(i64, Vec<u8>, bool)> =
        db::query_as("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *conn)
            .await?;
    if let Some((version, _, _)) = applied.iter().find(|(_, _, success)| !success) {
        return Ok(MigrationState::Dirty(*version));
    }
    for migration in migrator
        .iter()
        .filter(|m| !m.migration_type.is_down_migration())
    {
        match applied.iter().find(|(v, _, _)| *v == migration.version) {
            None => return Ok(MigrationState::Pending(migration.version)),
            Some((_, checksum, _)) if checksum.as_slice() != &*migration.checksum => {
                return Ok(MigrationState::Mismatch(migration.version))
            }
            Some(_) => {}
        }
    }
    Ok(MigrationState::Current)
}

#[cfg(not(target_arch = "wasm32"))]
/// [`Storage::open`] with [`StorageOptions::read_only`]: a first check that
/// the file is current through one read-only connection, then a read-only
/// pool, and the authoritative check again on one of its connections.
async fn open_read_only(
    path: PathBuf,
    options: StorageOptions,
    migrator: &Migrator,
) -> Result<Storage, StorageError> {
    if preflight(&path, false)? == Existing::Empty {
        return Err(needs_upgrade(&path, "the file is empty"));
    }
    check_current(&path, migrator).await?;

    let connect = sqlite_options()
        .filename(&path)
        .read_only(true)
        .busy_timeout(BUSY_TIMEOUT)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(options.max_connections.max(1))
        .idle_timeout(options.idle_timeout)
        .connect_with(connect)
        .await
        .map_err(|e| classify(&path, e, true))?;

    // The authoritative check, through the pool's own locking reads (see
    // `check_current` for why the first one can't be).
    let checked = match pool.acquire().await {
        Ok(mut conn) => check_on(&mut conn, &path, migrator).await,
        Err(e) => Err(classify(&path, e, true)),
    };
    if let Err(e) = checked {
        pool.close().await;
        return Err(e);
    }
    Ok(Storage::new(pool, path, true))
}

#[cfg(not(target_arch = "wasm32"))]
/// The first, cheap pass of the read-only check ([`check_on`]), before the
/// pool exists. It also stands in for [`probe`]: a file that isn't a
/// database fails here, as [`StorageError::Corrupt`] with `untouched`.
///
/// A WAL file read through a normal read-only connection gets a `-wal` and
/// a `-shm` file, and SQLite can't remove them when that connection closes.
/// When neither exists, nothing else had the file open a moment ago, so this
/// pass opens it `immutable`, which creates neither and takes no locks: a
/// file refused here leaves the directory exactly as it was. When they
/// exist, another connection may be writing, and it reads through them
/// normally.
///
/// `immutable` promises SQLite the file won't change, and that isn't
/// guaranteed: the app may open it during this pass and write, and its
/// checkpoints write the main file, so this pass can read a torn or stale
/// file. So its "current" is only a first answer. [`open_read_only`] checks
/// again through a connection of the read-only pool, which takes SQLite's
/// locks, and that check decides. (A file that passes here and fails there
/// is left with the pool's `-wal` and `-shm`, as any WAL reader leaves them.)
async fn check_current(path: &Path, migrator: &Migrator) -> Result<(), StorageError> {
    let sidecar = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name).exists()
    };
    let in_use = sidecar("-wal") || sidecar("-shm");
    let mut conn = sqlite_options()
        .filename(path)
        .read_only(true)
        .immutable(!in_use)
        .busy_timeout(BUSY_TIMEOUT)
        .connect()
        .await
        .map_err(|e| classify(path, e, true))?;
    let checked = check_on(&mut conn, path, migrator).await;
    // A failed close can't change a read-only file.
    let _ = conn.close().await;
    checked
}

#[cfg(not(target_arch = "wasm32"))]
async fn check_on(
    conn: &mut SqliteConnection,
    path: &Path,
    migrator: &Migrator,
) -> Result<(), StorageError> {
    let sql = |e| classify(path, e, true);
    // One read transaction, so the three checks see one version of the file.
    let mut tx = conn.begin().await.map_err(sql)?;
    if !schema::is_current(&mut tx).await.map_err(sql)? {
        return Err(needs_upgrade(
            path,
            "its schema predates this release's baseline",
        ));
    }
    match migration_state(&mut tx, migrator).await.map_err(sql)? {
        MigrationState::Current => {}
        MigrationState::NoTable => {
            return Err(needs_upgrade(path, "no migration has been recorded"));
        }
        MigrationState::Pending(version) => {
            return Err(needs_upgrade(
                path,
                &format!("migration {version} hasn't been applied"),
            ));
        }
        MigrationState::Dirty(version) => {
            return Err(db::MigrateError::Dirty(version).into());
        }
        MigrationState::Mismatch(version) => {
            return Err(db::MigrateError::VersionMismatch(version).into());
        }
    }
    let steps = crate::data_steps::pending(&mut tx)
        .await
        .map_err(|e| match e {
            StorageError::Sqlx(e) => sql(e),
            other => other,
        })?;
    if let Some(step) = steps.first() {
        return Err(StorageError::DataStepPending {
            path: path.to_path_buf(),
            step: step.to_string(),
        });
    }
    tx.rollback().await.map_err(sql)?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn needs_upgrade(path: &Path, reason: &str) -> StorageError {
    StorageError::NeedsUpgrade {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    }
}

/// SQLite's "not a database" (26) and "corrupt" (11) become
/// [`StorageError::Corrupt`]; everything else stays a SQLite error.
/// `untouched` says whether nothing could have written to the file yet.
pub(crate) fn classify(path: &Path, e: db::Error, untouched: bool) -> StorageError {
    match &e {
        db::Error::Database(db) if is_corrupt(&e) => StorageError::Corrupt {
            path: path.to_path_buf(),
            reason: db.message().to_string(),
            untouched,
        },
        _ => StorageError::Sqlx(e),
    }
}

pub(crate) fn is_corrupt(e: &db::Error) -> bool {
    let db::Error::Database(db) = e else {
        return false;
    };
    let primary = db
        .code()
        .and_then(|c| c.parse::<i32>().ok())
        .map(|c| c & 0xff);
    matches!(primary, Some(11 | 26))
}
