//! The engine plugin boundary.
//!
//! Each database engine is its own crate (`seaquel-engine-postgres`, …) that
//! implements [`Engine`] and [`Driver`]. Seaquel Core looks engines up in an
//! [`EngineRegistry`] by the `driver` field of [`ConnectConfig`], so nothing
//! outside Core's default-plugin list names an engine crate.
//!
//! This crate is pure: it must keep building for `wasm32-unknown-unknown`. The
//! [`impl_sqlx_driver!`] macro is only expanded by native engine crates.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub use seaquel_runtime::{BoxStream, MaybeSend, MaybeSync};
pub use seaquel_types::{
    BatchStatement, ConnectConfig, ConnectResult, DatabaseStatistics, DbError, DriverType,
    ExecuteResult, ExpectRows, ExplainResult, QueryResult, SchemaColumn, SchemaIndex, SchemaTable,
    SqlWithBindings, StreamBatch, Value, MAX_SAFE_INTEGER,
};
pub use tokio_util::sync::CancellationToken;

pub mod crud;
pub mod ddl;
mod dialect;
pub mod introspect;
mod sqlx_driver;

pub use dialect::{CastMap, Dialect, RowValues};

#[doc(hidden)]
pub mod __private {
    pub use async_stream;
    pub use async_trait;
    pub use futures;
    pub use log;

    /// `impl_sqlx_driver!`: name the column a cell failed to decode in.
    pub fn in_column(mut e: crate::DbError, column: &str) -> crate::DbError {
        e.message = format!("{} (column \"{column}\")", e.message);
        e
    }

    /// `impl_sqlx_driver!`'s `stream_start` when the engine gives none:
    /// the connection as it is, with nothing to stop on the server.
    pub async fn plain_stream_start<P: ?Sized, C>(_pool: &P, conn: C, _sql: &str) -> Plain<C> {
        Plain(conn)
    }

    /// A pooled connection with no [`crate::RunningStatement`] behaviour.
    pub struct Plain<C>(pub C);

    impl<C> std::ops::Deref for Plain<C> {
        type Target = C;
        fn deref(&self) -> &C {
            &self.0
        }
    }

    impl<C> std::ops::DerefMut for Plain<C> {
        fn deref_mut(&mut self) -> &mut C {
            &mut self.0
        }
    }

    impl<C> crate::RunningStatement for Plain<C> {
        fn finish(&mut self) {}
    }
}

/// The start of `sql` that a cancel guard compares with what the server
/// reports the session running, or `None` when too little of it would be
/// reliable (fewer than [`STATEMENT_PREFIX_MIN_CHARS`] characters): then
/// the guard falls back to the session alone.
///
/// Servers report a statement's text differently from what was sent:
/// leading whitespace, and a trailing `;` and whitespace, are stripped
/// (MySQL, MariaDB), bound parameters may
/// be shown expanded (MySQL with the binary log on), and characters
/// outside the Basic Multilingual Plane are shown as `?` (MariaDB) or
/// can't be compared at all (MySQL's utf8mb3 `PROCESSLIST`). So the prefix
/// is taken from `sql` without its trailing `;`s and whitespace; it starts
/// after ASCII whitespace and ends before the first
/// `stop_at` character, the first character above U+FFFF, or after
/// `max_chars` characters.
pub fn statement_prefix(sql: &str, stop_at: Option<char>, max_chars: usize) -> Option<String> {
    let prefix: String = sql
        .trim_end_matches(|c: char| c == ';' || c.is_whitespace())
        .trim_start_matches(STATEMENT_LEADING_WHITESPACE)
        .chars()
        .take_while(|&c| Some(c) != stop_at && u32::from(c) <= 0xFFFF)
        .take(max_chars)
        .collect();
    (prefix.chars().count() >= STATEMENT_PREFIX_MIN_CHARS).then_some(prefix)
}

/// The whitespace [`statement_prefix`] skips at the start of a statement.
pub const STATEMENT_LEADING_WHITESPACE: &[char] = &[' ', '\t', '\n', '\r', '\x0B', '\x0C'];

/// The shortest [`statement_prefix`] worth comparing.
pub const STATEMENT_PREFIX_MIN_CHARS: usize = 8;

/// The connection a sqlx engine's `query_stream` runs its statement on
/// (`impl_sqlx_driver!`'s `stream_start`), dereferencing to the pooled
/// connection. Dropped before [`RunningStatement::finish`] (a cancel, a
/// disconnect, the consumer going away, a decode error), it stops the
/// statement on the server if the engine can: the server keeps running a
/// statement whose client just stops reading.
pub trait RunningStatement {
    /// The statement is over on the server (it read every row, or the
    /// server ended it with an error): nothing to stop.
    fn finish(&mut self);
}

