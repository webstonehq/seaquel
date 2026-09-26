use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Connection, Pool, Sqlite, SqliteConnection,
};

use seaquel_engine::{
    BatchStatement, BoxStream, CancellationToken, ConnectConfig, DatabaseStatistics, DbError,
    Driver, ExecuteResult, ExplainResult, QueryResult, SchemaColumn, SchemaIndex, SchemaTable,
    StreamBatch, Value,
};

use crate::{introspect, read_only};

/// Directory containing the database file, resolved the same way sqlx will
/// open it. Stripping the scheme by hand breaks on Windows: `sqlite://C:\db`
/// minus `sqlite:` leaves `//C:\db`, which Windows treats as a UNC network path.
fn database_parent_dir(conn_str: &str) -> Result<Option<PathBuf>, DbError> {
    let options = SqliteConnectOptions::from_str(conn_str).map_err(DbError::connection_error)?;
    Ok(options
        .get_filename()
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(PathBuf::from))
}

/// Whether `conn_str` opens an in-memory database: `sqlite::memory:`,
/// `:memory:`, or `mode=memory` (the rules sqlx's parser applies).
fn is_in_memory(conn_str: &str) -> bool {
    let (path, query) = conn_str.split_once('?').unwrap_or((conn_str, ""));
    let path = path
        .strip_prefix("sqlite://")
        .or_else(|| path.strip_prefix("sqlite:"))
        .unwrap_or(path);
    path == ":memory:" || query.split('&').any(|kv| kv == "mode=memory")
}

/// Pool settings for `conn_str`. An in-memory database (sqlx gives the pool's
/// connections one shared-cache database) lives only while a connection to
/// it is open, so its pool keeps one open for good: the defaults (no minimum,
/// idle connections closed after 10 minutes, every connection replaced after
/// 30) would wipe it, e.g. the desktop tutorial's `sqlite::memory:` after it
/// sat idle. File databases keep the defaults.
fn pool_options(conn_str: &str) -> SqlitePoolOptions {
    let options = SqlitePoolOptions::new();
    if is_in_memory(conn_str) {
        options
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
    } else {
        options
    }
}

/// The SQLite `Driver`: [`SqlxDriver`] behind a check that no SQL text
/// holds a NUL byte (see [`refuse_nul`]).
pub struct SqliteDriver {
    inner: SqlxDriver,
}

impl SqliteDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        Ok(Self {
            inner: SqlxDriver::connect(config).await?,
        })
    }
}

/// The refusal for SQL text holding a NUL byte.
pub(crate) const NUL_BYTE: &str = "SQLite queries can't contain a NUL byte";

/// `QUERY_ERROR` if `sql` holds a NUL byte. SQLite stops parsing at a NUL
/// and hands it back as the unparsed tail, and sqlx 0.8.6 then loops on the
/// zero-length advance forever: a busy worker thread that never gives its
/// pooled connection back, so about as many such queries as the pool has
/// connections hang the database for good. Every entry point that takes
/// SQL text checks here before sqlx sees it.
fn refuse_nul(sql: &str) -> Result<(), DbError> {
    if sql.contains('\0') {
        return Err(DbError::query_error(NUL_BYTE));
    }
    Ok(())
}

#[seaquel_runtime::async_trait]
impl Driver for SqliteDriver {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        refuse_nul(sql)?;
        self.inner.query(sql, params).await
    }

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        refuse_nul(sql)?;
        self.inner.execute(sql, params).await
    }

    async fn transaction(&self, statements: Vec<BatchStatement>) -> Result<(), DbError> {
        for statement in &statements {
            refuse_nul(&statement.sql)?;
        }
        self.inner.transaction(statements).await
    }

    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        match refuse_nul(&sql) {
            Ok(()) => self.inner.query_stream(sql, params, cancel),
            Err(e) => Box::pin(futures::stream::once(async move { Err(e) })),
        }
    }

    async fn query_read_only(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        refuse_nul(sql)?;
        self.inner.query_read_only(sql, params).await
    }

    async fn close(&self) -> Result<(), DbError> {
        self.inner.close().await
    }

    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        self.inner.list_schemas().await
    }

    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        self.inner.schema_tables().await
    }

    async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        self.inner.table_metadata(schema, table).await
    }

    async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
        self.inner.statistics().await
    }

    async fn explain(
        &self,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        refuse_nul(sql)?;
        self.inner.explain(sql, params, analyze).await
    }
}

/// The sqlx-based driver (`impl_sqlx_driver!`), always used through
/// [`SqliteDriver`].
struct SqlxDriver {
    pool: Pool<Sqlite>,
}

