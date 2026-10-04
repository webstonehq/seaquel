//! The native driver: [`crate::session`]'s DuckDB code on tokio's blocking
//! threads, with sinks that decode each result's Arrow chunks into `Value`s
//! by DuckDB's logical types.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex};

use duckdb::arrow::array::{Array, StructArray};
use duckdb::core::{LogicalTypeHandle, LogicalTypeId};
use duckdb::{Connection, InterruptHandle, Statement};
use tokio::sync::mpsc;

use seaquel_engine::{
    BatchStatement, BoxStream, CancellationToken, CappedResult, ConnectConfig, DatabaseStatistics,
    DbError, Driver, ExecuteResult, ExplainResult, QueryResult, ReadOnlyOptions, RowCap,
    SchemaColumn, SchemaIndex, SchemaTable, StreamBatch, TransactionError, Value,
};

use crate::blocking::{self, Op, Worker};
use crate::decode::{self, Kind};
use crate::introspect;
use crate::session::{self, ChunkSink, Execution, Flow};

/// The [`Kind`] of a result column's DuckDB type. duckdb-rs panics on type
/// ids it doesn't know; the caller catches that and uses [`Kind::Plain`], so
/// decoding goes by the Arrow type alone. (The browser driver has no logical
/// types and reads the Arrow field instead: [`Kind::of_field`].)
fn kind_of(t: &LogicalTypeHandle) -> Kind {
    let children = || {
        (0..t.num_children())
            .map(|i| kind_of(&t.child(i)))
            .collect()
    };
    match t.id() {
        LogicalTypeId::Boolean => Kind::Bool,
        LogicalTypeId::Hugeint => Kind::HugeInt,
        LogicalTypeId::UHugeint => Kind::UHugeInt,
        LogicalTypeId::Uuid => Kind::Uuid,
        LogicalTypeId::Bit => Kind::Bit,
        LogicalTypeId::Bignum => Kind::Bignum,
        LogicalTypeId::TimeTZ => Kind::TimeTz,
        LogicalTypeId::Varchar if t.get_alias().as_deref() == Some("JSON") => Kind::Json,
        LogicalTypeId::List | LogicalTypeId::Array => Kind::List(Box::new(kind_of(&t.child(0)))),
        LogicalTypeId::Struct => Kind::Struct(children()),
        LogicalTypeId::Union => Kind::Union(children()),
        LogicalTypeId::Map => Kind::Map(
            Box::new(kind_of(&t.child(0))),
            Box::new(kind_of(&t.child(1))),
        ),
        _ => Kind::Plain,
    }
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

impl DuckdbDriver {
    pub async fn connect(config: &ConnectConfig) -> Result<Self, DbError> {
        let config = config.clone();
        let (conn, read_only) =
            tokio::task::spawn_blocking(move || session::open_sessions(&config))
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

    /// A connection of its own for one read-only call, cloned from
    /// `read_only_source`. Dropped after the call: by the call's future, and
    /// by the blocking task when it finishes (after a cancel, once the
    /// interrupt has landed).
    async fn read_only_session(&self) -> Result<Session, DbError> {
        let source = self.read_only_source.clone();
        let conn = tokio::task::spawn_blocking(move || blocking::lock(&source).try_clone())
            .await
            .map_err(|e| Op::Query.join_error(e))?
            .map_err(DbError::query_error)?;
        Ok(Session::new(conn))
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

/// A result's column names and kinds, read off the executed statement, and
/// the decoding of its cells by [`decode::decode`]. The DuckDB helper sends
/// the same `kinds` to its client in each schema frame
/// (`wire::schema_payload`), so both drivers decode by them.
pub(crate) struct Decoder {
    columns: Vec<String>,
    pub(crate) kinds: Vec<Kind>,
}

impl Decoder {
    pub(crate) fn of(stmt: &Statement<'_>) -> Self {
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
                catch_unwind(AssertUnwindSafe(|| kind_of(&stmt.column_logical_type(i))))
                    .unwrap_or(Kind::Plain)
            })
            .collect();
        Self { columns, kinds }
    }

    /// Row `row` of `chunk`.
    fn row(&self, chunk: &StructArray, row: usize) -> Result<Vec<Value>, DbError> {
        (0..self.kinds.len())
            .map(|i| self.read_cell(chunk, row, i))
            .collect()
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

/// A whole result's rows, collected under a [`RowCap`]: past it,
/// `RESULT_TOO_LARGE` or the rows so far with `truncated` set. A byte budget
/// counts the decoded rows. `query`, the read-only path and EXPLAIN.
struct CappedSink<'w> {
    worker: &'w Worker,
    cap: RowCap,
    decoder: Option<Decoder>,
    rows: Vec<Vec<Value>>,
    kept_bytes: usize,
    truncated: bool,
}

impl<'w> CappedSink<'w> {
    fn new(worker: &'w Worker, cap: RowCap) -> Self {
        Self {
            worker,
            cap,
            decoder: None,
            rows: Vec::new(),
            kept_bytes: 0,
            truncated: false,
        }
    }

    fn into_result(self) -> CappedResult {
        CappedResult {
            columns: self.decoder.map(|d| d.columns).unwrap_or_default(),
            rows: self.rows,
            truncated: self.truncated,
        }
    }
}

impl ChunkSink for CappedSink<'_> {
    fn columns(&mut self, stmt: &Statement<'_>) -> Result<(), DbError> {
        self.decoder = Some(Decoder::of(stmt));
        Ok(())
    }

    fn chunk(&mut self, chunk: StructArray) -> Result<Flow, DbError> {
        let Some(decoder) = &self.decoder else {
            return Err(DbError::query_error("DuckDB sent rows before its columns"));
        };
        for row in 0..chunk.len() {
            let row = decoder.row(&chunk, row)?;
            if self.worker.is_cancelled() {
                return Err(DbError::query_error("cancelled"));
            }
            if !self.cap.admit(self.rows.len(), self.kept_bytes)? {
                self.truncated = true;
                return Ok(Flow::Stop);
            }
            if self.cap.max_bytes().is_some() {
                self.kept_bytes = self
                    .kept_bytes
                    .saturating_add(seaquel_engine::row_bytes(&row));
            }
            self.rows.push(row);
        }
        Ok(Flow::Continue)
    }

    fn finish(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

/// Runs `sql` (materialized) and collects its rows under `cap`. The chunk
/// being read is already in DuckDB's memory, and so is the whole result (up
/// to the wrapper's `LIMIT` on the read-only path).
fn query_capped(
    conn: &Connection,
    worker: &Worker,
    sql: &str,
    bound: &[duckdb::types::Value],
    cap: RowCap,
) -> Result<CappedResult, DbError> {
    let mut sink = CappedSink::new(worker, cap);
    session::rows(conn, worker, sql, bound, Execution::Materialized, &mut sink)?;
    Ok(sink.into_result())
}

/// Rows per streamed batch, as in the sqlx drivers.
const BATCH_SIZE: usize = 5000;

/// Batches in flight between the blocking producer and the stream: the
/// DuckDB helper's credit window too (`wire::STREAM_CREDIT`, pinned by a
/// test). With streaming execution DuckDB runs ahead of the producer only
/// until `streaming_buffer_size` bytes (10^6 by default) are buffered and
/// then waits, so a stream nobody reads holds that plus these batches.
const STREAM_BUFFER: usize = 2;

/// `query_stream`'s rows, sent in [`BATCH_SIZE`] batches until the result
/// ends, the stream is dropped or `cancel` fires.
struct StreamSink<'a> {
    worker: &'a Worker,
    cancel: &'a CancellationToken,
    tx: &'a mpsc::Sender<Result<StreamBatch, DbError>>,
    decoder: Option<Decoder>,
    /// The columns, until a batch carried them.
    columns: Option<Vec<String>>,
    buffer: Vec<Vec<Value>>,
}

impl StreamSink<'_> {
    /// Nobody will read what comes next.
    fn stopped(&self) -> bool {
        self.worker.is_cancelled() || self.cancel.is_cancelled() || self.tx.is_closed()
    }
}

impl ChunkSink for StreamSink<'_> {
    fn columns(&mut self, stmt: &Statement<'_>) -> Result<(), DbError> {
        let decoder = Decoder::of(stmt);
        self.columns = Some(decoder.columns.clone());
        self.decoder = Some(decoder);
        Ok(())
    }

    fn chunk(&mut self, chunk: StructArray) -> Result<Flow, DbError> {
        let Some(decoder) = &self.decoder else {
            return Err(DbError::query_error("DuckDB sent rows before its columns"));
        };
        for row in 0..chunk.len() {
            let row = decoder.row(&chunk, row)?;
            if self.stopped() {
                return Ok(Flow::Stop);
            }
            self.buffer.push(row);
            if self.buffer.len() >= BATCH_SIZE {
                let batch = StreamBatch {
                    columns: self.columns.take(),
                    rows: std::mem::replace(&mut self.buffer, Vec::with_capacity(BATCH_SIZE)),
                    is_final: false,
                    truncated: false,
                };
                if self.tx.blocking_send(Ok(batch)).is_err() {
                    return Ok(Flow::Stop);
                }
            }
        }
        Ok(Flow::Continue)
    }

    /// The terminal batch: carries the columns if no batch did (an empty
    /// result).
    fn finish(&mut self) -> Result<(), DbError> {
        let _ = self.tx.blocking_send(Ok(StreamBatch {
            columns: self.columns.take(),
            rows: std::mem::take(&mut self.buffer),
            is_final: true,
            truncated: false,
        }));
        Ok(())
    }
}

/// `Driver::query_stream`, on the blocking thread, with DuckDB's streaming
/// execution: batches go out as DuckDB produces the rows, so a failing query
/// can fail after its first batches (the error is sent too), and a stream
/// that stops early stops the query.
fn stream_blocking(
    conn: &Connection,
    worker: &Worker,
    sql: &str,
    bound: &[duckdb::types::Value],
    cancel: &CancellationToken,
    tx: &mpsc::Sender<Result<StreamBatch, DbError>>,
) -> Result<(), DbError> {
    let mut sink = StreamSink {
        worker,
        cancel,
        tx,
        decoder: None,
        columns: None,
        buffer: Vec::with_capacity(BATCH_SIZE),
    };
    if sink.stopped() {
        return Ok(());
    }
    match session::rows(conn, worker, sql, bound, Execution::Streaming, &mut sink) {
        // Nobody is listening for the error (a cancel lands as one).
        Err(_) if sink.stopped() => Ok(()),
        outcome => outcome,
    }
}

#[seaquel_runtime::async_trait]
impl Driver for DuckdbDriver {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        let sql = sql.to_string();
        let bound = session::bind_all(&params)?;
        self.run(Op::Query, move |conn, worker| {
            let cap = RowCap::fail(seaquel_engine::max_query_rows());
            query_capped(conn, worker, &sql, &bound, cap).map(Into::into)
        })
        .await
    }

    /// Streams batches from a blocking producer, with DuckDB's streaming
    /// execution (see [`stream_blocking`]). Dropping the stream (Core's
    /// cancel) interrupts the query.
    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        Box::pin(async_stream::stream! {
            let bound = match session::bind_all(&params) {
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
        let bound = session::bind_all(&params)?;
        let rows_affected = self
            .run(Op::Execute, move |conn, worker| {
                session::execute(conn, worker, &sql, &bound)
            })
            .await?;
        Ok(ExecuteResult {
            rows_affected,
            last_insert_id: None,
        })
    }

    /// Every statement or none. The connection is held from BEGIN to
    /// COMMIT, so no other call can interleave. The first failure (or a
    /// panic) rolls back and is returned; bind errors fail before anything
    /// runs, and a statement that affected fewer rows than its
    /// `expect_rows` rolls back with `NO_ROWS_AFFECTED`. Dropping the future
    /// interrupts the running statement, which fails and rolls back. While
    /// a transaction opened by hand is running, the batch is refused before
    /// BEGIN (`TRANSACTION_OPEN`, see `session::transaction`): a failed
    /// nested BEGIN would make DuckDB abort the user's transaction.
    ///
    /// A failure names its statement ([`TransactionError`]): the one DuckDB
    /// refused, whose parameters didn't bind (before anything runs), that
    /// fell short of `expect_rows`, or that panicked. BEGIN, COMMIT and a
    /// cancel between statements name none.
    async fn transaction(
        &self,
        statements: Vec<BatchStatement>,
    ) -> Result<Vec<u64>, TransactionError> {
        let statements = statements
            .into_iter()
            .enumerate()
            .map(|(index, s)| {
                let bound =
                    session::bind_all(&s.params).map_err(|e| TransactionError::at(index, e))?;
                Ok((s.sql, bound, s.expect_rows))
            })
            .collect::<Result<Vec<_>, TransactionError>>()?;
        self.run(Op::Execute, move |conn, worker| {
            Ok(session::transaction(conn, worker, &statements))
        })
        .await?
    }

    /// See `session::read_only`. DuckDB's read-only path takes no bind
    /// values: its one parameter is the user's SQL. Nothing that calls it
    /// passes any.
    async fn query_read_only(
        &self,
        sql: &str,
        params: Vec<Value>,
        max_rows: Option<usize>,
    ) -> Result<CappedResult, DbError> {
        let options = ReadOnlyOptions::default().with_max_rows(max_rows);
        self.query_read_only_with(sql, params, options).await
    }

    /// [`DuckdbDriver::query_read_only`] with the options' `max_rows` and
    /// `max_bytes`. The timeout is ignored: dropping the call interrupts the
    /// query (the session's guard).
    async fn query_read_only_with(
        &self,
        sql: &str,
        params: Vec<Value>,
        options: ReadOnlyOptions,
    ) -> Result<CappedResult, DbError> {
        if !params.is_empty() {
            return Err(DbError::read_only(
                "read-only DuckDB queries take no bind values",
            ));
        }
        let session = self.read_only_session().await?;
        let sql = sql.to_string();
        let cap = options.row_cap();
        run_on(&session, Op::Query, move |conn, worker| {
            let mut sink = CappedSink::new(worker, cap);
            session::read_only(conn, worker, &sql, cap.limit(), &mut sink)?;
            Ok(sink.into_result())
        })
        .await
    }

    async fn close(&self) -> Result<(), DbError> {
        // DuckDB Connection is closed on drop
        Ok(())
    }

    // ── Introspection (see `crate::introspect::calls`) ──

    async fn list_schemas(&self) -> Result<Vec<String>, DbError> {
        introspect::calls::list_schemas(self).await
    }

    async fn schema_tables(&self) -> Result<Vec<SchemaTable>, DbError> {
        introspect::calls::schema_tables(self).await
    }

    async fn table_metadata(
        &self,
        schema: &str,
        table: &str,
    ) -> Result<(Vec<SchemaColumn>, Vec<SchemaIndex>), DbError> {
        introspect::calls::table_metadata(self, schema, table).await
    }

    async fn statistics(&self) -> Result<DatabaseStatistics, DbError> {
        introspect::calls::statistics(self).await
    }

    async fn explain(
        &self,
        sql: &str,
        params: Vec<Value>,
        analyze: bool,
    ) -> Result<ExplainResult, DbError> {
        introspect::calls::explain(self, sql, params, analyze).await
    }

    /// A plain `EXPLAIN (FORMAT JSON)` of one statement (a second is
    /// refused before anything runs), on a connection cloned for the call
    /// like [`DuckdbDriver::query_read_only`]'s, in a read-only transaction
    /// (see `session::explain_read_only`). No timeout: dropping the call
    /// interrupts it.
    async fn explain_read_only(
        &self,
        sql: &str,
        params: Vec<Value>,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ExplainResult, DbError> {
        if seaquel_sql::scan::split_statements(sql, seaquel_sql::SqlEngine::Duckdb).len() > 1 {
            return Err(DbError::read_only(seaquel_engine::EXPLAIN_ONE_STATEMENT));
        }
        let bound = session::bind_all(&params)?;
        let session = self.read_only_session().await?;
        let explain = introspect::explain_sql(sql, false);
        let r = run_on(&session, Op::Query, move |conn, worker| {
            let mut sink = CappedSink::new(worker, RowCap::fail(seaquel_engine::max_query_rows()));
            session::explain_read_only(conn, worker, &explain, &bound, &mut sink)?;
            Ok(QueryResult::from(sink.into_result()))
        })
        .await?;
        Ok(introspect::parse_explain(&r, false))
    }
}

/// The native driver's decoding of `sql`'s rows on `conn`, for the IPC and
/// session comparisons (`ipc.rs`, `session.rs`).
#[cfg(test)]
pub(crate) fn native_rows(conn: &Connection, sql: &str) -> Result<CappedResult, DbError> {
    let interrupt = conn.interrupt_handle();
    let (_call, worker) = blocking::call(interrupt);
    let cap = RowCap::fail(seaquel_engine::max_query_rows());
    query_capped(conn, &worker, sql, &[], cap)
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
        /// `CappedSink` with the first column's kind forced.
        struct Forced<'w>(CappedSink<'w>);
        impl ChunkSink for Forced<'_> {
            fn columns(&mut self, stmt: &Statement<'_>) -> Result<(), DbError> {
                self.0.columns(stmt)?;
                self.0.decoder.as_mut().unwrap().kinds[0] = Kind::Bignum;
                Ok(())
            }
            fn chunk(&mut self, chunk: StructArray) -> Result<Flow, DbError> {
                self.0.chunk(chunk)
            }
            fn finish(&mut self) -> Result<(), DbError> {
                self.0.finish()
            }
        }

        let conn = Connection::open_in_memory().unwrap();
        let (_call, worker) = blocking::call(conn.interrupt_handle());
        let mut sink = Forced(CappedSink::new(&worker, RowCap::fail(100_000)));
        let e = session::rows(
            &conn,
            &worker,
            "SELECT CASE WHEN i < 3000 THEN NULL ELSE '\\x01'::BLOB END AS t FROM range(3001) r(i)",
            &[],
            Execution::Materialized,
            &mut sink,
        )
        .unwrap_err();
        assert_eq!(sink.0.rows, vec![vec![Value::Null]; 3000]);
        assert_eq!(e.code, "UNSUPPORTED_TYPE");
        assert!(e.message.contains("Column \"t\""), "{}", e.message);
        assert!(e.message.contains("BIGNUM of 1 bytes"), "{}", e.message);
    }

    /// The native driver's batches in flight and the DuckDB helper's credit
    /// window are one number, so a stream holds as much in both.
    #[test]
    fn stream_buffer_is_the_helper_s_credit_window() {
        assert_eq!(STREAM_BUFFER as u32, crate::wire::STREAM_CREDIT);
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
    /// check then still reports "cancelled"), so the test times it. Both
    /// executions: the streaming one clears the flag too.
    #[test]
    fn cancel_that_lands_before_execute_is_not_lost() {
        let conn = Connection::open_in_memory().unwrap();
        let interrupt = conn.interrupt_handle();
        let conn = Mutex::new(conn);

        for execution in [Execution::Materialized, Execution::Streaming] {
            let (call, worker) = blocking::call(interrupt.clone());
            let started = tokio::time::Instant::now();
            let r = worker.run(&conn, Op::Query, move |conn, worker| {
                drop(call); // flags the call and interrupts DuckDB
                let mut sink = CappedSink::new(worker, RowCap::fail(100_000));
                session::rows(
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
        let (_call, worker) = blocking::call(interrupt);
        let r = worker
            .run(&conn, Op::Query, |conn, worker| {
                query_capped(conn, worker, "SELECT 42", &[], RowCap::fail(100_000))
            })
            .unwrap();
        assert_eq!(r.rows, vec![vec![Value::Int(42)]]);
    }
}
