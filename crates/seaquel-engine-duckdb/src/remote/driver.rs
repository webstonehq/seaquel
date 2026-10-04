//! `RemoteDriver`: every `Driver` method as a call to the helper.
//!
//! Rows come back as Arrow IPC (a schema frame, then batch frames) and are
//! decoded here with the shared reader ([`crate::ipc`]), by the column
//! kinds the helper sends in the schema frame (made from DuckDB's logical
//! types, as the native driver makes them), not by the Arrow fields: a
//! session that reset `arrow_lossless_conversion` sends UHUGEINT and BIT in
//! carriers the fields can't tell from DECIMAL(38, 0) and BLOB. The row and byte
//! caps are applied here too, as both other drivers do; a call stopped by
//! its cap is cancelled in the helper and the rest of its frames dropped.
//! Introspection goes through `query` ([`crate::introspect::calls`]).

use std::sync::{Arc, Mutex};

use arrow_array::RecordBatch;
use seaquel_engine::{
    BatchStatement, BoxStream, CancellationToken, CappedResult, DatabaseStatistics, DbError,
    Driver, ExecuteResult, ExplainResult, QueryResult, ReadOnlyOptions, RowCap, SchemaColumn,
    SchemaIndex, SchemaTable, StreamBatch, TransactionError, Value,
};
use tokio::task::JoinSet;

use super::conn::{Call, Conn, Incoming};
use crate::introspect;
use crate::ipc::{Columns, IpcStream};
use crate::wire::{protocol_error, read_schema_payload, Reply, Request};

/// Rows per streamed batch, as in the native driver.
const BATCH_SIZE: usize = 5000;

/// A DuckDB connection in a helper process of its own.
pub(super) struct RemoteDriver {
    conn: Arc<Conn>,
    /// The connection's tasks. Dropping the driver aborts them, and the
    /// exit watcher's child goes with it (killed, unless `close` let a
    /// closing helper go).
    _tasks: Mutex<JoinSet<()>>,
}

impl RemoteDriver {
    pub(super) fn new(conn: Arc<Conn>, tasks: JoinSet<()>) -> Self {
        RemoteDriver {
            conn,
            _tasks: Mutex::new(tasks),
        }
    }
}

/// A call's rows, read as they arrive.
struct Rows {
    call: Call,
    stream: Option<IpcStream>,
    ended: bool,
}

impl Rows {
    fn start(conn: &Arc<Conn>, request: &Request) -> Result<Rows, DbError> {
        Ok(Rows {
            call: conn.start(request)?,
            stream: None,
            ended: false,
        })
    }

    /// The result's columns, once its schema arrived.
    fn columns(&self) -> Option<Arc<Columns>> {
        self.stream.as_ref().map(IpcStream::columns)
    }

    /// The next batches, `None` at the end of the result. Each batch frame
    /// taken grants its credit back, dictionaries alone included.
    async fn next(&mut self) -> Result<Option<Vec<RecordBatch>>, DbError> {
        while !self.ended {
            match self.call.recv().await? {
                Incoming::Schema(bytes) => {
                    if self.stream.is_some() {
                        return Err(protocol_error("a second schema for one result"));
                    }
                    let (kinds, ipc) = read_schema_payload(bytes)?;
                    let (stream, batches) = IpcStream::start_with_kinds(ipc, kinds)?;
                    self.stream = Some(stream);
                    if !batches.is_empty() {
                        return Ok(Some(batches));
                    }
                }
                Incoming::Batch(bytes) => {
                    self.call.grant();
                    let Some(stream) = self.stream.as_mut() else {
                        return Err(protocol_error("rows before their schema"));
                    };
                    let batches = stream.push(bytes)?;
                    if !batches.is_empty() {
                        return Ok(Some(batches));
                    }
                }
                Incoming::Reply(reply) => {
                    self.ended = true;
                    match reply {
                        Reply::Done => {
                            // No end-of-stream marker: what the reader held
                            // back is read now.
                            if let Some(stream) = self.stream.as_mut() {
                                let batches = stream.finish()?;
                                if !batches.is_empty() {
                                    return Ok(Some(batches));
                                }
                            }
                        }
                        Reply::Error { error, .. } => return Err(error),
                        _ => return Err(protocol_error("an answer that doesn't end rows")),
                    }
                }
            }
        }
        Ok(None)
    }

    /// The whole result under `cap`: past it, `RESULT_TOO_LARGE` or the rows
    /// so far with `truncated` set (the call is then cancelled).
    async fn collect(mut self, cap: RowCap) -> Result<CappedResult, DbError> {
        let mut rows = Vec::new();
        let mut kept_bytes = 0;
        let mut truncated = false;
        'read: while let Some(batches) = self.next().await? {
            let Some(columns) = self.columns() else {
                break;
            };
            for batch in &batches {
                if !columns.collect(batch, cap, &mut rows, &mut kept_bytes)? {
                    truncated = true;
                    break 'read;
                }
            }
        }
        Ok(CappedResult {
            columns: self.columns().map(|c| c.names.clone()).unwrap_or_default(),
            rows,
            truncated,
        })
    }
}

/// A call answered by one control frame and no rows.
async fn answer(conn: &Arc<Conn>, request: &Request) -> Result<Reply, DbError> {
    let mut call = conn.start(request)?;
    match call.recv().await? {
        Incoming::Reply(reply) => Ok(reply),
        Incoming::Schema(_) | Incoming::Batch(_) => {
            Err(protocol_error("rows for a call that returns none"))
        }
    }
}

