//! The browser `Driver`: DuckDB-WASM through the page's bridge (phase 8,
//! Decisions 8–11).
//!
//! - **One DuckDB-WASM connection per Core connection**, on the page's one
//!   database; the connect config's path is ignored. A `restricted` connect
//!   (the MCP server's) is `NOT_SUPPORTED`.
//! - **Calls take turns** on the connection (an async mutex, as the native
//!   driver's blocking thread holds its connection), so a transaction or a
//!   stream is never interleaved with another call.
//! - **Every statement runs as a pending query**, unprepared: bound values
//!   are written in as literals ([`super::binds`]), the result arrives as
//!   IPC stream chunks, and dropping the call before its result was read to
//!   the end cancels the query in DuckDB (`cancelPendingQuery`).
//! - **The read-only path and its EXPLAIN** open a connection of their own
//!   per call, in `BEGIN TRANSACTION READ ONLY`, and roll back and close it
//!   on every outcome, a dropped call included.
//!
//! Nothing here panics: bridge answers are checked, and the decoder reads
//! validated arrays.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use arrow_array::RecordBatch;
use futures::lock::Mutex;
use log::warn;

use seaquel_engine::{
    not_supported, BatchStatement, BoxStream, CancellationToken, CappedResult, ConnectConfig,
    DatabaseStatistics, DbError, Dialect, Driver, Engine, ExecuteResult, ExplainResult,
    QueryResult, ReadOnlyOptions, RowCap, SchemaColumn, SchemaIndex, SchemaTable, StreamBatch,
    TransactionError, Value, TRANSACTION_ALREADY_OPEN, TRANSACTION_OPEN,
};
use seaquel_sql::params::duckdb_bind_literal;

use super::binds::inline_binds;
use super::bridge::{Bridge, DuckDbBridge};
use super::ipc::{read_file, Columns, IpcStream, KindRules};
use crate::dialect::DuckdbDialect;
use crate::introspect;

/// DuckDB-WASM 1.4.3's Arrow: HUGEINT comes as a bare `Decimal128(38, 0)`.
const RULES: KindRules = KindRules {
    decimal38_is_hugeint: true,
};

/// Rows per streamed batch, as in the native driver.
const BATCH_SIZE: usize = 5000;

/// The DuckDB engine over the page's DuckDB-WASM. Its id is `duckdb`, so
/// saved rows and the GUI's routing don't change.
pub fn browser_engine(bridge: DuckDbBridge) -> Arc<dyn Engine> {
    // wasm32 has one thread: nothing here is sent anywhere, and `Engine`
    // asks for no `Send` there.
    #[allow(clippy::arc_with_non_send_sync)]
    Arc::new(BrowserEngine {
        bridge: Rc::new(Bridge::new(bridge)),
    })
}

struct BrowserEngine {
    bridge: Rc<Bridge>,
}

#[seaquel_runtime::async_trait]
impl Engine for BrowserEngine {
    fn id(&self) -> &'static str {
        "duckdb"
    }

    async fn open(&self, config: &ConnectConfig) -> Result<Arc<dyn Driver>, DbError> {
        if config.restricted.unwrap_or(false) {
            return Err(not_supported(
                "A restricted DuckDB connection in the browser",
            ));
        }
        let conn = connect(&self.bridge)
            .await
            .map_err(DbError::connection_error)?;
        #[allow(clippy::arc_with_non_send_sync)]
        Ok(Arc::new(BrowserDriver {
            bridge: self.bridge.clone(),
            conn,
            turn: Mutex::new(()),
            closed: Cell::new(false),
        }))
    }

    fn dialect(&self) -> Option<&dyn Dialect> {
        Some(&DuckdbDialect)
    }
}

/// Opens a DuckDB-WASM connection. If the caller stops waiting, the
/// connection is closed once it arrives, so none is left open.
async fn connect(bridge: &Rc<Bridge>) -> Result<u32, String> {
    struct Connecting {
        bridge: Rc<Bridge>,
        promise: Option<js_sys::Promise>,
    }
    impl Drop for Connecting {
        fn drop(&mut self) {
            if let Some(promise) = self.promise.take() {
                let bridge = self.bridge.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Ok(v) = super::bridge::settle(Ok(promise)).await {
                        if let Ok(id) = super::bridge::connection_id(v) {
                            bridge.post_close(id);
                        }
                    }
                });
            }
        }
    }
    let promise = bridge.connect_promise()?;
    let mut guard = Connecting {
        bridge: bridge.clone(),
        promise: Some(promise.clone()),
    };
    let answer = super::bridge::settle(Ok(promise)).await;
    guard.promise = None;
    super::bridge::connection_id(answer?)
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Query,
    Execute,
}