/// Cap for non-streaming `query()` results. Streaming `query_stream` is
/// unbounded by design (the UI paginates). This cap exists to prevent a
/// `SELECT * FROM big_table` submitted through the non-streaming path from
/// OOM-killing the process. Override with `SEAQUEL_MAX_QUERY_ROWS`.
pub fn max_query_rows() -> usize {
    use std::sync::OnceLock;
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("SEAQUEL_MAX_QUERY_ROWS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|v: &usize| *v > 0)
            .unwrap_or(100_000)
    })
}

/// How many rows (and, optionally, bytes) a query may collect, and what
/// happens past that. Build it with [`RowCap::fail`], [`RowCap::truncate`]
/// or [`RowCap::read_only`], then feed each row's size to
/// [`RowCap::check`] (or [`RowCap::admit`]) before keeping it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowCap {
    rows: usize,
    truncate: bool,
    max_bytes: Option<usize>,
}

/// What [`RowCap::check`] says about one more row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admit {
    /// Keep it.
    Keep,
    /// Stop here and return the rows so far, `truncated`.
    Truncate,
    /// Stop with `RESULT_TOO_LARGE`: more rows than this.
    TooLarge(usize),
}

impl RowCap {
    /// Past `rows` rows the query fails with `RESULT_TOO_LARGE`.
    pub const fn fail(rows: usize) -> Self {
        RowCap {
            rows,
            truncate: false,
            max_bytes: None,
        }
    }

    /// The first `rows` rows come back, with `truncated` set when the query
    /// had more.
    pub const fn truncate(rows: usize) -> Self {
        RowCap {
            rows,
            truncate: true,
            max_bytes: None,
        }
    }

    /// Also stop, `truncated`, once the rows kept so far hold `max_bytes`
    /// (by [`row_bytes`]), whatever the row limit's mode. The row that
    /// crosses the budget is kept, so a result always has its first row
    /// and holds at most `max_bytes` plus one row. `None` sets no budget.
    #[must_use]
    pub const fn with_max_bytes(mut self, max_bytes: Option<usize>) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// The cap for [`Driver::query_read_only`] with its `max_rows`: without
    /// it, fail past [`max_query_rows`]; with it, truncate at `max_rows`, but
    /// never past [`max_query_rows`] (the cap guards memory, and a
    /// `max_rows` above it truncates there). `max_bytes` is
    /// [`ReadOnlyOptions::max_bytes`] (see [`RowCap::with_max_bytes`]).
    pub fn read_only(max_rows: Option<usize>, max_bytes: Option<usize>) -> Self {
        match max_rows {
            None => RowCap::fail(max_query_rows()),
            Some(n) => RowCap::truncate(n.min(max_query_rows())),
        }
        .with_max_bytes(max_bytes)
    }

    /// The most rows a result may hold.
    pub fn limit(self) -> usize {
        self.rows
    }

    /// The byte budget, if any.
    pub fn max_bytes(self) -> Option<usize> {
        self.max_bytes
    }

    /// What to do with one more row when `kept` rows holding `kept_bytes`
    /// (the sum of their [`row_bytes`]) are already in. The row limit comes
    /// first, so a query past both fails under [`RowCap::fail`].
    pub fn check(self, kept: usize, kept_bytes: usize) -> Admit {
        if kept >= self.rows {
            if self.truncate {
                Admit::Truncate
            } else {
                Admit::TooLarge(self.rows)
            }
        } else if self.max_bytes.is_some_and(|max| kept_bytes >= max) {
            Admit::Truncate
        } else {
            Admit::Keep
        }
    }

    /// [`RowCap::check`] as a `Result`: `Ok(true)` keeps the row,
    /// `Ok(false)` stops (the result is truncated), and `Err` is
    /// `RESULT_TOO_LARGE`.
    pub fn admit(self, kept: usize, kept_bytes: usize) -> Result<bool, DbError> {
        match self.check(kept, kept_bytes) {
            Admit::Keep => Ok(true),
            Admit::Truncate => Ok(false),
            Admit::TooLarge(n) => Err(DbError::result_too_large(n)),
        }
    }
}

/// Roughly how much memory a decoded cell holds: the `Value` itself plus
/// what it owns on the heap (text, bytes, JSON strings, array items). An
/// estimate for [`RowCap`]'s byte budget, not an exact allocation size.
pub fn cell_bytes(value: &Value) -> usize {
    const SLOT: usize = std::mem::size_of::<Value>();
    SLOT + match value {
        Value::Null | Value::Bool(_) | Value::Int(_) | Value::Float(_) => 0,
        Value::Decimal(s) | Value::Text(s) => s.len(),
        Value::Bytes(b) => b.len(),
        Value::Json(j) => json_bytes(j),
        Value::Array(items) => items.iter().map(cell_bytes).sum(),
    }
}