impl SqlxDriver {
    async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let conn_str = config
            .connection_string
            .as_deref()
            .ok_or_else(|| DbError::connection_error("connection_string is required for SQLite"))?;

        let exists = Sqlite::database_exists(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        if !exists {
            if !config.create_if_missing.unwrap_or(false) {
                let path = SqliteConnectOptions::from_str(conn_str)
                    .map_err(DbError::connection_error)?
                    .get_filename()
                    .display()
                    .to_string();
                return Err(DbError {
                    message: format!("Database file not found: {}", path),
                    code: "FILE_NOT_FOUND".to_string(),
                });
            }

            if let Some(parent) = database_parent_dir(conn_str)? {
                if !parent.exists() {
                    std::fs::create_dir_all(&parent).map_err(|e| {
                        DbError::connection_error(format!(
                            "Failed to create database directory: {}",
                            e
                        ))
                    })?;
                }
            }

            Sqlite::create_database(conn_str)
                .await
                .map_err(DbError::connection_error)?;
        }

        let pool = pool_options(conn_str)
            .connect(conn_str)
            .await
            .map_err(DbError::connection_error)?;

        Ok(Self { pool })
    }
}

seaquel_engine::impl_sqlx_driver!(
    SqlxDriver,
    Sqlite,
    sqlx::sqlite::SqliteArguments<'q>,
    decode_fn = crate::decode::to_value,
    bind_fn = crate::bind::bind_value,
    last_insert_id = |r: &sqlx::sqlite::SqliteQueryResult| Some(r.last_insert_rowid()),
    introspection = {
        /// Always `main` (TS `SELECT 'main' as schema_name`).
        async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
            Ok(introspect::schemas())
        }

        async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
            let r = self.query(introspect::SCHEMA_SQL, vec![]).await?;
            Ok(introspect::parse_schema(&r))
        }

        /// The schema is ignored, as in the TS: SQLite resolves the bound
        /// table name in `main` (then attached databases).
        async fn table_metadata(
            &self,
            _schema: &str,
            table: &str,
        ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
            let (columns, foreign_keys, indexes) = futures::join!(
                self.query(introspect::COLUMNS_SQL, vec![Value::from(table)]),
                self.query(introspect::FOREIGN_KEYS_SQL, vec![Value::from(table)]),
                self.query(introspect::INDEXES_SQL, vec![Value::from(table)]),
            );
            let mut columns = introspect::parse_columns(&columns?, Some(&foreign_keys?));
            let indexes = indexes?;
            let partial = introspect::partial_indexes(&indexes);
            let indexes = introspect::parse_indexes(&indexes);
            // Task 18: UNIQUE from the indexes (the parse stays the TS's),
            // without partial unique indexes.
            let plain: Vec<SchemaIndex> = indexes
                .iter()
                .filter(|i| !partial.contains(&i.name))
                .cloned()
                .collect();
            seaquel_engine::introspect::apply_unique_indexes(&mut columns, &plain);
            Ok((columns, indexes))
        }

        /// Row counts come from a `COUNT(*)` per table (moved here from
        /// TsEngineClient); a count that fails leaves the table at 0.
        async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
            let (overview, table_sizes, index_usage) = futures::join!(
                self.query(introspect::OVERVIEW_SQL, vec![]),
                self.query(introspect::TABLE_SIZES_SQL, vec![]),
                self.query(introspect::INDEX_USAGE_SQL, vec![]),
            );
            let mut table_sizes = introspect::parse_table_sizes(&table_sizes?);
            let counts = futures::future::join_all(table_sizes.iter().map(|t| {
                let sql = introspect::row_count_sql(&t.name);
                async move { self.query(&sql, vec![]).await }
            }))
            .await;
            for (table, count) in table_sizes.iter_mut().zip(counts) {
                if let Ok(r) = count {
                    table.row_count = introspect::parse_row_count(&r);
                }
            }
            Ok(DatabaseStatistics {
                overview: introspect::parse_overview(&overview?),
                table_sizes,
                index_usage: introspect::parse_index_usage(&index_usage?),
            })
        }

        /// `EXPLAIN QUERY PLAN`. SQLite has no analyzing EXPLAIN, so with
        /// `analyze` the statement itself runs first (it **executes**, as
        /// Postgres's `EXPLAIN ANALYZE` does, writes included), timed here
        /// (moved from TsEngineClient): the root gets the actual row count
        /// and time, and the result the execution time. There is no
        /// per-operator breakdown, and no estimate.
        async fn explain(
            &self,
            sql: &str,
            params: Vec<Value>,
            analyze: bool,
        ) -> Result<ExplainResult, DbError> {
            let measured = if analyze {
                let (result, ms) = crate::timing::timed(self.query(sql, params.clone())).await;
                Some((result?.rows.len(), ms))
            } else {
                None
            };
            let r = self.query(&introspect::explain_sql(sql), params).await?;
            let mut explain = introspect::parse_explain(&r, analyze);
            if let Some((rows, ms)) = measured {
                explain.plan.actual_rows = Some(rows as f64);
                explain.plan.actual_total_time = Some(ms);
                explain.execution_time = Some(ms);
            }
            Ok(explain)
        }
    },
    read_only = {
        /// On a connection of its own, opened read-only with `PRAGMA
        /// query_only = ON`, after the gate in `read_only.rs` checked the
        /// SQL is one statement SQLite calls read-only. See
        /// [`query_read_only`].
        async fn query_read_only(
            &self,
            sql: &str,
            params: Vec<Value>,
        ) -> Result<QueryResult, DbError> {
            query_read_only(&self.pool, sql, &params).await
        }
    }
);

