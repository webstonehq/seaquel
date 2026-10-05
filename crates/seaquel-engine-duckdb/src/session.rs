//! DuckDB's side of a call, minus what happens to the rows: opening the
//! database, binds, prepare, running a statement and reading its Arrow
//! chunks, transactions and the read-only scaffolding. Every function runs
//! on the thread that holds the connection (see [`crate::blocking`]).
//!
//! A result's chunks go to a [`ChunkSink`]: the DuckDB helper's sink writes
//! them as Arrow IPC for its client (`helper.rs`), with the column kinds
//! [`crate::kinds::of`] reads. (The in-process native driver, whose sinks
//! decoded them into `Value`s, was deleted once every interface used the
//! helper; this code is what it ran.)
//!
//! Chunks are always read with `Statement::step()`, never duckdb-rs's Arrow
//! iterator, which panics when a fetch fails (an interrupt included).

use std::panic::{catch_unwind, AssertUnwindSafe};

use duckdb::arrow::array::StructArray;
use duckdb::{
    params_from_iter, types::Decimal as DuckDecimal, types::Value as DuckValue, Config, Connection,
    Statement,
};
use log::warn;
use seaquel_engine::{
    ConnectConfig, DbError, ExpectRows, TransactionError, Value, TRANSACTION_ALREADY_OPEN,
    TRANSACTION_OPEN,
};

use crate::blocking::{self, Op, Worker};

/// What a sink wants after a chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    /// The next chunk, please.
    Continue,
    /// Stop reading: the sink has what it needs (a row cap) or nobody is
    /// listening. The statement is dropped and `finish` isn't called.
    Stop,
}

/// Where a result's Arrow chunks go.
pub(crate) trait ChunkSink {
    /// The executed statement, before its first chunk: its column names and
    /// types. Called once per result, also for an empty one.
    fn columns(&mut self, stmt: &Statement<'_>) -> Result<(), DbError>;

    /// One chunk of rows, in order.
    fn chunk(&mut self, chunk: StructArray) -> Result<Flow, DbError>;

    /// The result ended. Not called after [`Flow::Stop`] or an error.
    fn finish(&mut self) -> Result<(), DbError>;
}

/// How a statement runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Execution {
    /// `Statement::execute`: DuckDB materializes the whole result before
    /// the first chunk, so a failing query fails before any rows.
    Materialized,
    /// DuckDB's streaming execution: chunks are produced as they are
    /// fetched, DuckDB running ahead of the reader until
    /// `streaming_buffer_size` bytes are buffered (a session setting, 10^6
    /// bytes by default; it counts bytes, not rows), then waiting. The first
    /// rows come early, DuckDB holds only what is in flight, a query can fail
    /// after its first chunks, and stopping early stops the query.
    /// `query_stream` only.
    ///
    /// Stopping early or pausing (`tests/streaming.rs` pins the first): a write
    /// with `RETURNING` finishes before its first chunk (DuckDB's insert is a
    /// sink) and stays when the stream is dropped, in autocommit and in a
    /// transaction opened by hand, while side effects in a SELECT's
    /// expressions (`nextval`) happen only for the rows DuckDB produced
    /// before the drop. A stream that isn't read holds its statement's
    /// transaction open, and the connection's turn, until it is read to the
    /// end or dropped.
    Streaming,
}

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

/// Every parameter bound for DuckDB, or the first that can't be.
pub(crate) fn bind_all(params: &[Value]) -> Result<Vec<DuckValue>, DbError> {
    params.iter().map(to_duckdb_param).collect()
}

/// The instance settings for [`ConnectConfig::restricted`], in the open
/// config so they hold before the first statement. They go in after
/// [`ConnectConfig::duckdb_config`]'s options, so they win over them:
///
/// - `enable_external_access = false`: no file but the database's own (and
///   its WAL and temp directory), no URLs, no `ATTACH`, no `COPY`, no
///   `INSTALL`/`LOAD`. DuckDB refuses to turn it back on while the database
///   is running.
/// - `autoinstall_known_extensions` and `autoload_known_extensions = false`:
///   a query that needs an extension fails instead of downloading it into
///   `~/.duckdb` or loading one that's there.
/// - `lock_configuration = true`: changing a global option fails, so a query
///   can't turn the autoload settings back on (DuckDB already refuses to
///   re-enable external access). Session settings (`search_path`,
///   `enable_profiling`) still change; with external access off, the ones
///   naming a file (`profiling_output`, `log_query_path`) don't write it
///   (`tests/restricted.rs`). `arrow_lossless_conversion` goes in here
///   too, since [`open_sessions`] couldn't `SET` it afterwards.
///
/// DuckDB still opens and writes the database file, its WAL and its temp
/// directory. `duckdb_extensions()` fails, since it lists the extension
/// directory.
fn restricted_config(config: &ConnectConfig) -> Result<Config, DbError> {
    user_config(config)?
        .enable_external_access(false)
        .and_then(|c| c.enable_autoload_extension(false))
        .and_then(|c| c.with("arrow_lossless_conversion", "true"))
        .and_then(|c| c.with("lock_configuration", "true"))
        .map_err(DbError::connection_error)
}