/// [`cell_bytes`] for a JSON value: its strings and keys, plus a slot per
/// node.
fn json_bytes(value: &serde_json::Value) -> usize {
    const SLOT: usize = std::mem::size_of::<serde_json::Value>();
    SLOT + match value {
        serde_json::Value::String(s) => s.len(),
        serde_json::Value::Array(items) => items.iter().map(json_bytes).sum(),
        serde_json::Value::Object(map) => map.iter().map(|(k, v)| k.len() + json_bytes(v)).sum(),
        _ => 0,
    }
}

/// The sum of a row's [`cell_bytes`].
pub fn row_bytes(row: &[Value]) -> usize {
    row.iter().map(cell_bytes).sum()
}

/// A result collected under a [`RowCap`]: what [`Driver::query_read_only`]
/// returns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CappedResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// The query had more rows than the [`RowCap`] let through: past a
    /// [`RowCap::truncate`] limit, or past its byte budget. Never set for a
    /// [`RowCap::fail`] limit alone.
    pub truncated: bool,
}

/// A result that nothing cut short.
impl From<QueryResult> for CappedResult {
    fn from(r: QueryResult) -> Self {
        CappedResult {
            columns: r.columns,
            rows: r.rows,
            truncated: false,
        }
    }
}

impl From<CappedResult> for QueryResult {
    fn from(r: CappedResult) -> Self {
        QueryResult {
            columns: r.columns,
            rows: r.rows,
        }
    }
}

/// How [`Driver::query_read_only_with`] runs a query. Build it from
/// `ReadOnlyOptions::default()` and the `with_*` methods.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadOnlyOptions {
    /// See [`Driver::query_read_only`]'s `max_rows`.
    pub max_rows: Option<usize>,
    /// A byte budget for the rows kept: once the cells decoded so far add
    /// up to this many bytes (by [`cell_bytes`]), the driver stops fetching
    /// and returns what it has with [`CappedResult::truncated`] set, as it
    /// does at `max_rows`. The row that crosses the budget is kept. A
    /// single cell can't be split, so one row can still bring a cell as
    /// large as the database allows (up to 1 GB on Postgres, `max_allowed_packet`
    /// on MySQL, `SQLITE_MAX_LENGTH` on SQLite, 2 GB on SQL Server). `None`
    /// sets none. The MCP server passes about 8 MB, above its 4 MB output
    /// cap; the in-app AI passes none.
    pub max_bytes: Option<usize>,
    /// A limit the database itself enforces on the statement, so a query
    /// the caller gave up on stops on the server too: Postgres's `SET LOCAL
    /// statement_timeout`, MySQL's `max_execution_time`, MariaDB's
    /// `max_statement_time`. Past it the call fails with [`TIMEOUT`].
    ///
    /// It doesn't replace the caller's own deadline: a query waiting on
    /// something the server doesn't count (a lock on MySQL, the network)
    /// can outlive it. Engines whose query stops when the call's future is
    /// dropped may ignore it, since dropping the call is the caller's
    /// deadline (SQLite, DuckDB). `None` sets no limit.
    pub timeout: Option<Duration>,
}

impl ReadOnlyOptions {
    #[must_use]
    pub fn with_max_rows(mut self, max_rows: Option<usize>) -> Self {
        self.max_rows = max_rows;
        self
    }

    #[must_use]
    pub fn with_timeout(mut self, timeout: Option<Duration>) -> Self {
        self.timeout = timeout;
        self
    }

    #[must_use]
    pub fn with_max_bytes(mut self, max_bytes: Option<usize>) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// The [`RowCap`] these options ask for: [`RowCap::read_only`].
    pub fn row_cap(&self) -> RowCap {
        RowCap::read_only(self.max_rows, self.max_bytes)
    }
}

/// The code of a statement the database stopped at
/// [`ReadOnlyOptions::timeout`].
pub const TIMEOUT: &str = "TIMEOUT";

/// A statement the database stopped at its timeout, with its message.
pub fn timeout_error(message: impl std::fmt::Display) -> DbError {
    DbError {
        message: message.to_string(),
        code: TIMEOUT.to_string(),
    }
}

/// The refusal for more than one statement in a read-only EXPLAIN.
pub const EXPLAIN_ONE_STATEMENT: &str = "EXPLAIN runs on one statement at a time";

/// The error an engine returns for an operation it hasn't implemented yet.
pub fn not_supported(what: &str) -> DbError {
    DbError {
        message: format!("{what} is not supported by this engine yet"),
        code: "NOT_SUPPORTED".to_string(),
    }
}

/// An open connection (or pool) to one database.
#[seaquel_runtime::async_trait]
pub trait Driver: MaybeSend + MaybeSync {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError>;

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError>;

