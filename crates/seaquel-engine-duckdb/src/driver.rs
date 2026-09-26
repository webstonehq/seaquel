use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use duckdb::arrow::array::{Array, StructArray};
use duckdb::{
    params_from_iter, types::Decimal as DuckDecimal, types::Value as DuckValue, Connection,
    InterruptHandle, Statement,
};
use tokio::sync::mpsc;

use log::warn;
use seaquel_engine::{
    BatchStatement, BoxStream, CancellationToken, ConnectConfig, DatabaseStatistics, DbError,
    Driver, ExecuteResult, ExpectRows, ExplainResult, QueryResult, SchemaColumn, SchemaIndex,
    SchemaTable, StreamBatch, Value,
};

use crate::blocking::{self, Op, Worker};
use crate::decode::{self, Kind};
use crate::introspect;

/// Convert a parameter into a DuckDB `Value` for binding. `Int` binds as
/// BIGINT, so integers are exact. `Decimal` binds as an exact DECIMAL when
/// it has at most 38 digits (see [`decimal_param`]), otherwise as its text,
/// which DuckDB casts to the other side's type (UHUGEINT, BIGNUM, DOUBLE).
/// `Json` binds as its JSON text. Arrays are rejected: duckdb-rs can't bind
/// LIST parameters; use an inline literal for these.
fn to_duckdb_param(v: &Value) -> Result<DuckValue, DbError> {
    Ok(match v {
        Value::Null => DuckValue::Null,
        Value::Bool(b) => DuckValue::Boolean(*b),
        Value::Int(i) => DuckValue::BigInt(*i),
        Value::Float(f) => DuckValue::Double(*f),
        Value::Decimal(s) => decimal_param(s).unwrap_or_else(|| DuckValue::Text(s.clone())),
        Value::Text(s) => DuckValue::Text(s.clone()),
        Value::Bytes(b) => DuckValue::Blob(b.clone()),
        Value::Json(j) => DuckValue::Text(j.to_string()),
        Value::Array(_) => {
            return Err(DbError::query_error("array parameters are not supported"));
        }
    })
}

/// `-12.50` as DECIMAL(4, 2), exactly. `None` for anything that isn't plain
/// decimal digits (`NaN`, `1e5`) or needs more than DuckDB's 38 digits.
fn decimal_param(s: &str) -> Option<DuckValue> {
    let (negative, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let (int, frac) = digits.split_once('.').unwrap_or((digits, ""));
    let all_digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if int.len() + frac.len() == 0 || !all_digits(int) || !all_digits(frac) {
        return None;
    }
    let significant = format!("{int}{frac}");
    let significant = significant.trim_start_matches('0');
    let scale = u8::try_from(frac.len()).ok()?;
    // One integer digit more than the scale, so `-0.05` is DECIMAL(3, 2):
    // DuckDB prints DECIMAL(2, 2) as `-.05`.
    let width = u8::try_from(significant.len())
        .ok()?
        .max(scale.saturating_add(1));
    if significant.len() > 38 || scale > 38 {
        return None;
    }
    let width = width.min(38);
    let magnitude: i128 = if significant.is_empty() {
        0
    } else {
        significant.parse().ok()?
    };
    let value = if negative { -magnitude } else { magnitude };
    DuckDecimal::new(width, scale, value)
        .ok()
        .map(DuckValue::Decimal)
}

/// A DuckDB connection and its interrupt handle.
struct Session {
    connection: Arc<Mutex<Connection>>,
    interrupt: Arc<InterruptHandle>,
}

impl Session {
    fn new(conn: Connection) -> Self {
        Self {
            interrupt: conn.interrupt_handle(),
            connection: Arc::new(Mutex::new(conn)),
        }
    }
}

/// One DuckDB connection for the user's calls. Every call runs on a
/// blocking thread and holds its connection for its whole duration (see
/// [`crate::blocking`]), so calls take turns and a transaction is never
/// interleaved with another call.
///
/// `query_read_only` runs each call on a connection of its own, cloned from
/// `read_only_source` (which runs nothing itself) and dropped after the call.
/// Nothing a read-only query does to its session reaches the next one
/// (`enable_profiling()` on a shared clone kept writing a file after every
/// later call), read-only calls don't queue behind each other or behind the
/// user's transaction or long query, and a cancel interrupts only that
/// call's statement.
pub struct DuckdbDriver {
    main: Session,
    read_only_source: Arc<Mutex<Connection>>,
}

/// Opens the database. Blocking: DuckDB may read or create a file.
fn open(config: &ConnectConfig) -> Result<Connection, DbError> {
    let path = config.path.as_deref().unwrap_or(":memory:");

    if path == ":memory:" || path.is_empty() {
        return Connection::open_in_memory().map_err(DbError::connection_error);
    }
    // DuckDB creates missing files on open; only allow that when asked
    // so a mistyped path fails instead of opening a new, empty database.
    let file = std::path::Path::new(path);
    if !file.exists() {
        if !config.create_if_missing.unwrap_or(false) {
            return Err(DbError {
                message: format!("Database file not found: {}", path),
                code: "FILE_NOT_FOUND".to_string(),
            });
        }
        if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|e| {
                DbError::connection_error(format!("Failed to create database directory: {}", e))
            })?;
        }
    }
    Connection::open(path).map_err(DbError::connection_error)
}