/// The [`ConnectConfig::duckdb_config`] options a restricted instance takes.
/// None of them reaches a file, a URL or an extension: opening read-only,
/// and the thread and memory limits.
const RESTRICTED_OPTIONS: &[&str] = &[
    "access_mode",
    "threads",
    "worker_threads",
    "memory_limit",
    "max_memory",
];

/// The open config with [`ConnectConfig::duckdb_config`]'s options, in
/// order. An option DuckDB doesn't know (or a bad value) fails the connect
/// with DuckDB's own message.
///
/// A `restricted` instance takes only [`RESTRICTED_OPTIONS`]; any other key
/// is `INVALID_CONNECTION`, since options like `allowed_directories`,
/// `allowed_paths`, `temp_directory`, `secret_directory` or the extension
/// settings reopen what the lock-down closes.
fn user_config(config: &ConnectConfig) -> Result<Config, DbError> {
    let restricted = config.restricted.unwrap_or(false);
    let mut out = Config::default();
    for (key, value) in config.duckdb_config.iter().flatten() {
        if restricted
            && !RESTRICTED_OPTIONS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(key))
        {
            return Err(DbError {
                message: format!(
                    "The DuckDB option \"{key}\" isn't allowed on a restricted connection"
                ),
                code: "INVALID_CONNECTION".to_string(),
            });
        }
        out = out
            .with(key, value)
            .map_err(|e| DbError::connection_error(format!("DuckDB option \"{key}\": {e}")))?;
    }
    Ok(out)
}

