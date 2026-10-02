//! The browser's open (phase 8 Decision 4): the metadata file lives in
//! SQLite's memory database, starting from the snapshot the page kept, and
//! goes through the same baseline, numbered migrations and data steps as a
//! file on desktop. There is no file system here: no legacy-JSON check, no
//! WAL, no busy timeout and no size cap (one connection, in memory).
//!
//! The migrations are recorded in `_sqlx_migrations` exactly as sqlx's
//! migrator records them (same table, version, description, SHA-384
//! checksum, `success`, a measured `execution_time`), so a file made here
//! opens on desktop with nothing pending, and the other way round.

use std::path::Path;

#[cfg(any(test, feature = "test-hooks"))]
use crate::db::Row;
use crate::db::{self, SqliteConnection, SqlitePool};
use crate::migrations::{Migration, MIGRATIONS};
use crate::open::{classify, is_corrupt, SQLITE_MAGIC};
use crate::{schema, Storage, StorageError, StorageOptions};

/// sqlx's `_sqlx_migrations` table, statement for statement.
const MIGRATIONS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);
                "#;

impl Storage {
    /// Opens the metadata file in memory: `options.image` (a snapshot the
    /// page kept) or a new, empty file; then the baseline, the migrations
    /// and the data steps, as on desktop (see the native `Storage::open`).
    /// `path` only names the file in errors. `read_only`, `max_bytes` and
    /// the pool options don't apply here.
    ///
    /// An image that doesn't start with SQLite's header, or that SQLite
    /// reports as not a database or corrupt, fails with
    /// [`StorageError::Corrupt`] (`STORAGE_CORRUPT`); the page keeps such a
    /// snapshot aside and starts empty.
    pub async fn open(
        path: impl AsRef<Path>,
        options: StorageOptions,
    ) -> Result<Self, StorageError> {
        let path = path.as_ref().to_path_buf();
        let image = options.image.map(|i| i.0).filter(|i| !i.is_empty());
        if let Some(image) = &image {
            if !image.starts_with(SQLITE_MAGIC) {
                return Err(StorageError::Corrupt {
                    path,
                    reason: "the file doesn't start with SQLite's header".to_string(),
                    untouched: true,
                });
            }
        }
        let pool =
            SqlitePool::open_in_memory(image.as_deref()).map_err(|e| classify(&path, e, true))?;
        // The probe: a file SQLite can't read fails here, before anything
        // writes to it.
        db::query("SELECT COUNT(*) FROM sqlite_master")
            .execute(&pool)
            .await
            .map_err(|e| classify(&path, e, true))?;
        if let Err(e) = prepare(&pool).await {
            pool.close().await;
            return Err(match e {
                StorageError::Sqlx(e) => classify(&path, e, false),
                StorageError::Migrate(
                    db::MigrateError::Execute(e) | db::MigrateError::ExecuteMigration(e, _),
                ) if is_corrupt(&e) => classify(&path, e, false),
                other => other,
            });
        }
        Ok(Self::new(pool, path, false))
    }

    /// The whole file as it stands (`sqlite3_serialize`), for the page to
    /// keep (phase 8 Decision 6). Refused while a call holds the
    /// connection or a write is open, so it never holds uncommitted rows:
    /// the page takes it between calls.
    pub fn snapshot(&self) -> Result<Vec<u8>, StorageError> {
        Ok(self.pool().snapshot()?)
    }

    /// How many write transactions have committed since the open, so the
    /// page knows whether anything may have changed since its last
    /// snapshot. Every committing write counts, including Core's own (a
    /// data step, a refill) that emit no event, and so does a write
    /// transaction that changed nothing (the open's baseline): the count
    /// can run ahead of real changes, never behind them.
    pub fn commits(&self) -> u64 {
        self.pool().commits()
    }

    /// Plain SQL through the pool, every cell as text (`NULL` for NULL):
    /// for tests, which can't name the executor. Only with the
    /// `test-hooks` feature (this crate's own wasm32 tests turn it on), so
    /// it never ships in the module.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub async fn debug_rows(&self, sql: &str) -> Result<Vec<Vec<String>>, StorageError> {
        let rows = db::query(sql).fetch_all(self.pool()).await?;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let mut cells = Vec::with_capacity(row.len());
            for i in 0..row.len() {
                let cell: Option<String> = row.try_get_unchecked(i)?;
                cells.push(cell.unwrap_or_else(|| "NULL".to_string()));
            }
            out.push(cells);
        }
        Ok(out)
    }
}

/// The baseline in one transaction, then the numbered migrations, then the
/// data steps, as the native open's `prepare`.
async fn prepare(pool: &SqlitePool) -> Result<(), StorageError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    schema::baseline(&mut tx).await?;
    tx.commit().await?;
    migrate(pool).await?;
    // A failed step is logged and retried on the next open, not an error.
    crate::data_steps::run(pool).await;
    Ok(())
}