/// Opens the database and sets up the session. `arrow_lossless_conversion`
/// makes DuckDB send TIMETZ with its offset, and HUGEINT, UHUGEINT and UUID
/// as their own bytes rather than as DECIMAL(38, 0) and text (see
/// [`crate::decode`]). It only changes how results reach this driver.
///
/// Also opens the connection the read-only path clones from, a `try_clone()`
/// of the first: the same database instance, its own session.
/// `arrow_lossless_conversion` is a global setting (checked: a clone reads
/// `true` after a plain `SET` on the first connection, and `false` after
/// `SET GLOBAL … = false`), so HUGEINT, UUID and TIMETZ decode the same on
/// every clone.
fn open_sessions(config: &ConnectConfig) -> Result<(Connection, Connection), DbError> {
    let conn = open(config)?;
    conn.execute_batch("SET arrow_lossless_conversion = true")
        .map_err(DbError::connection_error)?;
    let read_only = conn.try_clone().map_err(DbError::connection_error)?;
    Ok((conn, read_only))
}

impl DuckdbDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let config = config.clone();
        let (conn, read_only) = tokio::task::spawn_blocking(move || open_sessions(&config))
            .await
            .map_err(|e| Op::Connect.join_error(e))??;
        Ok(Self {
            main: Session::new(conn),
            read_only_source: Arc::new(Mutex::new(read_only)),
        })
    }

    /// Runs `f` with the main connection on a blocking thread. Dropping the
    /// returned future interrupts `f` (see [`crate::blocking`]).
    async fn run<T, F>(&self, op: Op, f: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection, &Worker) -> Result<T, DbError> + Send + 'static,
    {
        run_on(&self.main, op, f).await
    }
}

/// Runs `f` with `session`'s connection on a blocking thread. Dropping the
/// returned future interrupts `f` through that session's interrupt handle.
async fn run_on<T, F>(session: &Session, op: Op, f: F) -> Result<T, DbError>
where
    T: Send + 'static,
    F: FnOnce(&Connection, &Worker) -> Result<T, DbError> + Send + 'static,
{
    let (call, worker) = blocking::call(session.interrupt.clone());
    let conn = session.connection.clone();
    let result = tokio::task::spawn_blocking(move || worker.run(&conn, op, f)).await;
    drop(call);
    result.map_err(|e| op.join_error(e))?
}

/// An executed statement's rows, read chunk by chunk straight from DuckDB's
/// Arrow output and decoded by [`decode::decode`].
struct ResultReader<'s, 'c> {
    stmt: &'s Statement<'c>,
    columns: Vec<String>,
    kinds: Vec<Kind>,
    chunk: Option<StructArray>,
    /// The next row of `chunk`.
    next: usize,
}