impl Op {
    fn error(self, e: impl std::fmt::Display) -> DbError {
        match self {
            Op::Query => DbError::query_error(e),
            Op::Execute => DbError::execute_error(e),
        }
    }
}

/// Cancels the query running on `conn` when dropped while armed.
struct CancelOnDrop<'b> {
    bridge: &'b Bridge,
    conn: u32,
    armed: bool,
}

impl Drop for CancelOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.bridge.post_cancel(self.conn);
        }
    }
}

/// A statement running as a pending query. Dropped before its result was
/// read to the end (from its start on, while DuckDB still executes it), it
/// cancels the query: `cancelPendingQuery` stops one still executing (S4),
/// and a result being fetched is discarded by the connection's next
/// statement.
struct Pending<'b> {
    guard: CancelOnDrop<'b>,
    columns: Arc<Columns>,
    /// `None` once the whole result is in `ready` (an ENUM SELECT read
    /// again as a file).
    stream: Option<IpcStream>,
    /// Batches already read: those that came with the schema, or the
    /// whole result.
    ready: Vec<RecordBatch>,
}

/// Whether `sql` is exactly one SELECT, which is safe to run a second time.
fn is_one_select(sql: &str) -> bool {
    use seaquel_sql::statements::{query_type, QueryType};
    use seaquel_sql::SqlEngine::Duckdb;
    seaquel_sql::scan::split_statements(sql, Duckdb).len() == 1
        && query_type(sql, Duckdb) == QueryType::Select
}

impl<'b> Pending<'b> {
    /// Starts `sql` and waits for its result's schema. `cancel`, when given,
    /// stops the wait (and so the query).
    async fn start(
        bridge: &'b Bridge,
        conn: u32,
        sql: &str,
        op: Op,
        cancel: Option<&CancellationToken>,
    ) -> Result<Pending<'b>, DbError> {
        // Armed before the statement is sent: a drop at any await below
        // cancels it.
        let mut guard = CancelOnDrop {
            bridge,
            conn,
            armed: true,
        };
        let mut header = bridge
            .start_pending(conn, sql)
            .await
            .map_err(|e| op.error(e))?;
        let header = loop {
            if let Some(header) = header {
                break header;
            }
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(op.error("cancelled"));
            }
            header = bridge.poll_pending(conn).await.map_err(|e| op.error(e))?;
        };
        let (stream, ready) = IpcStream::start(header, RULES)?;
        if stream.dictionary_column().is_some() && is_one_select(sql) {
            // DuckDB-WASM's pending results carry no ENUM dictionary; its
            // `runQuery` files do. A lone SELECT changes nothing, so it is
            // cancelled and run again that way (not cancellable, and read
            // whole before the caps apply, as natively). Anything else has
            // already run: its ENUM column fails with `UNSUPPORTED_TYPE`
            // when its rows are read.
            bridge.post_cancel(conn);
            let file = bridge.run_query(conn, sql).await.map_err(|e| op.error(e))?;
            guard.armed = false;
            let (columns, batches) = read_file(&file, RULES)?;
            return Ok(Pending {
                guard,
                columns,
                stream: None,
                ready: batches,
            });
        }
        Ok(Pending {
            guard,
            columns: stream.columns(),
            stream: Some(stream),
            ready,
        })
    }

    fn columns(&self) -> Arc<Columns> {
        self.columns.clone()
    }

    /// The next batches, `None` at the end of the result.
    async fn next(
        &mut self,
        op: Op,
        cancel: Option<&CancellationToken>,
    ) -> Result<Option<Vec<RecordBatch>>, DbError> {
        if !self.ready.is_empty() {
            return Ok(Some(std::mem::take(&mut self.ready)));
        }
        let Some(stream) = self.stream.as_mut() else {
            return Ok(None);
        };
        while self.guard.armed {
            if cancel.is_some_and(CancellationToken::is_cancelled) {
                return Err(op.error("cancelled"));
            }
            match self
                .guard
                .bridge
                .fetch_chunk(self.guard.conn)
                .await
                .map_err(|e| op.error(e))?
            {
                None => {}
                Some(bytes) if bytes.is_empty() => {
                    // Read to the end: nothing left to cancel.
                    self.guard.armed = false;
                    let batches = stream.finish()?;
                    if !batches.is_empty() {
                        return Ok(Some(batches));
                    }
                }
                Some(bytes) => {
                    let batches = stream.push(bytes)?;
                    if !batches.is_empty() {
                        return Ok(Some(batches));
                    }
                }
            }
        }
        Ok(None)
    }
}