/// Opens the database. Blocking: DuckDB may read or create a file.
fn open(config: &ConnectConfig) -> Result<Connection, DbError> {
    let path = config.path.as_deref().unwrap_or(":memory:");
    let restricted = config.restricted.unwrap_or(false);
    let flags = || {
        if restricted {
            restricted_config(config)
        } else {
            user_config(config)
        }
    };

    if path == ":memory:" || path.is_empty() {
        return Connection::open_in_memory_with_flags(flags()?).map_err(DbError::connection_error);
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
    Connection::open_with_flags(path, flags()?).map_err(DbError::connection_error)
}

/// Opens the database and sets up the session. `arrow_lossless_conversion`
/// makes DuckDB send TIMETZ with its offset, and HUGEINT, UHUGEINT and UUID
/// as their own bytes rather than as DECIMAL(38, 0) and text (see
/// [`crate::decode`]). It only changes how results reach the driver.
///
/// Also opens the connection the read-only path clones from, a `try_clone()`
/// of the first: the same database instance, its own session.
/// `arrow_lossless_conversion` is a global setting (checked: a clone reads
/// `true` after a plain `SET` on the first connection, and `false` after
/// `SET GLOBAL … = false`), so HUGEINT, UUID and TIMETZ decode the same on
/// every clone.
pub(crate) fn open_sessions(config: &ConnectConfig) -> Result<(Connection, Connection), DbError> {
    let conn = open(config)?;
    // A restricted instance has it from its open config, and its
    // configuration is locked.
    if !config.restricted.unwrap_or(false) {
        conn.execute_batch("SET arrow_lossless_conversion = true")
            .map_err(DbError::connection_error)?;
    }
    let read_only = conn.try_clone().map_err(DbError::connection_error)?;
    Ok((conn, read_only))
}

/// Prepares `sql`, then fails if the call was cancelled meanwhile. DuckDB
/// clears its interrupt flag when a statement starts executing (see
/// [`crate::blocking`]), so an interrupt that lands between prepare and
/// execute would be lost; this check catches it. Both executions clear the
/// flag, the streaming one included.
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

/// Runs `sql` and hands its result to `sink`: the columns, then each chunk
/// as DuckDB produces it (with `Statement::step()`), then `finish` unless
/// the sink stopped it. A cancelled call stops between chunks with a
/// `cancelled` error.
pub(crate) fn rows(
    conn: &Connection,
    worker: &Worker,
    sql: &str,
    bound: &[DuckValue],
    execution: Execution,
    sink: &mut dyn ChunkSink,
) -> Result<(), DbError> {
    let mut stmt = prepare(conn, worker, Op::Query, sql)?;
    let params = params_from_iter(bound.iter());
    match execution {
        Execution::Materialized => {
            stmt.execute(params).map_err(DbError::query_error)?;
        }
        // Binds and starts the streaming execution. The iterator it returns
        // is dropped unread: it panics on a failed fetch, and `step()` below
        // reads the same result and returns the error instead.
        Execution::Streaming => {
            drop(stmt.stream_arrow(params).map_err(DbError::query_error)?);
        }
    }
    sink.columns(&stmt)?;
    while let Some(chunk) = stmt.step().map_err(DbError::query_error)? {
        if worker.is_cancelled() {
            return Err(Op::Query.error("cancelled"));
        }
        if sink.chunk(chunk)? == Flow::Stop {
            return Ok(());
        }
    }
    sink.finish()
}

/// `Driver::execute`: runs `sql`, returning the rows it affected.
pub(crate) fn execute(
    conn: &Connection,
    worker: &Worker,
    sql: &str,
    bound: &[DuckValue],
) -> Result<u64, DbError> {
    let affected = prepare(conn, worker, Op::Execute, sql)?
        .execute(params_from_iter(bound.iter()))
        .map_err(DbError::execute_error)?;
    Ok(affected as u64)
}

/// Whether a transaction opened by hand is running on `conn`. In autocommit
/// every statement runs in a transaction of its own, so two
/// `txid_current()` calls return different ids; inside one transaction they
/// return the same. A probe that fails counts as open, so the batch is
/// refused rather than risking the user's transaction. (duckdb-rs's
/// `is_autocommit` is a stub that always says true.) Logs nothing.
fn transaction_open(conn: &Connection) -> bool {
    let txid = || conn.query_row("SELECT txid_current()", [], |row| row.get::<_, i64>(0));
    match (txid(), txid()) {
        (Ok(first), Ok(second)) => first == second,
        _ => true,
    }
}

/// `Driver::transaction`: every statement or none, each with its bound
/// values and its `expect_rows`.
pub(crate) fn transaction(
    conn: &Connection,
    worker: &Worker,
    statements: &[(String, Vec<DuckValue>, Option<ExpectRows>)],
) -> Result<Vec<u64>, TransactionError> {
    let no_params = || params_from_iter(std::iter::empty::<DuckValue>());

    // A BEGIN inside a transaction opened by hand would fail and make
    // DuckDB abort the user's transaction, so refuse before sending it.
    if transaction_open(conn) {
        return Err(DbError {
            message: TRANSACTION_ALREADY_OPEN.to_string(),
            code: TRANSACTION_OPEN.to_string(),
        }
        .into());
    }
    conn.execute("BEGIN", no_params())
        .map_err(DbError::execute_error)?;

    // The statement running, so a panic can name it too.
    let current = std::cell::Cell::new(None);
    let body = catch_unwind(AssertUnwindSafe(
        || -> Result<Vec<u64>, TransactionError> {
            let mut counts = Vec::with_capacity(statements.len());
            for (index, (sql, bound, expect)) in statements.iter().enumerate() {
                if worker.is_cancelled() {
                    return Err(DbError::execute_error("cancelled").into());
                }
                current.set(Some(index));
                let at = |e| TransactionError::at(index, e);
                let affected = prepare(conn, worker, Op::Execute, sql)
                    .map_err(at)?
                    .execute(params_from_iter(bound.iter()))
                    .map_err(|e| at(DbError::execute_error(e)))?;
                if let Some(expect) = expect {
                    expect.check(index, affected as u64).map_err(at)?;
                }
                counts.push(affected as u64);
            }
            current.set(None);
            // Nobody would learn the outcome: roll back instead.
            if worker.is_cancelled() {
                return Err(DbError::execute_error("cancelled").into());
            }
            conn.execute("COMMIT", no_params())
                .map_err(DbError::execute_error)?;
            Ok(counts)
        },
    ));
    let outcome = match body {
        Ok(outcome) => outcome,
        Err(payload) => Err(TransactionError {
            index: current.get(),
            error: DbError::execute_error(format!(
                "DuckDB panicked: {}",
                blocking::panic_message(&*payload)
            )),
        }),
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

/// [`READ_ONLY_WRAPPER`] with `LIMIT` one past the row cap's `limit` (the
/// caller's `max_rows`, or the driver's own cap without it), so DuckDB stops
/// there instead of materializing a huge result before the cap sees it: the
/// one row past it is how the cap tells `RESULT_TOO_LARGE` or `truncated`.
/// The limit is a number, never SQL text from the caller.
fn read_only_wrapper(limit: usize) -> String {
    let limit = limit.saturating_add(1);
    format!("{READ_ONLY_WRAPPER} LIMIT {limit}")
}

/// DuckDB's refusals on the read-only path: `query()`'s, and the read-only
/// transaction's (`nextval`, a write `query()` let through).
const READ_ONLY_REFUSALS: &[&str] = &[
    "Expected a single SELECT statement",
    "transaction is launched in read-only mode",
];

/// A refusal on the read-only path as `READ_ONLY`; anything else as is.
fn read_only_refusal(e: DbError) -> DbError {
    if READ_ONLY_REFUSALS.iter().any(|r| e.message.contains(r)) {
        DbError::read_only(e.message)
    } else {
        e
    }
}

/// Runs `body` in a `BEGIN TRANSACTION READ ONLY` that is always rolled
/// back, a panic or a cancel included; the connection is the call's own and
/// is dropped after the call anyway, which ends any transaction a failed
/// ROLLBACK left.
fn in_read_only_transaction(
    conn: &Connection,
    body: impl FnOnce() -> Result<(), DbError>,
) -> Result<(), DbError> {
    let no_params = || params_from_iter(std::iter::empty::<DuckValue>());
    if let Err(e) = conn.execute("BEGIN TRANSACTION READ ONLY", no_params()) {
        // An interrupt can land in BEGIN. Whether or not it opened the
        // transaction, make sure none is left behind.
        let _ = conn.execute("ROLLBACK", no_params());
        return Err(DbError::query_error(e));
    }
    let body = catch_unwind(AssertUnwindSafe(body));
    rollback_read_only(conn);
    body.unwrap_or_else(|payload| {
        Err(DbError::query_error(format!(
            "DuckDB panicked: {}",
            blocking::panic_message(&*payload)
        )))
    })
}

/// `Driver::query_read_only`, on the call's own connection: `BEGIN
/// TRANSACTION READ ONLY`, the user's SQL through [`read_only_wrapper`] (at
/// most `limit + 1` rows, materialized), `ROLLBACK`.
pub(crate) fn read_only(
    conn: &Connection,
    worker: &Worker,
    sql: &str,
    limit: usize,
    sink: &mut dyn ChunkSink,
) -> Result<(), DbError> {
    let wrapper = read_only_wrapper(limit);
    let bound = [DuckValue::Text(sql.to_string())];
    in_read_only_transaction(conn, || {
        rows(
            conn,
            worker,
            &wrapper,
            &bound,
            Execution::Materialized,
            sink,
        )
    })
    .map_err(|mut e| {
        // DuckDB points at the wrapper ("LINE 1: SELECT * FROM query(?)"
        // and a caret), which isn't the SQL the user or the model wrote.
        if let Some(at) = e.message.find(&format!("\n\nLINE 1: {READ_ONLY_WRAPPER}")) {
            e.message.truncate(at);
        }
        read_only_refusal(e)
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

/// `Driver::explain_read_only`, on the call's own connection: the EXPLAIN in
/// a `BEGIN TRANSACTION READ ONLY` that is always rolled back, as
/// [`read_only`] does for queries. `explain` is one statement (checked by
/// the caller): duckdb-rs's `prepare` runs every statement but the last
/// itself, so `EXPLAIN SELECT 1; DELETE …` would run the DELETE for real.
pub(crate) fn explain_read_only(
    conn: &Connection,
    worker: &Worker,
    explain: &str,
    bound: &[DuckValue],
    sink: &mut dyn ChunkSink,
) -> Result<(), DbError> {
    in_read_only_transaction(conn, || {
        rows(conn, worker, explain, bound, Execution::Materialized, sink)
    })
    .map_err(read_only_refusal)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use seaquel_engine::RowCap;

    use crate::ipc::{decode_batches, KindRules};
    use crate::test_reference as reference;

    /// The helper's sink in miniature: the statement's Arrow schema and
    /// chunks, written as an IPC stream.
    #[derive(Default)]
    struct ArrowSink {
        schema: Option<duckdb::arrow::datatypes::SchemaRef>,
        chunks: Vec<StructArray>,
        finished: bool,
    }

    impl ChunkSink for ArrowSink {
        fn columns(&mut self, stmt: &Statement<'_>) -> Result<(), DbError> {
            self.schema = Some(stmt.schema());
            Ok(())
        }

        fn chunk(&mut self, chunk: StructArray) -> Result<Flow, DbError> {
            self.chunks.push(chunk);
            Ok(Flow::Continue)
        }

        fn finish(&mut self) -> Result<(), DbError> {
            self.finished = true;
            Ok(())
        }
    }

    impl ArrowSink {
        fn ipc(&self) -> Vec<u8> {
            let schema = self.schema.clone().expect("no columns");
            let mut out = Vec::new();
            let mut w = arrow_ipc::writer::StreamWriter::try_new(&mut out, &schema).unwrap();
            for chunk in &self.chunks {
                w.write(&duckdb::arrow::record_batch::RecordBatch::from(chunk))
                    .unwrap();
            }
            w.finish().unwrap();
            drop(w);
            out
        }
    }

    fn connection() -> Connection {
        let config = serde_json::from_value(serde_json::json!({
            "driver": "duckdb",
            "path": ":memory:"
        }))
        .unwrap();
        open_sessions(&config).unwrap().0
    }

    /// The chunks a sink receives decode to the reference
    /// (`test_reference`): read back with `Kind::of_field` (the IPC path),
    /// they give the fixture's cell for every typed-cell case, and the
    /// literal rows for chunked and empty results, an ENUM and the rest,
    /// materialized and streamed.
    #[test]
    fn arrow_sink_chunks_decode_to_the_reference() {
        let conn = connection();
        let cases = reference::all();
        let interrupt = conn.interrupt_handle();
        let mut failures = Vec::new();
        for case in &cases {
            for sql in &case.setup {
                conn.execute_batch(sql).unwrap();
            }
            for execution in [Execution::Materialized, Execution::Streaming] {
                let (_call, worker) = blocking::call(interrupt.clone());
                let mut sink = ArrowSink::default();
                if let Err(e) = rows(&conn, &worker, &case.select, &[], execution, &mut sink) {
                    failures.push(format!("{} ({execution:?}): {e:?}", case.select));
                    continue;
                }
                assert!(sink.finished, "{} ({execution:?})", case.select);
                let cap = RowCap::fail(seaquel_engine::max_query_rows());
                match decode_batches(&sink.ipc(), cap, KindRules::default()) {
                    Ok(r) => {
                        if let Err(e) = case.check(&r.columns, &r.rows) {
                            failures.push(format!("{e} ({execution:?})"));
                        }
                    }
                    Err(e) => failures.push(format!("{} ({execution:?}): {e:?}", case.select)),
                }
            }
            for sql in &case.teardown {
                conn.execute_batch(sql).unwrap();
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} differ:\n{}",
            failures.len(),
            cases.len() * 2,
            failures.join("\n")
        );
    }

    /// `sql`'s rows through a sink and the IPC reader.
    fn decoded(conn: &Connection, sql: &str) -> Vec<Vec<Value>> {
        let (_call, worker) = blocking::call(conn.interrupt_handle());
        let mut sink = ArrowSink::default();
        rows(conn, &worker, sql, &[], Execution::Materialized, &mut sink).unwrap();
        let cap = RowCap::fail(seaquel_engine::max_query_rows());
        decode_batches(&sink.ipc(), cap, KindRules::default())
            .unwrap()
            .rows
    }

    /// Interrupting DuckDB in the middle of a streamed result makes the
    /// next fetch fail, and that is an error, not a panic: `step()`, not
    /// duckdb-rs's Arrow iterator, which panics on it.
    #[test]
    fn an_interrupt_mid_stream_is_an_error_not_a_panic() {
        struct Interrupting {
            interrupt: Arc<duckdb::InterruptHandle>,
            chunks: usize,
        }
        impl ChunkSink for Interrupting {
            fn columns(&mut self, _: &Statement<'_>) -> Result<(), DbError> {
                Ok(())
            }
            fn chunk(&mut self, _: StructArray) -> Result<Flow, DbError> {
                self.chunks += 1;
                self.interrupt.interrupt();
                Ok(Flow::Continue)
            }
            fn finish(&mut self) -> Result<(), DbError> {
                panic!("the result ended");
            }
        }

        let conn = connection();
        let interrupt = conn.interrupt_handle();
        let (_call, worker) = blocking::call(interrupt.clone());
        let mut sink = Interrupting {
            interrupt,
            chunks: 0,
        };
        let r = catch_unwind(AssertUnwindSafe(|| {
            rows(
                &conn,
                &worker,
                "SELECT i FROM range(100000000) t(i)",
                &[],
                Execution::Streaming,
                &mut sink,
            )
        }))
        .expect("panicked");
        let e = r.unwrap_err();
        assert_eq!(e.code, "QUERY_ERROR");
        assert!(e.message.contains("Interrupted"), "{}", e.message);
        assert_eq!(sink.chunks, 1);

        // The connection is fine.
        assert_eq!(decoded(&conn, "SELECT 42 AS n"), vec![vec![Value::Int(42)]]);
    }

    /// A sink that stops ends the result there: no `finish`, no error, and
    /// with streaming execution no more of the query runs.
    #[test]
    fn a_sink_that_stops_ends_the_result() {
        #[derive(Default)]
        struct FirstChunk {
            chunks: usize,
        }
        impl ChunkSink for FirstChunk {
            fn columns(&mut self, _: &Statement<'_>) -> Result<(), DbError> {
                Ok(())
            }
            fn chunk(&mut self, _: StructArray) -> Result<Flow, DbError> {
                self.chunks += 1;
                Ok(Flow::Stop)
            }
            fn finish(&mut self) -> Result<(), DbError> {
                panic!("finished after a stop");
            }
        }

        let conn = connection();
        let interrupt = conn.interrupt_handle();
        let (_call, worker) = blocking::call(interrupt);
        let mut sink = FirstChunk::default();
        let started = tokio::time::Instant::now();
        rows(
            &conn,
            &worker,
            "SELECT i, md5(i::VARCHAR) FROM range(3000000000) t(i)",
            &[],
            Execution::Streaming,
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.chunks, 1);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
    }

    fn file_config(path: &std::path::Path, create_if_missing: bool) -> ConnectConfig {
        serde_json::from_value(serde_json::json!({
            "driver": "duckdb",
            "path": path.to_str().unwrap(),
            "create_if_missing": create_if_missing,
        }))
        .unwrap()
    }

    #[test]
    fn missing_file_is_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.duckdb");
        let err = open_sessions(&file_config(&path, false)).err().unwrap();
        assert_eq!(err.code, "FILE_NOT_FOUND");
        assert!(!path.exists(), "database file must not be created");
    }

    #[test]
    fn create_if_missing_creates_file_and_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("new.duckdb");
        open_sessions(&file_config(&path, true)).unwrap();
        assert!(path.exists(), "database file should be created");
    }

    #[test]
    fn in_memory_needs_no_file() {
        let _ = connection();
    }

    /// DuckDB clears its interrupt flag when a statement is prepared or
    /// starts executing, so an interrupt that lands before that is lost;
    /// the cancelled check after prepare must catch it. The drop happens
    /// inside the call, deterministically, while it holds the connection.
    /// Without the check the query runs to the end (seconds; the per-chunk
    /// check then still reports "cancelled"), so the test times it. Both
    /// executions: the streaming one clears the flag too.
    #[test]
    fn cancel_that_lands_before_execute_is_not_lost() {
        let conn = connection();
        let interrupt = conn.interrupt_handle();
        let conn = std::sync::Mutex::new(conn);

        for execution in [Execution::Materialized, Execution::Streaming] {
            let (call, worker) = blocking::call(interrupt.clone());
            let started = tokio::time::Instant::now();
            let r = worker.run(&conn, Op::Query, move |conn, worker| {
                drop(call); // flags the call and interrupts DuckDB
                let mut sink = ArrowSink::default();
                rows(
                    conn,
                    worker,
                    "SELECT sum(i % 7) FROM range(1500000000) t(i)",
                    &[],
                    execution,
                    &mut sink,
                )
            });
            let took = started.elapsed();
            let e = r.unwrap_err();
            assert_eq!(e.code, "QUERY_ERROR");
            assert!(e.message.contains("cancelled"), "{}", e.message);
            assert!(
                took < std::time::Duration::from_millis(500),
                "the query ran ({execution:?}): {took:?}"
            );
        }

        // The connection is fine.
        let conn = conn.into_inner().unwrap();
        assert_eq!(decoded(&conn, "SELECT 42"), vec![vec![Value::Int(42)]]);
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
}