impl<'s, 'c> ResultReader<'s, 'c> {
    fn new(stmt: &'s Statement<'c>) -> Self {
        let count = stmt.column_count();
        let columns = (0..count)
            .map(|i| {
                stmt.column_name(i)
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            })
            .collect();
        // duckdb-rs panics on logical types it doesn't know (reading or
        // walking them); the cells then decode by their Arrow type alone.
        let kinds = (0..count)
            .map(|i| {
                catch_unwind(AssertUnwindSafe(|| Kind::of(&stmt.column_logical_type(i))))
                    .unwrap_or(Kind::Plain)
            })
            .collect();
        Self {
            stmt,
            columns,
            kinds,
            chunk: None,
            next: 0,
        }
    }

    /// The next row, `None` after the last.
    fn next_row(&mut self) -> Result<Option<Vec<Value>>, DbError> {
        let chunk = loop {
            match &self.chunk {
                Some(chunk) if self.next < chunk.len() => break chunk,
                _ => match self.stmt.step().map_err(DbError::query_error)? {
                    Some(chunk) => {
                        self.chunk = Some(chunk);
                        self.next = 0;
                    }
                    None => {
                        self.chunk = None;
                        return Ok(None);
                    }
                },
            }
        };
        let row = self.next;
        self.next += 1;
        (0..self.kinds.len())
            .map(|i| self.read_cell(chunk, row, i))
            .collect::<Result<_, _>>()
            .map(Some)
    }

    /// Cell `i` of `row`. The decoder reads Arrow arrays itself, so it
    /// doesn't hit duckdb-rs's panics (`unreachable!` on Arrow types it
    /// doesn't map, such as `TIME_NS`'s `Time64(ns)`). An Arrow type the
    /// decoder doesn't know is an error; `catch_unwind` stays as the safety
    /// net for anything else (a malformed array). Either becomes
    /// `UNSUPPORTED_TYPE` naming the column. Reading has no side effects, so
    /// the statement and the connection are fine afterwards. A caught panic
    /// still prints through the process's panic hook.
    fn read_cell(&self, chunk: &StructArray, row: usize, i: usize) -> Result<Value, DbError> {
        let column = chunk.column(i);
        let problem = match catch_unwind(AssertUnwindSafe(|| {
            decode::decode(&self.kinds[i], column.as_ref(), row)
        })) {
            Ok(Ok(v)) => return Ok(v),
            Ok(Err(problem)) => problem,
            Err(payload) => blocking::panic_message(&*payload),
        };
        Err(DbError {
            message: format!(
                "Column \"{}\" has a type Seaquel can't read yet (Arrow {}): {problem}. \
                 Cast it in the query, e.g. to VARCHAR.",
                self.columns[i],
                column.data_type()
            ),
            code: "UNSUPPORTED_TYPE".to_string(),
        })
    }
}

/// Prepares `sql`, then fails if the call was cancelled meanwhile. DuckDB
/// clears its interrupt flag when a statement starts executing (see
/// [`crate::blocking`]), so an interrupt that lands between prepare and
/// execute would be lost; this check catches it.
///
/// For multi-statement SQL, duckdb-rs's `prepare` runs every statement but
/// the last itself; a cancel that lands between those isn't seen until the
/// last one.
fn prepare<'c>(
    conn: &'c Connection,
    worker: &Worker,
    op: Op,
    sql: &str,
) -> Result<Statement<'c>, DbError> {
    let stmt = conn.prepare(sql).map_err(|e| op.error(e))?;
    if worker.is_cancelled() {
        return Err(op.error("cancelled"));
    }
    Ok(stmt)
}

/// `Driver::query`, on the blocking thread.
fn query_blocking(
    conn: &Connection,
    worker: &Worker,
    sql: &str,
    bound: &[DuckValue],
) -> Result<QueryResult, DbError> {
    let mut stmt = prepare(conn, worker, Op::Query, sql)?;
    stmt.execute(params_from_iter(bound.iter()))
        .map_err(DbError::query_error)?;
    let mut reader = ResultReader::new(&stmt);

    let cap = seaquel_engine::max_query_rows();
    let mut rows: Vec<Vec<Value>> = Vec::new();
    while let Some(row) = reader.next_row()? {
        if worker.is_cancelled() {
            return Err(DbError::query_error("cancelled"));
        }
        if rows.len() >= cap {
            return Err(DbError::result_too_large(cap));
        }
        rows.push(row);
    }

    Ok(QueryResult {
        columns: reader.columns,
        rows,
    })
}

