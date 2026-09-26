//! Opening the metadata file: the pre-open checks, the pool, the baseline,
//! the numbered migrations and the data steps.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{ConnectOptions, Connection};

use crate::error::LEGACY_JSON_FILES;
use crate::{schema, StorageError};

/// The numbered migrations in `migrations/`, run after the baseline.
///
/// Migrations a newer build applied and this one doesn't know are ignored,
/// so going back to an older release still opens the file. That holds only
/// because migrations are expand-only (see `migrations/README.md`).
fn migrator() -> Migrator {
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.set_ignore_missing(true);
    migrator
}

/// How long a connection waits for another's write lock before failing, as
/// both TypeScript backends set it.
const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// The first 16 bytes of every SQLite database file.
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";

/// How [`Storage::open`] sizes its pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageOptions {
    /// The most connections the pool opens at once. At least 1.
    pub max_connections: u32,
    /// Close a connection after it has been idle this long. `None` keeps
    /// idle connections open. The web server sets it so idle users don't hold
    /// file handles.
    pub idle_timeout: Option<Duration>,
}

impl Default for StorageOptions {
    fn default() -> Self {
        Self {
            max_connections: 4,
            idle_timeout: None,
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
}

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
    /// - Then the numbered migrations run, each in its own transaction, and
    ///   then the data steps this file hasn't had (`data_steps.rs`). A step
    ///   that fails is rolled back and logged, and `open` still succeeds.
    pub async fn open(
        path: impl AsRef<Path>,
        options: StorageOptions,
    ) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();
        if preflight(&path)? == Existing::NonEmpty {
            probe(&path).await?;
        }

        let connect = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(BUSY_TIMEOUT)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(options.max_connections.max(1))
            .idle_timeout(options.idle_timeout)
            .connect_with(connect)
            .await
            .map_err(|e| classify(&path, e, false))?;

        if let Err(e) = prepare(&pool).await {
            pool.close().await;
            return Err(match e {
                StorageError::Sqlx(e) => classify(&path, e, false),
                StorageError::Migrate(
                    sqlx::migrate::MigrateError::Execute(e)
                    | sqlx::migrate::MigrateError::ExecuteMigration(e, _),
                ) if is_corrupt(&e) => classify(&path, e, false),
                other => other,
            });
        }
        Ok(Self { pool, path })
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

/// What [`preflight`] found at the path.
#[derive(Debug, PartialEq, Eq)]
enum Existing {
    /// Nothing, or an empty file: a new database.
    Empty,
    /// A file that starts with SQLite's header.
    NonEmpty,
}

/// The checks that run before anything opens `path`.
fn preflight(path: &Path) -> Result<Existing, StorageError> {
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
            std::fs::create_dir_all(dir).map_err(|source| StorageError::Io {
                path: dir.to_path_buf(),
                source,
            })?;
            Ok(Existing::Empty)
        }
        Err(e) => Err(io(e)),
    }
}

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

/// Read the schema through a read-only connection that sets no journal mode,
/// so a file with SQLite's header over something else is refused before the
/// pool switches it to WAL (which rewrites the header).
async fn probe(path: &Path) -> Result<(), StorageError> {
    let mut conn = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .busy_timeout(BUSY_TIMEOUT)
        .connect()
        .await
        .map_err(|e| classify(path, e, true))?;
    let read = sqlx::query("SELECT COUNT(*) FROM sqlite_master")
        .execute(&mut conn)
        .await;
    // A failed close can't change a read-only file.
    let _ = conn.close().await;
    read.map(drop).map_err(|e| classify(path, e, true))
}

/// The baseline in one transaction, then the numbered migrations, then the
/// data steps.
async fn prepare(pool: &SqlitePool) -> Result<(), StorageError> {
    // IMMEDIATE takes the write lock up front, so a second process opening
    // the same file waits (busy_timeout) instead of failing halfway.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    schema::baseline(&mut tx).await?;
    tx.commit().await?;
    migrator().run(pool).await?;
    // A failed step is logged and retried on the next open, not an error.
    crate::data_steps::run(pool).await;
    Ok(())
}

/// SQLite's "not a database" (26) and "corrupt" (11) become
/// [`StorageError::Corrupt`]; everything else stays a SQLite error.
/// `untouched` says whether nothing could have written to the file yet.
fn classify(path: &Path, e: sqlx::Error, untouched: bool) -> StorageError {
    match &e {
        sqlx::Error::Database(db) if is_corrupt(&e) => StorageError::Corrupt {
            path: path.to_path_buf(),
            reason: db.message().to_string(),
            untouched,
        },
        _ => StorageError::Sqlx(e),
    }
}

fn is_corrupt(e: &sqlx::Error) -> bool {
    let sqlx::Error::Database(db) = e else {
        return false;
    };
    let primary = db
        .code()
        .and_then(|c| c.parse::<i32>().ok())
        .map(|c| c & 0xff);
    matches!(primary, Some(11 | 26))
}