fn unexpected() -> DbError {
    protocol_error("an answer of the wrong kind")
}

#[seaquel_runtime::async_trait]
impl Driver for RemoteDriver {
    async fn query(&self, sql: &str, params: Vec<Value>) -> Result<QueryResult, DbError> {
        let request = Request::Query {
            sql: sql.to_string(),
            params,
        };
        Rows::start(&self.conn, &request)?
            .collect(RowCap::fail(seaquel_engine::max_query_rows()))
            .await
            .map(Into::into)
    }

    /// Streaming execution in the helper, batches under its credit window.
    /// Dropping the stream, or `cancel`, cancels the call in the helper.
    fn query_stream<'a>(
        &'a self,
        sql: String,
        params: Vec<Value>,
        cancel: CancellationToken,
    ) -> BoxStream<'a, Result<StreamBatch, DbError>> {
        Box::pin(async_stream::stream! {
            let mut rows = match Rows::start(&self.conn, &Request::Stream { sql, params }) {
                Ok(rows) => rows,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };
            let mut columns: Option<Vec<String>> = None;
            let mut sent_columns = false;
            let mut buffer = Vec::with_capacity(BATCH_SIZE);
            loop {
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => None,
                    next = rows.next() => Some(next),
                };
                let batches = match next {
                    // Cancelled: nobody is listening (as natively).
                    None => return,
                    Some(Err(e)) => {
                        yield Err(e);
                        return;
                    }
                    Some(Ok(None)) => break,
                    Some(Ok(Some(batches))) => batches,
                };
                let Some(decoder) = rows.columns() else { break };
                if columns.is_none() {
                    columns = Some(decoder.names.clone());
                }
                for batch in &batches {
                    for i in 0..batch.num_rows() {
                        match decoder.row(batch, i) {
                            Ok(row) => buffer.push(row),
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        }
                        if buffer.len() >= BATCH_SIZE {
                            let batch = StreamBatch {
                                columns: if sent_columns { None } else { columns.clone() },
                                rows: std::mem::replace(&mut buffer, Vec::with_capacity(BATCH_SIZE)),
                                is_final: false,
                                truncated: false,
                            };
                            sent_columns = true;
                            yield Ok(batch);
                            if cancel.is_cancelled() {
                                return;
                            }
                        }
                    }
                }
            }
            if columns.is_none() {
                columns = rows.columns().map(|c| c.names.clone());
            }
            yield Ok(StreamBatch {
                columns: if sent_columns { None } else { Some(columns.unwrap_or_default()) },
                rows: buffer,
                is_final: true,
                truncated: false,
            });
        })
    }

    async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<ExecuteResult, DbError> {
        let request = Request::Execute {
            sql: sql.to_string(),
            params,
        };
        match answer(&self.conn, &request).await? {
            Reply::Executed { rows_affected } => Ok(ExecuteResult {
                rows_affected,
                last_insert_id: None,
            }),
            Reply::Error { error, .. } => Err(error),
            _ => Err(unexpected()),
        }
    }

    /// Every statement or none, in the helper's main session (see the
    /// native driver). Dropping the call cancels it there, which rolls back.
    async fn transaction(
        &self,
        statements: Vec<BatchStatement>,
    ) -> Result<Vec<u64>, TransactionError> {
        match answer(&self.conn, &Request::Transaction { statements }).await? {
            Reply::Committed { rows_affected } => Ok(rows_affected),
            Reply::Error { error, index } => Err(TransactionError { index, error }),
            _ => Err(unexpected().into()),
        }
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

    /// The native driver's read-only path, in the helper on a clone of its
    /// own; the caps applied here. The timeout is ignored: dropping the call
    /// cancels it in the helper.
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
        let cap = options.row_cap();
        let request = Request::ReadOnly {
            sql: sql.to_string(),
            limit: cap.limit(),
        };
        Rows::start(&self.conn, &request)?.collect(cap).await
    }

    /// One statement (a second is refused here, before anything is sent,
    /// and again in the helper), made into a plain EXPLAIN and run
    /// read-only in the helper on a clone of its own.
    async fn explain_read_only(
        &self,
        sql: &str,
        params: Vec<Value>,
        _timeout: Option<std::time::Duration>,
    ) -> Result<ExplainResult, DbError> {
        if seaquel_sql::scan::split_statements(sql, seaquel_sql::SqlEngine::Duckdb).len() > 1 {
            return Err(DbError::read_only(seaquel_engine::EXPLAIN_ONE_STATEMENT));
        }
        let request = Request::ExplainReadOnly {
            sql: sql.to_string(),
            params,
        };
        let r = Rows::start(&self.conn, &request)?
            .collect(RowCap::fail(seaquel_engine::max_query_rows()))
            .await?;
        Ok(introspect::parse_explain(&QueryResult::from(r), false))
    }

    /// Asks the helper to close the database and exit. After 2 s a helper
    /// still checkpointing is left to finish, one that didn't take `close`
    /// is killed.
    /// Every call fails afterwards with `CONNECTION_CLOSED`.
    async fn close(&self) -> Result<(), DbError> {
        self.conn.close().await;
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