    /// Execute multiple statements in a single transaction on one connection.
    ///
    /// Drivers **must** override this to provide real atomicity. The default
    /// returns an error rather than silently running each statement on its
    /// own pooled connection — that was the old behaviour and it let a
    /// mid-batch failure leave earlier writes committed. Loudly failing here
    /// surfaces the gap instead of producing half-written data.
    ///
    /// Before COMMIT, every statement's affected rows must pass
    /// [`BatchStatement::check_affected`]; on a shortfall the driver rolls
    /// back and returns that `NO_ROWS_AFFECTED` error.
    async fn transaction(&self, _statements: Vec<BatchStatement>) -> Result<(), DbError> {
        Err(DbError {
            message: "transactions are not supported by this driver".to_string(),
            code: "TRANSACTION_NOT_SUPPORTED".to_string(),
        })
    }

    /// Stream query results in batches.
    ///
    /// Drivers that genuinely stream (the sqlx-based ones) override this and
    /// stop fetching as soon as `cancel` fires. The default runs `query()` and
    /// emits a single terminal batch, so DuckDB and MSSQL get a
    /// correct-but-non-streaming path for free; it can't interrupt `query()`,
    /// so it ignores `cancel` and Core drops the result instead.
    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        let _ = cancel;
        Box::pin(async_stream::try_stream! {
            let result = self.query(&sql, params).await?;
            yield StreamBatch {
                columns: Some(result.columns),
                rows: result.rows,
                is_final: true,
                truncated: false,
            };
        })
    }

    /// Run one query that must not change anything: the AI's `run_query`
    /// tool and dashboard widgets (Core's `read_only` query option).
    ///
    /// The default fails with `NOT_SUPPORTED`, so an engine that doesn't
    /// implement it fails closed. Never implement it by calling
    /// [`Driver::query`]. An implementation must meet this contract:
    ///
    /// - **Exactly one statement runs.** Input holding more than one is
    ///   refused before anything runs, or refused by the database as a whole.
    /// - **The database refuses writes**, through the engine's own read-only
    ///   mode (a read-only transaction, session or connection). Where the
    ///   engine has none (SQL Server), everything runs in a transaction that
    ///   is always rolled back, and a query that ends that transaction
    ///   itself is reported.
    /// - **Nothing the query does to its session outlives the call.** The
    ///   connection it ran on is closed, or is one the engine owns for this
    ///   path alone and is left as it was found. A pooled or shared
    ///   connection is never handed back read-only, inside a transaction, or
    ///   with changed settings, locks or prepared statements.
    /// - **Dropping the future cancels the query.** It stops as far as the
    ///   engine allows, and the driver stays usable for the next call.
    ///   Cancellation is drop-only: no `CancellationToken` is passed, so an
    ///   engine whose query runs where dropping the future doesn't reach it
    ///   must interrupt it from a `Drop` guard held by the future. DuckDB
    ///   (the query runs on `spawn_blocking`) calls the clone's interrupt
    ///   handle from that guard; SQL Server runs each call on a connection
    ///   of its own, so a drop at any await drops that connection and the
    ///   server rolls back. The sqlx engines stop when their connection is
    ///   dropped (closed, with `close_on_drop()`).
    /// - **Refusals are `READ_ONLY`.** The engine's read-only refusal (and
    ///   the driver's own, e.g. "one statement at a time") comes back as
    ///   [`DbError::read_only`] with the database's message. Other errors
    ///   keep their usual codes.
    /// - **Rows are capped** by [`RowCap::read_only`]`(max_rows)`. Without
    ///   `max_rows`, like [`Driver::query`]: past [`max_query_rows`] it
    ///   fails with `RESULT_TOO_LARGE`. With it, the engine stops once it
    ///   has seen `max_rows + 1` rows, returns the first `max_rows` and sets
    ///   [`CappedResult::truncated`]. Stopping early never skips a check the
    ///   engine makes after the query (SQL Server reads the rest of the
    ///   response and still checks the transaction).
    /// - **Bytes are capped too** when [`query_read_only_with`] gets
    ///   [`ReadOnlyOptions::max_bytes`]: the engine sums [`row_bytes`] of the
    ///   rows it keeps and stops the same way once they reach it. This
    ///   method has no byte budget; the drivers that honour one override
    ///   `query_read_only_with`.
    ///
    /// [`query_read_only_with`]: Driver::query_read_only_with
    ///
    /// Core runs the AI's token check (`seaquel_sql::read_only`) before
    /// calling this. Drivers must not rely on it: the testkit's read-only
    /// harness calls them directly with SQL the check would refuse.
    async fn query_read_only(
        &self,
        _sql: &str,
        _params: Vec<Value>,
        _max_rows: Option<usize>,
    ) -> Result<CappedResult, DbError> {
        Err(not_supported("Running read-only queries"))
    }

    /// [`Driver::query_read_only`] with [`ReadOnlyOptions`], which is what
    /// Core calls. The default runs `query_read_only` with the options'
    /// `max_rows` and ignores their `max_bytes` (every engine in this repo
    /// overrides it) and their `timeout`, which is right only for an
    /// engine whose query stops when its future is dropped (see
    /// [`ReadOnlyOptions::timeout`]). An engine where the server keeps
    /// running a dropped query (Postgres, MySQL) overrides this and sets the
    /// timeout on the server.
    async fn query_read_only_with(
        &self,
        sql: &str,
        params: Vec<Value>,
        options: ReadOnlyOptions,
    ) -> Result<CappedResult, DbError> {
        self.query_read_only(sql, params, options.max_rows).await
    }

    /// A plain EXPLAIN (never ANALYZE) of one statement that must not change
    /// anything: the MCP server's `explain_query`. The default fails with
    /// `NOT_SUPPORTED`, so an engine that doesn't implement it fails closed.
    /// Never implement it by calling [`Driver::explain`] where planning can
    /// run user code (Postgres folds immutable functions, MariaDB evaluates
    /// constant subqueries, `nextval` included). The contract:
    ///
    /// - **Exactly one statement.** More than one is refused with
    ///   [`DbError::read_only`] (e.g. [`EXPLAIN_ONE_STATEMENT`]) before
    ///   anything runs, or by the database as a whole. Today's `explain`
    ///   on SQLite runs every statement, and DuckDB's runs all but the first
    ///   for real.
    /// - **Whatever planning runs is read-only**, in the same read-only
    ///   transaction or session as [`Driver::query_read_only`] where the
    ///   engine plans by running code; an engine whose EXPLAIN only compiles
    ///   (SQL Server's SHOWPLAN) may keep its EXPLAIN.
    /// - **Nothing outlives the call and dropping it cancels it**, as for
    ///   `query_read_only`, and `timeout` is [`ReadOnlyOptions::timeout`].
    async fn explain_read_only(
        &self,
        _sql: &str,
        _params: Vec<Value>,
        _timeout: Option<Duration>,
    ) -> Result<ExplainResult, DbError> {
        Err(not_supported("Read-only EXPLAIN"))
    }

    async fn close(&self) -> Result<(), DbError>;

    // ── Introspection ──
    //
    // Engines whose dialect still lives in TypeScript leave these alone; the
    // frontend's TypeScript client handles them.

    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        Err(not_supported("Listing schemas"))
    }

    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        Err(not_supported("Loading the schema"))
    }

    async fn table_metadata(
        &self,
        _schema: &str,
        _table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        Err(not_supported("Loading table metadata"))
    }

    async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
        Err(not_supported("Database statistics"))
    }

    async fn explain(
        &self,
        _sql: &str,
        _params: Vec<Value>,
        _analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        Err(not_supported("EXPLAIN"))
    }
}