/// Rows per streamed batch, as in the sqlx drivers.
///
/// DuckDB still materializes the whole result before the first batch
/// (`Statement::execute` doesn't use DuckDB's streaming execution; the
/// driver then reads the result's Arrow chunks with `step()`), so batching
/// bounds what's in flight to the UI, not DuckDB's memory use.
const BATCH_SIZE: usize = 5000;

/// Batches in flight between the blocking producer and the stream. See
/// [`BATCH_SIZE`]: the result is already materialized in DuckDB by then.
const STREAM_BUFFER: usize = 2;

/// `Driver::query_stream`, on the blocking thread: sends batches until the
/// result ends, the stream is dropped or `cancel` fires. Errors are sent too.
fn stream_blocking(
    conn: &Connection,
    worker: &Worker,
    sql: &str,
    bound: &[DuckValue],
    cancel: &CancellationToken,
    tx: &mpsc::Sender<Result<StreamBatch, DbError>>,
) -> Result<(), DbError> {
    let stopped = || worker.is_cancelled() || cancel.is_cancelled() || tx.is_closed();
    if stopped() {
        return Ok(());
    }
    let mut stmt = match prepare(conn, worker, Op::Query, sql) {
        Err(_) if stopped() => return Ok(()),
        stmt => stmt?,
    };
    stmt.execute(params_from_iter(bound.iter()))
        .map_err(DbError::query_error)?;
    let mut reader = ResultReader::new(&stmt);
    let mut columns = Some(reader.columns.clone());

    let mut buffer: Vec<Vec<Value>> = Vec::with_capacity(BATCH_SIZE);
    while let Some(row) = reader.next_row()? {
        if stopped() {
            return Ok(());
        }
        buffer.push(row);
        if buffer.len() >= BATCH_SIZE {
            let batch = StreamBatch {
                columns: columns.take(),
                rows: std::mem::replace(&mut buffer, Vec::with_capacity(BATCH_SIZE)),
                is_final: false,
            };
            if tx.blocking_send(Ok(batch)).is_err() {
                return Ok(());
            }
        }
    }
    // Terminal batch: carries the columns if no batch did (empty result).
    let _ = tx.blocking_send(Ok(StreamBatch {
        columns,
        rows: buffer,
        is_final: true,
    }));
    Ok(())
}

/// `Driver::transaction`, on the blocking thread.
fn transaction_blocking(
    conn: &Connection,
    worker: &Worker,
    statements: &[(String, Vec<DuckValue>, Option<ExpectRows>)],
) -> Result<(), DbError> {
    let no_params = || params_from_iter(std::iter::empty::<DuckValue>());

    // Fails (and leaves it alone) when a transaction opened by hand is
    // already running.
    conn.execute("BEGIN", no_params())
        .map_err(DbError::execute_error)?;

    let body = catch_unwind(AssertUnwindSafe(|| -> Result<(), DbError> {
        for (index, (sql, bound, expect)) in statements.iter().enumerate() {
            if worker.is_cancelled() {
                return Err(DbError::execute_error("cancelled"));
            }
            let affected = prepare(conn, worker, Op::Execute, sql)?
                .execute(params_from_iter(bound.iter()))
                .map_err(DbError::execute_error)?;
            if let Some(expect) = expect {
                expect.check(index, affected as u64)?;
            }
        }
        // Nobody would learn the outcome: roll back instead.
        if worker.is_cancelled() {
            return Err(DbError::execute_error("cancelled"));
        }
        conn.execute("COMMIT", no_params())
            .map_err(DbError::execute_error)?;
        Ok(())
    }));
    let outcome = match body {
        Ok(outcome) => outcome,
        Err(payload) => Err(DbError::execute_error(format!(
            "DuckDB panicked: {}",
            blocking::panic_message(&*payload)
        ))),
    };
    if outcome.is_err() {
        // Best-effort rollback; surface the original error regardless of
        // whether the rollback itself succeeds (it fails harmlessly when a
        // failed COMMIT already ended the transaction). duckdb-rs's
        // `is_autocommit` is a stub that always says true, so it can't tell.
        let _ = conn.execute("ROLLBACK", no_params());
    }
    outcome
}