/// Runs `sql` on `conn` and collects its rows under `cap`: past it,
/// `RESULT_TOO_LARGE` or the rows so far with `truncated` set (the query is
/// then cancelled). An empty result keeps its column names.
async fn query_capped(
    bridge: &Bridge,
    conn: u32,
    sql: &str,
    cap: RowCap,
) -> Result<CappedResult, DbError> {
    let mut pending = Pending::start(bridge, conn, sql, Op::Query, None).await?;
    let columns = pending.columns();
    let mut rows = Vec::new();
    let mut kept_bytes = 0;
    let mut truncated = false;
    'read: while let Some(batches) = pending.next(Op::Query, None).await? {
        for batch in &batches {
            if !columns.collect(batch, cap, &mut rows, &mut kept_bytes)? {
                truncated = true;
                break 'read;
            }
        }
    }
    Ok(CappedResult {
        columns: columns.names.clone(),
        rows,
        truncated,
    })
}

/// Runs `sql` on `conn` to the end and returns the rows it changed: the
/// `Count` column DuckDB answers INSERT, UPDATE and DELETE with (and
/// `CREATE TABLE … AS`), 0 for anything else. Rows of other results are
/// read (so the statement runs to its end, as natively) but not decoded.
async fn execute_on(bridge: &Bridge, conn: u32, sql: &str, op: Op) -> Result<u64, DbError> {
    let mut pending = Pending::start(bridge, conn, sql, op, None).await?;
    let columns = pending.columns();
    let is_count = columns.names.len() == 1 && columns.names[0] == "Count";
    let mut affected = None;
    while let Some(batches) = pending.next(op, None).await? {
        for batch in &batches {
            if is_count && affected.is_none() && batch.num_rows() > 0 {
                affected = match columns.row(batch, 0)?.first() {
                    Some(Value::Int(n)) => Some(u64::try_from(*n).unwrap_or(0)),
                    _ => Some(0),
                };
            }
        }
    }
    Ok(affected.unwrap_or(0))
}

/// Whether a transaction opened by hand is running on `conn`: inside one,
/// two `txid_current()` calls return the same id; in autocommit each
/// statement has its own. A probe that fails counts as open, as natively.
async fn transaction_open(bridge: &Bridge, conn: u32) -> bool {
    let cap = RowCap::fail(2);
    let txid = || async {
        query_capped(bridge, conn, "SELECT txid_current()", cap)
            .await
            .ok()
            .and_then(|r| r.rows.into_iter().next())
            .and_then(|row| row.into_iter().next())
    };
    match (txid().await, txid().await) {
        (Some(first), Some(second)) => first == second,
        _ => true,
    }
}

/// The read-only wrapper's text, which DuckDB's errors point at.
const READ_ONLY_WRAPPER: &str = "SELECT * FROM query(";

/// DuckDB's refusals on the read-only path, as in the native driver.
const READ_ONLY_REFUSALS: &[&str] = &[
    "Expected a single SELECT statement",
    "transaction is launched in read-only mode",
];

/// A refusal on the read-only path becomes `READ_ONLY`, and DuckDB's
/// pointer at the wrapper (`LINE 1: SELECT * FROM query('…`) goes: it isn't
/// the SQL the user or the model wrote.
fn read_only_error(mut e: DbError) -> DbError {
    if let Some(at) = e.message.find(&format!("\n\nLINE 1: {READ_ONLY_WRAPPER}")) {
        e.message.truncate(at);
    }
    if READ_ONLY_REFUSALS.iter().any(|r| e.message.contains(r)) {
        DbError::read_only(e.message)
    } else {
        e
    }
}

/// A connection of the read-only path's own, in a read-only transaction.
/// [`OwnConnection::finish`] rolls back and closes it; dropped without that,
/// it sends a cancel, the `ROLLBACK` and the close anyway.
struct OwnConnection {
    bridge: Rc<Bridge>,
    conn: u32,
    finished: bool,
}

impl OwnConnection {
    async fn begin_read_only(bridge: &Rc<Bridge>) -> Result<Self, DbError> {
        let conn = connect(bridge).await.map_err(DbError::query_error)?;
        let own = OwnConnection {
            bridge: bridge.clone(),
            conn,
            finished: false,
        };
        // A failed BEGIN is a query error; dropping `own` sends the
        // ROLLBACK (harmless without a transaction) and the close.
        execute_on(&own.bridge, conn, "BEGIN TRANSACTION READ ONLY", Op::Query).await?;
        Ok(own)
    }