/// Applies the pending migrations as sqlx's `Migrator::run_direct` does
/// for SQLite, under one `BEGIN IMMEDIATE` (as the native open holds it),
/// each in its own savepoint. Applied versions this build doesn't know are
/// ignored (`set_ignore_missing(true)` natively).
async fn migrate(pool: &SqlitePool) -> Result<(), StorageError> {
    let mut conn = pool.acquire().await?;
    if is_current(&mut conn).await? {
        return Ok(());
    }
    db::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;
    run_direct(&mut conn).await?;
    db::query("COMMIT").execute(&mut *conn).await?;
    Ok(())
}

/// Whether every known migration is applied with its checksum and none is
/// dirty, read without writing.
async fn is_current(conn: &mut SqliteConnection) -> Result<bool, StorageError> {
    let exists: Option<String> = db::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(&mut *conn)
    .await?;
    if exists.is_none() {
        return Ok(false);
    }
    let applied: Vec<(i64, Vec<u8>, bool)> =
        db::query_as("SELECT version, checksum, success FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *conn)
            .await?;
    if applied.iter().any(|(_, _, success)| !success) {
        return Ok(false);
    }
    Ok(MIGRATIONS.iter().all(|m| {
        applied
            .iter()
            .any(|(v, sum, _)| *v == m.version && *sum == m.checksum())
    }))
}

/// sqlx's `run_direct` with its SQLite `Migrate` impl, in order: the
/// table, the dirty check, the applied list, then each migration either
/// checked or applied.
async fn run_direct(conn: &mut SqliteConnection) -> Result<(), StorageError> {
    let exec = db::MigrateError::Execute;
    db::query(MIGRATIONS_TABLE)
        .execute(&mut *conn)
        .await
        .map_err(exec)?;
    let dirty: Option<(i64,)> = db::query_as(
        "SELECT version FROM _sqlx_migrations WHERE success = false ORDER BY version LIMIT 1",
    )
    .fetch_optional(&mut *conn)
    .await
    .map_err(exec)?;
    if let Some((version,)) = dirty {
        return Err(db::MigrateError::Dirty(version).into());
    }
    let applied: Vec<(i64, Vec<u8>)> =
        db::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&mut *conn)
            .await
            .map_err(exec)?;
    for m in MIGRATIONS {
        match applied.iter().find(|(v, _)| *v == m.version) {
            Some((_, sum)) if *sum != m.checksum() => {
                return Err(db::MigrateError::VersionMismatch(m.version).into());
            }
            Some(_) => {}
            None => apply(conn, m).await?,
        }
    }
    Ok(())
}

/// sqlx's SQLite `apply`: the migration and its record in one savepoint
/// (the transaction is already open, so sqlx's `begin` nests), then the
/// measured time.
async fn apply(conn: &mut SqliteConnection, m: &Migration) -> Result<(), StorageError> {
    use seaquel_runtime::Executor as _;
    let clock = seaquel_runtime::WasmExecutor;
    db::query("SAVEPOINT _sqlx_savepoint_1")
        .execute(&mut *conn)
        .await
        .map_err(db::MigrateError::Execute)?;
    let start = clock.monotonic();
    let applied = async {
        db::query(m.sql)
            .execute(&mut *conn)
            .await
            .map_err(|e| db::MigrateError::ExecuteMigration(e, m.version))?;
        db::query(
            r#"
    INSERT INTO _sqlx_migrations ( version, description, success, checksum, execution_time )
    VALUES ( ?1, ?2, TRUE, ?3, -1 )
                "#,
        )
        .bind(m.version)
        .bind(m.description)
        .bind(m.checksum())
        .execute(&mut *conn)
        .await
        .map_err(db::MigrateError::Execute)?;
        Ok::<(), db::MigrateError>(())
    }
    .await;
    if let Err(e) = applied {
        // The savepoint's work goes; the caller's ROLLBACK (the connection
        // dropped inside its transaction) undoes the rest of this open.
        let _ = db::query("ROLLBACK TO SAVEPOINT _sqlx_savepoint_1")
            .execute(&mut *conn)
            .await;
        return Err(e.into());
    }
    db::query("RELEASE SAVEPOINT _sqlx_savepoint_1")
        .execute(&mut *conn)
        .await
        .map_err(db::MigrateError::Execute)?;
    let elapsed = clock.monotonic().saturating_sub(start);
    db::query(
        r#"
    UPDATE _sqlx_migrations
    SET execution_time = ?1
    WHERE version = ?2
                "#,
    )
    .bind(i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
    .bind(m.version)
    .execute(&mut *conn)
    .await
    .map_err(db::MigrateError::Execute)?;
    Ok(())
}