/// The only statement the read-only path prepares (see [`read_only_wrapper`]).
/// The user's SQL is its parameter, so duckdb-rs's `prepare` (which runs
/// every statement but the last itself) never sees it: `query()` parses it
/// with DuckDB's own parser and runs it only if it is exactly one SELECT
/// (`WITH`, `FROM t`, `VALUES`, `SUMMARIZE`, `DESCRIBE` and `SHOW` count),
/// refusing `COPY`, `SET`, `ATTACH`, `INSTALL`, DDL, DML and every
/// multi-statement input. (Core's token check runs first and admits only
/// statements starting with SELECT or WITH; it also refuses the SELECTs a
/// connection can't contain, such as `enable_logging()`.)
const READ_ONLY_WRAPPER: &str = "SELECT * FROM query(?)";

/// [`READ_ONLY_WRAPPER`] with `LIMIT` one past the row cap, so DuckDB stops
/// there instead of materializing a huge result before the driver's cap
/// (`RESULT_TOO_LARGE`) sees it. The limit is our own number, never user
/// input.
fn read_only_wrapper() -> String {
    let limit = seaquel_engine::max_query_rows().saturating_add(1);
    format!("{READ_ONLY_WRAPPER} LIMIT {limit}")
}

/// DuckDB's refusals on the read-only path: `query()`'s, and the read-only
/// transaction's (`nextval`, a write `query()` let through).
const READ_ONLY_REFUSALS: &[&str] = &[
    "Expected a single SELECT statement",
    "transaction is launched in read-only mode",
];

/// `Driver::query_read_only`, on the blocking thread, on the call's own
/// connection: `BEGIN TRANSACTION READ ONLY`, the user's SQL through
/// [`read_only_wrapper`], `ROLLBACK`. The rollback runs on every path once
/// `BEGIN` was sent, a panic or a cancel included; the connection is dropped
/// after the call anyway, which ends any transaction a failed ROLLBACK left.
fn read_only_blocking(
    conn: &Connection,
    worker: &Worker,
    sql: &str,
) -> Result<QueryResult, DbError> {
    let no_params = || params_from_iter(std::iter::empty::<DuckValue>());
    if let Err(e) = conn.execute("BEGIN TRANSACTION READ ONLY", no_params()) {
        // An interrupt can land in BEGIN. Whether or not it opened the
        // transaction, make sure none is left behind.
        let _ = conn.execute("ROLLBACK", no_params());
        return Err(DbError::query_error(e));
    }
    let body = catch_unwind(AssertUnwindSafe(|| {
        query_blocking(
            conn,
            worker,
            &read_only_wrapper(),
            &[DuckValue::Text(sql.to_string())],
        )
    }));
    rollback_read_only(conn);
    let outcome = body.unwrap_or_else(|payload| {
        Err(DbError::query_error(format!(
            "DuckDB panicked: {}",
            blocking::panic_message(&*payload)
        )))
    });
    outcome.map_err(|mut e| {
        // DuckDB points at the wrapper ("LINE 1: SELECT * FROM query(?)"
        // and a caret), which isn't the SQL the user or the model wrote.
        if let Some(at) = e.message.find(&format!("\n\nLINE 1: {READ_ONLY_WRAPPER}")) {
            e.message.truncate(at);
        }
        if READ_ONLY_REFUSALS.iter().any(|r| e.message.contains(r)) {
            DbError::read_only(e.message)
        } else {
            e
        }
    })
}

/// Ends the read-only transaction. A failure (a cancel's interrupt can land
/// on it) is only logged: the call's connection is dropped right after,
/// which rolls the transaction back, and no other call uses it.
fn rollback_read_only(conn: &Connection) {
    let no_params = || params_from_iter(std::iter::empty::<DuckValue>());
    if let Err(e) = conn.execute("ROLLBACK", no_params()) {
        warn!(activity = "db.query_read_only", driver = "duckdb"; "ROLLBACK of a read-only query failed: {e}");
    }
}