    /// Rolls back, then closes. Dropped while the ROLLBACK is on its way,
    /// `Drop` sends the cancel, a ROLLBACK and the close.
    async fn finish(mut self) {
        if let Err(e) = execute_on(&self.bridge, self.conn, "ROLLBACK", Op::Query).await {
            // The connection is closed next, which ends the transaction.
            warn!(activity = "db.query_read_only", driver = "duckdb", code = e.code.as_str(); "ROLLBACK of a read-only query failed");
        }
        self.bridge.post_close(self.conn);
        self.finished = true;
    }
}

impl Drop for OwnConnection {
    fn drop(&mut self) {
        if !self.finished {
            self.bridge.post_cancel(self.conn);
            self.bridge.post_query(self.conn, "ROLLBACK");
            self.bridge.post_close(self.conn);
        }
    }
}

/// While a batch runs: dropped before it ended, it cancels the statement
/// and rolls the transaction back, so the next call doesn't find it open.
struct TransactionGuard<'b> {
    bridge: &'b Bridge,
    conn: u32,
    open: bool,
}

impl Drop for TransactionGuard<'_> {
    fn drop(&mut self) {
        if self.open {
            self.bridge.post_cancel(self.conn);
            self.bridge.post_query(self.conn, "ROLLBACK");
        }
    }
}

struct BrowserDriver {
    bridge: Rc<Bridge>,
    conn: u32,
    /// Held by each call for its whole duration.
    turn: Mutex<()>,
    closed: Cell<bool>,
}

impl Drop for BrowserDriver {
    fn drop(&mut self) {
        if !self.closed.get() {
            self.bridge.post_close(self.conn);
        }
    }
}