/// The read-only path (plan: AI safety, Decision 1):
///
/// 1. A connection of its own, never from the pool, opened from the pool's
///    options with `.read_only(true)` (`SQLITE_OPEN_READONLY`). For
///    `sqlite::memory:` those options name sqlx's shared-cache database
///    (`file:sqlx-in-memory-N`), so it sees the same tables; the pool keeps
///    one connection open for good, so closing this one doesn't drop the
///    database. The open flag doesn't stop writes to that shared-cache
///    database, which is what `query_only` is for.
/// 2. `PRAGMA query_only = ON`, which refuses writes on every database.
/// 3. An authorizer (`read_only::deny_settings`) that refuses every PRAGMA
///    but a few query ones, `ATTACH` and `DETACH` for the rest of the
///    connection's life: `PRAGMA hard_heap_limit`, `soft_heap_limit` and
///    `temp_store_directory` change the whole process, and SQLite calls
///    them read-only.
/// 4. The gate: exactly one statement, and `sqlite3_stmt_readonly` is true
///    for it. sqlx would otherwise run every statement in the string
///    (`PRAGMA query_only = OFF; INSERT …`), and neither the flag nor the
///    pragma stops `VACUUM INTO 'file'`.
/// 5. The SQL through `fetch_capped` on that connection, which is then
///    closed.
///
/// A progress handler interrupts the statement once the call's future is
/// dropped (a cancel), so it stops holding its locks at once. Without it,
/// dropping the connection waits for sqlx's worker to finish the statement.
async fn query_read_only(
    pool: &Pool<Sqlite>,
    sql: &str,
    params: &[Value],
) -> Result<QueryResult, DbError> {
    let options = (*pool.connect_options()).clone().read_only(true);
    let mut conn = SqliteConnection::connect_with(&options)
        .await
        .map_err(DbError::connection_error)?;
    let result = run_read_only(&mut conn, sql, params).await;
    if let Err(e) = conn.close().await {
        log::warn!(activity = "db.query_read_only"; "Closing the read-only connection failed: {e}");
    }
    result
}

/// Steps 2–5 of [`query_read_only`] on its connection.
async fn run_read_only(
    conn: &mut SqliteConnection,
    sql: &str,
    params: &[Value],
) -> Result<QueryResult, DbError> {
    sqlx::Executor::execute(&mut *conn, "PRAGMA query_only = ON")
        .await
        .map_err(DbError::query_error)?;
    // Set when this future is dropped, which the progress handler turns
    // into SQLITE_INTERRUPT. A normal return sets it too, after the rows
    // are in.
    let cancelled = Arc::new(AtomicBool::new(false));
    let _interrupt_on_drop = SetOnDrop(cancelled.clone());
    {
        // The handle lock is released at the end of this block, before
        // `fetch_capped` needs the connection's worker.
        let mut handle = conn.lock_handle().await.map_err(DbError::query_error)?;
        handle.set_progress_handler(PROGRESS_OPS, move || !cancelled.load(Ordering::Relaxed));
        read_only::deny_settings(&mut handle);
        read_only::check(&mut handle, sql)?;
    }
    fetch_capped(&mut *conn, sql, params)
        .await
        .map_err(read_only::map_read_only_error)
}

/// SQLite virtual machine instructions between progress-handler calls: often
/// enough that a cancel lands within a millisecond or so, rare enough to cost
/// nothing measurable.
const PROGRESS_OPS: i32 = 1000;

/// Sets its flag when dropped.
struct SetOnDrop(Arc<AtomicBool>);