/// Limits the interface puts on what an engine opens, next to the
/// [`ConnectConfig`] the user's target produced. Core sets them from its
/// builder (the web server's are tighter than the desktop's); they never
/// come from the wire.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OpenOptions {
    /// The most database connections one open [`Driver`] holds at once.
    /// `None` keeps the engine's default: a sqlx pool of 10 (Postgres,
    /// MySQL, SQLite); for SQL Server, its one session plus 4 read-only
    /// connections. Postgres, MySQL and SQL Server honour it; SQLite and
    /// DuckDB (local files, desktop only) ignore it. At least 1.
    pub max_pool_size: Option<u32>,
}

/// A database engine plugin: knows how to open [`Driver`]s for one `driver`
/// value on the wire.
#[seaquel_runtime::async_trait]
pub trait Engine: MaybeSend + MaybeSync {
    /// Stable id. Must equal [`DriverType::as_str`] for the driver it serves.
    fn id(&self) -> &'static str;

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError>;

    /// [`Engine::open`] under `options`, which is what Core calls. The
    /// default ignores them; an engine with a connection pool overrides
    /// this (and `open` calls it with the defaults).
    async fn open_with(
        &self,
        config: &ConnectConfig,
        options: OpenOptions,
    ) -> Result<Arc<dyn Driver>, DbError> {
        let _ = options;
        self.open(config).await
    }

    /// The engine's SQL dialect, when it has moved to Rust.
    fn dialect(&self) -> Option<&dyn Dialect> {
        None
    }
}

/// The engines compiled into this build, keyed by id.
#[derive(Default, Clone)]
pub struct EngineRegistry {
    engines: HashMap<&'static str, Arc<dyn Engine>>,
}