fn bind_all(params: &[Value]) -> Result<Vec<DuckValue>, DbError> {
    params.iter().map(to_duckdb_param).collect()
}

#[seaquel_runtime::async_trait]
impl Driver for DuckdbDriver {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        let sql = sql.to_string();
        let bound = bind_all(&params)?;
        self.run(Op::Query, move |conn, worker| {
            query_blocking(conn, worker, &sql, &bound)
        })
        .await
    }

    /// Streams batches from a blocking producer. DuckDB materializes the
    /// result before the first row, so a slow query sends nothing until it's
    /// done. Dropping the stream (Core's cancel) interrupts the query.
    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        Box::pin(async_stream::stream! {
            let bound = match bind_all(&params) {
                Ok(bound) => bound,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };
            let (tx, mut rx) = mpsc::channel(STREAM_BUFFER);
            let (call, worker) = blocking::call(self.main.interrupt.clone());
            let conn = self.main.connection.clone();
            let task = tokio::task::spawn_blocking(move || {
                let err_tx = tx.clone();
                let outcome = worker.run(&conn, Op::Query, |conn, worker| {
                    stream_blocking(conn, worker, &sql, &bound, &cancel, &tx)
                });
                if let Err(e) = outcome {
                    let _ = err_tx.blocking_send(Err(e));
                }
            });
            // Dropped with this stream; interrupts the query if it's running.
            let _call = call;
            while let Some(item) = rx.recv().await {
                let last = item.as_ref().map_or(true, |b| b.is_final);
                yield item;
                if last {
                    return;
                }
            }
            // The producer ended without a final batch or an error: it was
            // cancelled (nothing is listening then) or it panicked.
            if let Err(e) = task.await {
                yield Err(Op::Query.join_error(e));
            }
        })
    }

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        let sql = sql.to_string();
        let bound = bind_all(&params)?;
        let rows_affected = self
            .run(Op::Execute, move |conn, worker| {
                prepare(conn, worker, Op::Execute, &sql)?
                    .execute(params_from_iter(bound.iter()))
                    .map_err(DbError::execute_error)
            })
            .await?;
        Ok(ExecuteResult {
            rows_affected: rows_affected as u64,
            last_insert_id: None,
        })
    }

    /// Every statement or none. The connection is held from BEGIN to
    /// COMMIT, so no other call can interleave. The first failure (or a
    /// panic) rolls back and is returned; bind errors fail before anything
    /// runs, and a statement that affected fewer rows than its
    /// `expect_rows` rolls back with `NO_ROWS_AFFECTED`. Dropping the future
    /// interrupts the running statement, which fails and rolls back. BEGIN
    /// fails while a transaction opened by hand is running, which is then
    /// left alone.
    async fn transaction(&self, statements: Vec<BatchStatement>) -> Result<(), DbError> {
        let statements = statements
            .into_iter()
            .map(|s| Ok((s.sql, bind_all(&s.params)?, s.expect_rows)))
            .collect::<Result<Vec<_>, DbError>>()?;
        self.run(Op::Execute, move |conn, worker| {
            transaction_blocking(conn, worker, &statements)
        })
        .await
    }

    /// See [`read_only_blocking`]. DuckDB's read-only path takes no bind
    /// values: its one parameter is the user's SQL. Nothing that calls it
    /// passes any.
    async fn query_read_only(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        if !params.is_empty() {
            return Err(DbError::read_only(
                "read-only DuckDB queries take no bind values",
            ));
        }
        let source = self.read_only_source.clone();
        let conn = tokio::task::spawn_blocking(move || blocking::lock(&source).try_clone())
            .await
            .map_err(|e| Op::Query.join_error(e))?
            .map_err(DbError::query_error)?;
        // Dropped after the call: by this future, and by the blocking task
        // when it finishes (after a cancel, once the interrupt has landed).
        let session = Session::new(conn);
        let sql = sql.to_string();
        run_on(&session, Op::Query, move |conn, worker| {
            read_only_blocking(conn, worker, &sql)
        })
        .await
    }

    async fn close(&self) -> Result<(), DbError> {
        // DuckDB Connection is closed on drop
        Ok(())
    }

    // ── Introspection (see `crate::introspect`) ──

    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        let r = self.query(&introspect::schemas_sql(), vec![]).await?;
        Ok(introspect::parse_schemas(&r))
    }

    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        let r = self.query(&introspect::schema_sql(), vec![]).await?;
        Ok(introspect::parse_schema(&r))
    }

    /// `schema` is the schema as `schema_tables` lists it (`fx_aux.main`
    /// for an attached catalog). Bug fix 7: a failing foreign-key or UNIQUE
    /// query is logged and the columns come back without foreign keys or
    /// UNIQUE flags.
    async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        let params = || vec![Value::from(schema), Value::from(table)];
        let columns = self.query(&introspect::columns_sql(), params()).await?;
        let optional = |what: &'static str, r: Result<QueryResult, DbError>| match r {
            Ok(r) => Some(r),
            Err(e) => {
                warn!(activity = "db.table_metadata", driver = "duckdb", schema = schema, table = table; "The {what} of {schema}.{table} failed to load: {}", e.message);
                None
            }
        };
        let foreign_keys = optional(
            "foreign keys",
            self.query(&introspect::foreign_keys_sql(), params()).await,
        );
        let unique = optional(
            "UNIQUE constraints",
            self.query(&introspect::unique_columns_sql(), params())
                .await,
        );
        let indexes = self.query(&introspect::indexes_sql(), params()).await?;
        let mut columns = introspect::parse_columns(&columns, foreign_keys.as_ref());
        if let Some(unique) = &unique {
            introspect::apply_unique_columns(&mut columns, unique);
        }
        Ok((columns, introspect::parse_indexes(&indexes)))
    }

    /// Table sizes (row counts only: DuckDB keeps no per-table sizes), the
    /// indexes and the overview, over the same catalogs as the tree. Each
    /// table is counted in its own catalog; a count that fails leaves 0, as
    /// TsEngineClient did.
    async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
        let overview = self.query(introspect::OVERVIEW_SQL, vec![]).await?;
        let sizes = self.query(&introspect::table_sizes_sql(), vec![]).await?;
        let usage = self.query(&introspect::index_usage_sql(), vec![]).await?;
        let mut table_sizes = introspect::parse_table_sizes(&sizes);
        for (table, sql) in table_sizes
            .iter_mut()
            .zip(introspect::row_count_targets(&sizes))
        {
            if let Ok(r) = self.query(&sql, vec![]).await {
                table.row_count = introspect::parse_row_count(&r);
            }
        }
        Ok(DatabaseStatistics {
            overview: introspect::parse_overview(&overview),
            table_sizes,
            index_usage: introspect::parse_index_usage(&usage),
        })
    }

    /// `EXPLAIN (FORMAT JSON)`, with `ANALYZE` when asked (which **runs**
    /// the statement, writes included, as Postgres's does). The parameters
    /// are bound to the EXPLAIN.
    async fn explain(
        &self,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        let r = self
            .query(&introspect::explain_sql(sql, analyze), params)
            .await?;
        Ok(introspect::parse_explain(&r, analyze))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(path: &std::path::Path, create_if_missing: bool) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": "duckdb",
            "path": path.to_str().unwrap(),
            "create_if_missing": create_if_missing,
        }))
        .unwrap()
    }

    fn temp_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("seaquel-duckdb-test-{}", std::process::id()))
    }

    /// A cell the decoder rejects fails its row with `UNSUPPORTED_TYPE`
    /// naming the column, after the rows before it read fine. No SQL value
    /// fails to decode today, so the column's kind is forced: a BIGNUM needs
    /// at least 4 bytes.
    #[test]
    fn undecodable_cell_is_unsupported_type() {
        let conn = Connection::open_in_memory().unwrap();
        let mut stmt = conn
            .prepare("SELECT CASE WHEN i < 3000 THEN NULL ELSE '\\x01'::BLOB END AS t FROM range(3001) r(i)")
            .unwrap();
        stmt.execute([]).unwrap();
        let mut reader = ResultReader::new(&stmt);
        reader.kinds[0] = Kind::Bignum;
        for _ in 0..3000 {
            assert_eq!(reader.next_row().unwrap(), Some(vec![Value::Null]));
        }
        let e = reader.next_row().unwrap_err();
        assert_eq!(e.code, "UNSUPPORTED_TYPE");
        assert!(e.message.contains("Column \"t\""), "{}", e.message);
        assert!(e.message.contains("BIGNUM of 1 bytes"), "{}", e.message);
    }

    #[test]
    fn decimal_params() {
        let d = |s: &str| match decimal_param(s) {
            Some(DuckValue::Decimal(d)) => Some((d.width(), d.scale(), d.to_string())),
            _ => None,
        };
        assert_eq!(d("1.50"), Some((3, 2, "1.50".into())));
        assert_eq!(d("-0.05"), Some((3, 2, "-0.05".into())));
        assert_eq!(d("007"), Some((1, 0, "7".into())));
        assert_eq!(d("0"), Some((1, 0, "0".into())));
        assert_eq!(d(".5"), Some((2, 1, "0.5".into())));
        assert_eq!(d(&"9".repeat(38)).map(|x| x.0), Some(38));
        assert_eq!(
            d(&format!("0.{}", "1".repeat(38))).map(|x| (x.0, x.1)),
            Some((38, 38))
        );
        for s in [&"9".repeat(39), "NaN", "1e5", "", "-", ".", "1.2.3", "１"] {
            assert_eq!(d(s), None, "{s}");
        }
    }

    #[tokio::test]
    async fn missing_file_is_not_created() {
        let path = temp_path().join("missing.duckdb");
        let err = DuckdbDriver::connect(&config(&path, false))
            .await
            .err()
            .unwrap();
        assert_eq!(err.code, "FILE_NOT_FOUND");
        assert!(!path.exists(), "database file must not be created");
    }

    #[tokio::test]
    async fn create_if_missing_creates_file_and_directory() {
        let dir = temp_path().join("create");
        let path = dir.join("nested").join("new.duckdb");
        DuckdbDriver::connect(&config(&path, true)).await.unwrap();
        assert!(path.exists(), "database file should be created");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn in_memory_needs_no_file() {
        let cfg: ConnectConfig =
            serde_json::from_value(serde_json::json!({ "driver": "duckdb", "path": ":memory:" }))
                .unwrap();
        assert!(DuckdbDriver::connect(&cfg).await.is_ok());
    }

    /// DuckDB clears its interrupt flag when a statement is prepared or
    /// starts executing, so an interrupt that lands before that is lost;
    /// the cancelled check after prepare must catch it. The drop happens
    /// inside the call, deterministically, while it holds the connection.
    /// Without the check the query runs to the end (seconds; the per-row
    /// check then still reports "cancelled"), so the test times it.
    #[test]
    fn cancel_that_lands_before_execute_is_not_lost() {
        let conn = Connection::open_in_memory().unwrap();
        let interrupt = conn.interrupt_handle();
        let conn = Mutex::new(conn);

        let (call, worker) = blocking::call(interrupt.clone());
        let started = tokio::time::Instant::now();
        let r = worker.run(&conn, Op::Query, move |conn, worker| {
            drop(call); // flags the call and interrupts DuckDB
            query_blocking(
                conn,
                worker,
                "SELECT sum(i % 7) FROM range(1500000000) t(i)",
                &[],
            )
        });
        let took = started.elapsed();
        let e = r.unwrap_err();
        assert_eq!(e.code, "QUERY_ERROR");
        assert!(e.message.contains("cancelled"), "{}", e.message);
        assert!(
            took < std::time::Duration::from_millis(500),
            "the query ran: {took:?}"
        );

        // The connection is fine.
        let (_call, worker) = blocking::call(interrupt);
        let r = worker
            .run(&conn, Op::Query, |conn, worker| {
                query_blocking(conn, worker, "SELECT 42", &[])
            })
            .unwrap();
        assert_eq!(r.rows, vec![vec![Value::Int(42)]]);
    }
}