impl Drop for SetOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn filename(conn_str: &str) -> PathBuf {
        SqliteConnectOptions::from_str(conn_str)
            .unwrap()
            .get_filename()
            .to_path_buf()
    }

    #[test]
    fn in_memory_databases_keep_a_connection_open() {
        for conn_str in [
            "sqlite::memory:",
            "sqlite://:memory:",
            ":memory:",
            "sqlite:shared?mode=memory&cache=shared",
            "sqlite::memory:?cache=shared",
        ] {
            let o = pool_options(conn_str);
            assert_eq!(
                (
                    o.get_min_connections(),
                    o.get_idle_timeout(),
                    o.get_max_lifetime()
                ),
                (1, None, None),
                "{conn_str}"
            );
        }
        let defaults = SqlitePoolOptions::new();
        for conn_str in [
            "sqlite:///tmp/app.db",
            "sqlite:app.db?mode=rwc",
            "sqlite:memory.db",
        ] {
            let o = pool_options(conn_str);
            assert_eq!(
                (
                    o.get_min_connections(),
                    o.get_idle_timeout(),
                    o.get_max_lifetime()
                ),
                (
                    defaults.get_min_connections(),
                    defaults.get_idle_timeout(),
                    defaults.get_max_lifetime()
                ),
                "{conn_str}"
            );
        }
    }

    /// Review item (Task 3): in sqlx's shared-cache in-memory database, a
    /// pool connection holding a write lock on a table makes a reader's
    /// step return SQLITE_LOCKED, not SQLITE_BUSY. sqlx builds libsqlite3
    /// with `unlock_notify` and waits for the writer instead of returning
    /// the error, so the read-only query waits, exactly as a pool query
    /// does, and returns the committed rows once the writer commits. The
    /// wait is bounded by the writer's transaction; the driver never leaves
    /// one open across calls (`transaction` runs its statements and
    /// commits in one call).
    #[tokio::test]
    async fn in_memory_read_waits_for_a_writer_then_returns() {
        use sqlx::Executor;

        let config: ConnectConfig = serde_json::from_value(serde_json::json!({
            "driver": "sqlite",
            "connection_string": "sqlite::memory:"
        }))
        .unwrap();
        let driver = SqlxDriver::connect(&config).await.unwrap();
        driver
            .execute("CREATE TABLE t (n INTEGER)", vec![])
            .await
            .unwrap();
        driver
            .execute("INSERT INTO t VALUES (1)", vec![])
            .await
            .unwrap();

        let mut writer = driver.pool.acquire().await.unwrap();
        (&mut *writer).execute("BEGIN IMMEDIATE").await.unwrap();
        (&mut *writer)
            .execute("INSERT INTO t VALUES (2)")
            .await
            .unwrap();

        let committed = AtomicBool::new(false);
        let read = async {
            let result = driver
                .query_read_only("SELECT n FROM t ORDER BY n", vec![])
                .await;
            (committed.load(Ordering::SeqCst), result)
        };
        let commit = async {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            committed.store(true, Ordering::SeqCst);
            (&mut *writer).execute("COMMIT").await.unwrap();
        };
        let ((returned_after_commit, result), ()) =
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                futures::join!(read, commit)
            })
            .await
            .expect("the read still waits after the commit");
        assert!(returned_after_commit, "the read didn't wait for the writer");
        assert_eq!(
            result.unwrap().rows,
            vec![vec![Value::Int(1)], vec![Value::Int(2)]]
        );
        drop(writer);
        driver.close().await.unwrap();
    }

    #[test]
    fn windows_path_has_no_unc_prefix() {
        let path = r"C:\Autoit\Project\no_enc\s17.db";
        assert_eq!(filename(&format!("sqlite://{path}")), Path::new(path));
        assert_eq!(filename(&format!("sqlite:{path}")), Path::new(path));
    }

    #[test]
    fn unix_path_parent_dir() {
        assert_eq!(
            database_parent_dir("sqlite:///Users/me/data/app.db").unwrap(),
            Some(PathBuf::from("/Users/me/data"))
        );
    }

    #[test]
    fn query_params_are_not_part_of_the_path() {
        assert_eq!(
            database_parent_dir("sqlite:///tmp/x/app.db?mode=rwc").unwrap(),
            Some(PathBuf::from("/tmp/x"))
        );
    }

    #[test]
    fn in_memory_and_bare_filenames_have_no_parent_dir() {
        assert_eq!(database_parent_dir("sqlite::memory:").unwrap(), None);
        assert_eq!(database_parent_dir("sqlite://app.db").unwrap(), None);
    }
}