impl EngineRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an engine.
    ///
    /// # Panics
    ///
    /// If an engine with the same id is already registered. Two engines
    /// claiming one id is a build mistake, not a runtime condition.
    pub fn register(&mut self, engine: Arc<dyn Engine>) {
        let id = engine.id();
        if self.engines.insert(id, engine).is_some() {
            panic!("engine \"{id}\" registered twice");
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Engine>> {
        self.engines.get(id).cloned()
    }

    /// Registered ids, sorted.
    pub fn ids(&self) -> Vec<&'static str> {
        let mut ids: Vec<_> = self.engines.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    /// Open a driver with the engine matching `config.driver`.
    pub async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        let id = config.driver.as_str();
        let engine = self
            .get(id)
            .ok_or_else(|| DbError::engine_not_available(id))?;
        engine.open(config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use futures::StreamExt;
    use serde_json::json;

    #[test]
    fn decode_errors_name_the_column() {
        let e = __private::in_column(DbError::query_error("can't decode a TIME value"), "t");
        assert_eq!(e.code, "QUERY_ERROR");
        assert_eq!(
            e.message,
            "Query failed: can't decode a TIME value (column \"t\")"
        );
    }

    struct FakeDriver;

    #[seaquel_runtime::async_trait]
    impl Driver for FakeDriver {
        async fn query(&self, _sql: &str, _params: Vec<Value>) -> Result<QueryResult, DbError> {
            Ok(QueryResult {
                columns: vec!["a".into()],
                rows: vec![vec![Value::Int(1)]],
            })
        }

        async fn execute(&self, _sql: &str, _params: Vec<Value>) -> Result<ExecuteResult, DbError> {
            Ok(ExecuteResult {
                rows_affected: 0,
                last_insert_id: None,
            })
        }

        async fn close(&self) -> Result<(), DbError> {
            Ok(())
        }
    }

    struct FakeEngine(&'static str);

    #[seaquel_runtime::async_trait]
    impl Engine for FakeEngine {
        fn id(&self) -> &'static str {
            self.0
        }

        async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
            Ok(Arc::new(FakeDriver))
        }
    }

    fn config(driver: &str) -> ConnectConfig {
        serde_json::from_value(json!({ "driver": driver })).unwrap()
    }

    #[test]
    fn open_dispatches_on_the_driver_field() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        let driver = block_on(registry.open(&config("sqlite"))).unwrap();
        let result = block_on(driver.query("SELECT 1", vec![])).unwrap();
        assert_eq!(result.columns, vec!["a"]);
    }

    #[test]
    fn open_reports_a_missing_engine() {
        let registry = EngineRegistry::new();
        let err = block_on(registry.open(&config("postgres"))).err().unwrap();
        assert_eq!(err.code, "ENGINE_NOT_AVAILABLE");
        assert!(err.message.contains("\"postgres\""), "{}", err.message);
    }

    #[test]
    fn ids_are_sorted() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        registry.register(Arc::new(FakeEngine("duckdb")));
        assert_eq!(registry.ids(), vec!["duckdb", "sqlite"]);
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn registering_an_id_twice_panics() {
        let mut registry = EngineRegistry::new();
        registry.register(Arc::new(FakeEngine("sqlite")));
        registry.register(Arc::new(FakeEngine("sqlite")));
    }

    #[test]
    fn introspection_defaults_are_not_supported() {
        let d = FakeDriver;
        let codes = [
            block_on(d.list_schemas()).err().unwrap(),
            block_on(d.schema_tables()).err().unwrap(),
            block_on(d.table_metadata("public", "t")).err().unwrap(),
            block_on(d.statistics()).err().unwrap(),
            block_on(d.explain("SELECT 1", vec![], false))
                .err()
                .unwrap(),
        ];
        for err in codes {
            assert_eq!(err.code, "NOT_SUPPORTED");
            assert!(
                err.message.ends_with("is not supported by this engine yet"),
                "{}",
                err.message
            );
        }
    }

    /// Fail closed: a driver that implements `query` but not
    /// `query_read_only` must not run read-only queries through `query`.
    #[test]
    fn read_only_queries_are_not_supported_by_default() {
        let err = block_on(FakeDriver.query_read_only("SELECT 1", vec![], None))
            .err()
            .unwrap();
        assert_eq!(err.code, "NOT_SUPPORTED");
        assert_eq!(
            err.message,
            "Running read-only queries is not supported by this engine yet"
        );
        let err = block_on(FakeDriver.query_read_only_with(
            "SELECT 1",
            vec![],
            ReadOnlyOptions::default().with_timeout(Some(Duration::from_secs(1))),
        ))
        .err()
        .unwrap();
        assert_eq!(err.code, "NOT_SUPPORTED");
        let err = block_on(FakeDriver.explain_read_only("SELECT 1", vec![], None))
            .err()
            .unwrap();
        assert_eq!(err.code, "NOT_SUPPORTED");
        assert_eq!(
            err.message,
            "Read-only EXPLAIN is not supported by this engine yet"
        );
    }

    #[test]
    fn row_caps_fail_or_truncate() {
        let max = max_query_rows();
        assert_eq!(RowCap::read_only(None, None), RowCap::fail(max));
        assert_eq!(RowCap::read_only(Some(10), None), RowCap::truncate(10));
        assert_eq!(
            RowCap::read_only(Some(usize::MAX), None),
            RowCap::truncate(max)
        );
        assert!(RowCap::truncate(2).admit(1, 0).unwrap());
        assert!(!RowCap::truncate(2).admit(2, 0).unwrap());
        assert!(!RowCap::truncate(0).admit(0, 0).unwrap());
        assert!(RowCap::fail(2).admit(1, usize::MAX).unwrap());
        assert_eq!(
            RowCap::fail(2).admit(2, 0).unwrap_err().code,
            "RESULT_TOO_LARGE"
        );
    }

    #[test]
    fn a_byte_budget_truncates_under_either_row_mode() {
        let cap = RowCap::read_only(None, Some(100));
        assert_eq!(
            cap,
            RowCap::fail(max_query_rows()).with_max_bytes(Some(100))
        );
        assert_eq!(cap.check(0, 0), Admit::Keep);
        assert_eq!(cap.check(3, 99), Admit::Keep);
        // The row that crossed it was kept; the next one stops.
        assert_eq!(cap.check(4, 100), Admit::Truncate);
        assert_eq!(cap.check(4, 5_000), Admit::Truncate);
        // The row limit comes first.
        let cap = RowCap::fail(2).with_max_bytes(Some(100));
        assert_eq!(cap.check(2, 1_000), Admit::TooLarge(2));
        let cap = RowCap::read_only(Some(10), Some(100));
        assert_eq!(cap.check(10, 0), Admit::Truncate);
        assert_eq!(cap.check(1, 100), Admit::Truncate);
        assert_eq!(
            ReadOnlyOptions::default()
                .with_max_rows(Some(10))
                .with_max_bytes(Some(100))
                .row_cap(),
            RowCap::truncate(10).with_max_bytes(Some(100))
        );
    }

    #[test]
    fn cell_sizes_count_what_a_cell_holds() {
        let slot = std::mem::size_of::<Value>();
        assert_eq!(cell_bytes(&Value::Int(1)), slot);
        assert_eq!(cell_bytes(&Value::Text("x".repeat(1000))), slot + 1000);
        assert_eq!(cell_bytes(&Value::Bytes(vec![0; 64])), slot + 64);
        assert_eq!(
            cell_bytes(&Value::Array(vec![Value::Text("ab".into()), Value::Null])),
            slot + (slot + 2) + slot
        );
        let json = cell_bytes(&Value::Json(serde_json::json!({ "key": "x".repeat(500) })));
        assert!(json > 503, "{json}");
        assert_eq!(
            row_bytes(&[Value::Int(1), Value::Text("abc".into())]),
            2 * slot + 3
        );
    }

    #[test]
    fn engines_have_no_dialect_by_default() {
        assert!(FakeEngine("sqlite").dialect().is_none());
    }

    /// A dialect built from the generic builders, used as a trait object.
    struct TinyDialect;

    impl Dialect for TinyDialect {
        fn quote_ident(&self, id: &str) -> String {
            format!("\"{}\"", id.replace('"', "\"\""))
        }
        fn paginate(&self, sql: &str, limit: u64, offset: u64) -> String {
            format!("{sql} LIMIT {limit} OFFSET {offset}")
        }
        fn build_update(
            &self,
            schema: &str,
            table: &str,
            column: &str,
            value: Value,
            pks: &[String],
            row: &RowValues,
            casts: Option<&CastMap>,
        ) -> SqlWithBindings {
            let qi = |s: &str| self.quote_ident(s);
            crud::build_param_update(
                schema,
                table,
                column,
                value,
                pks,
                row,
                &qi,
                casts,
                &crud::dollar_placeholder,
            )
        }
        fn build_set_default(
            &self,
            schema: &str,
            table: &str,
            column: &str,
            pks: &[String],
            row: &RowValues,
            casts: Option<&CastMap>,
        ) -> SqlWithBindings {
            let qi = |s: &str| self.quote_ident(s);
            crud::build_param_set_default(
                schema,
                table,
                column,
                pks,
                row,
                &qi,
                casts,
                &crud::dollar_placeholder,
            )
        }
        fn build_insert(
            &self,
            schema: &str,
            table: &str,
            values: &[(String, Value)],
            casts: Option<&CastMap>,
        ) -> SqlWithBindings {
            let qi = |s: &str| self.quote_ident(s);
            crud::build_param_insert(schema, table, values, &qi, casts, &crud::dollar_placeholder)
        }
        fn build_delete(
            &self,
            schema: &str,
            table: &str,
            pks: &[String],
            row: &RowValues,
            casts: Option<&CastMap>,
        ) -> SqlWithBindings {
            let qi = |s: &str| self.quote_ident(s);
            crud::build_param_delete(
                schema,
                table,
                pks,
                row,
                &qi,
                casts,
                &crud::dollar_placeholder,
            )
        }
        fn create_table(&self, def: &seaquel_types::CreateTableDefinition) -> String {
            ddl::generate_create_table_ddl(def, &|s| self.quote_ident(s))
        }
        fn alter_table(
            &self,
            from: &seaquel_types::CreateTableDefinition,
            to: &seaquel_types::CreateTableDefinition,
        ) -> String {
            let opts = ddl::AlterTableOptions {
                qualify_drop_index: true,
                ..Default::default()
            };
            ddl::generate_alter_table_sql(from, to, &|s| self.quote_ident(s), opts)
        }
        fn column_types(&self) -> Vec<seaquel_types::ColumnTypeInfo> {
            vec![]
        }
        fn explain_sql(&self, sql: &str, analyze: bool) -> String {
            format!("EXPLAIN {}{sql}", if analyze { "ANALYZE " } else { "" })
        }
    }

    struct EngineWithDialect;

    #[seaquel_runtime::async_trait]
    impl Engine for EngineWithDialect {
        fn id(&self) -> &'static str {
            "postgres"
        }
        async fn open(&self, _config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
            Ok(Arc::new(FakeDriver))
        }
        fn dialect(&self) -> Option<&dyn Dialect> {
            Some(&TinyDialect)
        }
    }

    #[test]
    fn a_dialect_is_usable_through_the_engine() {
        let engine: Arc<dyn Engine> = Arc::new(EngineWithDialect);
        let d = engine.dialect().unwrap();
        let row: RowValues = vec![("id".into(), Value::Int(9))];
        let out = d.build_delete("public", "we\"ird", &["id".to_string()], &row, None);
        assert_eq!(
            out.sql,
            "DELETE FROM \"public\".\"we\"\"ird\" WHERE \"id\" = $1"
        );
        assert_eq!(out.bind_values, Some(vec![Value::Int(9)]));
        assert_eq!(
            d.paginate("SELECT 1", 10, 20),
            "SELECT 1 LIMIT 10 OFFSET 20"
        );
    }

    #[test]
    fn default_query_stream_emits_one_final_batch() {
        let driver = FakeDriver;
        let batches: Vec<_> = block_on(
            driver
                .query_stream("SELECT 1".into(), vec![], CancellationToken::new())
                .collect(),
        );
        assert_eq!(batches.len(), 1);
        let batch = batches.into_iter().next().unwrap().unwrap();
        assert_eq!(batch.columns, Some(vec!["a".to_string()]));
        assert_eq!(batch.rows, vec![vec![Value::Int(1)]]);
        assert!(batch.is_final);
    }
}