#[seaquel_runtime::async_trait]
impl Driver for BrowserDriver {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        let sql = inline_binds(sql, &params)?;
        let _turn = self.turn.lock().await;
        let cap = RowCap::fail(seaquel_engine::max_query_rows());
        query_capped(&self.bridge, self.conn, &sql, cap)
            .await
            .map(Into::into)
    }

    /// Streams the result as DuckDB-WASM fetches it, chunk by chunk. Dropping
    /// the stream (Core's cancel) cancels the query; so does `cancel`.
    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        Box::pin(async_stream::stream! {
            let sql = match inline_binds(&sql, &params) {
                Ok(sql) => sql,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };
            let turn = self.turn.lock().await;
            if cancel.is_cancelled() {
                return;
            }
            let mut pending =
                match Pending::start(&self.bridge, self.conn, &sql, Op::Query, Some(&cancel)).await {
                    Ok(p) => p,
                    Err(_) if cancel.is_cancelled() => return,
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                };
            let columns = pending.columns();
            let mut names = Some(columns.names.clone());
            let mut buffer: Vec<Vec<Value>> = Vec::with_capacity(BATCH_SIZE);
            loop {
                let batches = match pending.next(Op::Query, Some(&cancel)).await {
                    Ok(Some(batches)) => batches,
                    Ok(None) => break,
                    Err(_) if cancel.is_cancelled() => return,
                    Err(e) => {
                        yield Err(e);
                        return;
                    }
                };
                for batch in &batches {
                    for row in 0..batch.num_rows() {
                        match columns.row(batch, row) {
                            Ok(row) => buffer.push(row),
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        }
                        if buffer.len() >= BATCH_SIZE {
                            yield Ok(StreamBatch {
                                columns: names.take(),
                                rows: std::mem::replace(&mut buffer, Vec::with_capacity(BATCH_SIZE)),
                                is_final: false,
                                truncated: false,
                            });
                        }
                    }
                }
            }
            // Read to the end: the connection is free before the terminal
            // batch, so a consumer that stops at it doesn't hold the turn.
            drop(pending);
            drop(turn);
            // Terminal batch: carries the columns if no batch did (empty result).
            yield Ok(StreamBatch {
                columns: names,
                rows: buffer,
                is_final: true,
                truncated: false,
            });
        })
    }

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        let sql = inline_binds(sql, &params)?;
        let _turn = self.turn.lock().await;
        let rows_affected = execute_on(&self.bridge, self.conn, &sql, Op::Execute).await?;
        Ok(ExecuteResult {
            rows_affected,
            last_insert_id: None,
        })
    }

    /// Every statement or none, as the native driver: refused before BEGIN
    /// while a transaction opened by hand is running ([`TRANSACTION_OPEN`]),
    /// each statement's `expect_rows` checked, the first failure rolled back
    /// and returned with its index. Dropping the call cancels the statement
    /// and sends the ROLLBACK ([`TransactionGuard`]).
    async fn transaction(
        &self,
        statements: Vec<BatchStatement>,
    ) -> Result<Vec<u64>, TransactionError> {
        let statements = statements
            .iter()
            .enumerate()
            .map(|(index, s)| {
                inline_binds(&s.sql, &s.params)
                    .map(|sql| (sql, s.expect_rows))
                    .map_err(|e| TransactionError::at(index, e))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let _turn = self.turn.lock().await;
        let (bridge, conn) = (&*self.bridge, self.conn);
        if transaction_open(bridge, conn).await {
            return Err(DbError {
                message: TRANSACTION_ALREADY_OPEN.to_string(),
                code: TRANSACTION_OPEN.to_string(),
            }
            .into());
        }
        let mut guard = TransactionGuard {
            bridge,
            conn,
            open: true,
        };
        if let Err(e) = execute_on(bridge, conn, "BEGIN", Op::Execute).await {
            // No transaction to roll back; the guard's ROLLBACK would be
            // harmless, but nothing needs it.
            guard.open = false;
            return Err(e.into());
        }
        let body = async {
            let mut counts = Vec::with_capacity(statements.len());
            for (index, (sql, expect)) in statements.iter().enumerate() {
                let at = |e| TransactionError::at(index, e);
                let affected = execute_on(bridge, conn, sql, Op::Execute)
                    .await
                    .map_err(at)?;
                if let Some(expect) = expect {
                    expect.check(index, affected).map_err(at)?;
                }
                counts.push(affected);
            }
            execute_on(bridge, conn, "COMMIT", Op::Execute).await?;
            Ok::<_, TransactionError>(counts)
        };
        let outcome = body.await;
        if outcome.is_err() {
            // Best effort, and harmless when a failed COMMIT already ended
            // the transaction; the original error is what's returned.
            let _ = execute_on(bridge, conn, "ROLLBACK", Op::Execute).await;
        }
        guard.open = false;
        outcome
    }

    async fn query_read_only(
        &self,
        sql: &str,
        params: Vec<Value>,
        max_rows: Option<usize>,
    ) -> Result<CappedResult, DbError> {
        let options = ReadOnlyOptions::default().with_max_rows(max_rows);
        self.query_read_only_with(sql, params, options).await
    }

    /// On a connection of its own, in a read-only transaction that is always
    /// rolled back: `SELECT * FROM query('<the SQL as a literal>') LIMIT
    /// <cap + 1>`, so DuckDB's `query()` runs exactly one SELECT. `max_rows`
    /// and `max_bytes` are checked while decoding; the timeout is ignored,
    /// since dropping the call cancels the query.
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
        // Refuses a NUL, which DuckDB-WASM would cut the SQL at.
        let sql = inline_binds(sql, &[])?;
        let literal =
            duckdb_bind_literal(&Value::Text(sql)).map_err(|e| DbError::query_error(e.message))?;
        let cap = options.row_cap();
        let wrapper = format!(
            "{READ_ONLY_WRAPPER}{literal}) LIMIT {}",
            cap.limit().saturating_add(1)
        );
        let own = OwnConnection::begin_read_only(&self.bridge).await?;
        let body = query_capped(&own.bridge, own.conn, &wrapper, cap).await;
        own.finish().await;
        body.map_err(read_only_error)
    }

    /// A plain `EXPLAIN (FORMAT JSON)` of one statement (a second is refused
    /// before anything runs), on a connection of its own in a read-only
    /// transaction, as [`BrowserDriver::query_read_only_with`].
    async fn explain_read_only(
        &self,
        sql: &str,
        params: Vec<Value>,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ExplainResult, DbError> {
        if seaquel_sql::scan::split_statements(sql, seaquel_sql::SqlEngine::Duckdb).len() > 1 {
            return Err(DbError::read_only(seaquel_engine::EXPLAIN_ONE_STATEMENT));
        }
        let explain = inline_binds(&introspect::explain_sql(sql, false), &params)?;
        let own = OwnConnection::begin_read_only(&self.bridge).await?;
        let cap = RowCap::fail(seaquel_engine::max_query_rows());
        let body = query_capped(&own.bridge, own.conn, &explain, cap).await;
        own.finish().await;
        let r: QueryResult = body.map_err(read_only_error)?.into();
        Ok(introspect::parse_explain(&r, false))
    }

    /// Closes the DuckDB-WASM connection after the call holding it ends.
    /// `closed` is set once the close has gone out, so a close dropped while
    /// it waits for its turn leaves it to `Drop`.
    async fn close(&self) -> Result<(), DbError> {
        if self.closed.get() {
            return Ok(());
        }
        let _turn = self.turn.lock().await;
        if self.closed.get() {
            return Ok(());
        }
        let closing = self.bridge.close(self.conn);
        self.closed.set(true);
        let _ = closing.await;
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
}