#[cfg(test)]
mod statement_prefix_tests {
    use super::statement_prefix;

    #[test]
    fn prefixes_follow_what_servers_report() {
        let p = |sql| statement_prefix(sql, Some('?'), 4096);
        assert_eq!(
            p("SELECT SLEEP(30) AS m").as_deref(),
            Some("SELECT SLEEP(30) AS m")
        );
        // Leading whitespace is skipped.
        assert_eq!(
            p("\n\n  \t SELECT 1 AS abc").as_deref(),
            Some("SELECT 1 AS abc")
        );
        // Up to the first parameter.
        assert_eq!(p("SELECT SLEEP(?) AS m").as_deref(), Some("SELECT SLEEP("));
        // Up to the first character above U+FFFF; BMP characters stay.
        assert_eq!(p("SELECT 'é\u{1F600}' AS e").as_deref(), Some("SELECT 'é"));
        // MySQL and MariaDB drop a trailing `;` and whitespace.
        assert_eq!(
            p("SELECT SLEEP(30) AS m;").as_deref(),
            Some("SELECT SLEEP(30) AS m")
        );
        assert_eq!(
            p("SELECT SLEEP(30) AS m ;  \n").as_deref(),
            Some("SELECT SLEEP(30) AS m")
        );
        assert_eq!(
            p("SELECT 1 AS abc;;\t;"),
            Some("SELECT 1 AS abc".to_string())
        );
        // Too short to trust.
        assert_eq!(p("SELECT ?"), None);
        assert_eq!(p("  \u{1F600} SELECT 1"), None);
        assert_eq!(p(""), None);
        // Without a stop character, `?` is text.
        assert_eq!(
            statement_prefix("SELECT '?' AS q", None, 4096).as_deref(),
            Some("SELECT '?' AS q")
        );
        assert_eq!(
            statement_prefix("SELECT 1234567890", None, 10).as_deref(),
            Some("SELECT 123")
        );
    }
}
