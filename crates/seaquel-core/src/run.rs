//! The editor's run (phase 5b): [`Workspace::run`] and [`Workspace::page`].
//!
//! A run is planned by `seaquel_workspace::run::plan` (pure) and carried out
//! here on one of the workspace's connections, under one stream id
//! registered like a query stream's: `Workspace::cancel`, an early cancel,
//! `disconnect` and `close_all` reach it the same way. Every driver call is
//! wrapped in the run's token, so a cancel drops the statement in flight
//! (which stops a streamed or paged SELECT on the server) and the rest never
//! run. Nothing here logs SQL or values: activity, counts, kinds and codes
//! only.

use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::StreamExt;
use log::{debug, info, warn};
use seaquel_engine::{BoxStream, CancellationToken, DbError, QueryResult, StreamBatch};
use seaquel_runtime::Executor;
use seaquel_sql::scan::{count_query, scan, split_statements, tokens, ScanOptions, TokenKind};
use seaquel_sql::statements::{query_type, table_from_select, QueryType};
use seaquel_sql::SqlEngine;
use seaquel_workspace::run::{
    check_text_size, history_item, page_offset, plan, select_kind, HistoryContext, PageParams,
    PageSource, PlanOptions, RunEvent, RunParams, RunTarget, StatementKind, Step, INVALID_ARGUMENT,
};

use crate::{Connection, Core, Value, Workspace, CONNECTION_CLOSED};

/// The code for a run on a Core without an executor, or on a connection
/// whose SQL rules Core doesn't know.
pub(crate) const NOT_SUPPORTED: &str = "NOT_SUPPORTED";

/// The largest page size: a page fetches one row more than it shows, and
/// that row must stay within `max_query_rows()`.
pub(crate) fn max_page_size() -> u32 {
    u32::try_from(seaquel_engine::max_query_rows().saturating_sub(1)).unwrap_or(u32::MAX)
}

pub(crate) fn one(event: RunEvent) -> BoxStream<'static, RunEvent> {
    Box::pin(futures::stream::once(std::future::ready(event)))
}

pub(crate) fn closed_event() -> RunEvent {
    RunEvent::error(
        CONNECTION_CLOSED,
        "Connection was closed while the query was running",
    )
}

fn kind_name(kind: StatementKind) -> &'static str {
    match kind {
        StatementKind::Page => "page",
        StatementKind::Stream => "stream",
        StatementKind::Write => "write",
        StatementKind::Utility => "utility",
    }
}

/// Milliseconds between two clock readings, to 0.01 ms (as the TS rounded).
pub(crate) fn elapsed_ms(start: Duration, end: Duration) -> f64 {
    let ms = end.saturating_sub(start).as_secs_f64() * 1000.0;
    (ms * 100.0).round() / 100.0
}

/// What one statement came to, for the run's history row.
struct Outcome {
    // Read only when the run writes history, which needs `storage`
    // (a `workspace` build without it records nothing).
    #[cfg_attr(not(feature = "storage"), allow(dead_code))]
    elapsed_ms: f64,
    #[cfg_attr(not(feature = "storage"), allow(dead_code))]
    row_count: u64,
    /// A utility statement that returned no rows: history prefers any other.
    hidden_utility: bool,
}

impl Workspace {
    /// Run the editor's text on one of this workspace's connections
    /// (`db.run`), under
    /// `params.stream_id` in this workspace's scope.
    ///
    /// Events: per statement a `statementStart`, its `batch`es and a
    /// `statementDone` or `statementError`; a deferred statement only
    /// `statementDeferred`; a planned failure only `statementError` (with
    /// its `sql`). Then one `done`, or one `error` for a run-level failure
    /// (`CONFIRM_REQUIRED`, `CONNECTION_NOT_FOUND`, `INVALID_ARGUMENT`,
    /// `INVALID_PARAMETERS` at the cursor, `NOT_SUPPORTED` without an
    /// executor). Nothing runs before the destructive check. A statement's
    /// failure doesn't stop the run.
    ///
    /// Cancelled ([`Workspace::cancel`], `close_all`, or dropping the
    /// stream), the run ends with no further event; its connection closed by
    /// `disconnect`, it ends with a `CONNECTION_CLOSED` error. Either way
    /// the statement in flight is dropped and no later one runs.
    ///
    /// A run with `history` whose statements all ran without failing, and
    /// that wasn't cancelled, appends one history row (with the storage
    /// feature); `done.history` is the row. A failed append is logged with
    /// its code and doesn't fail the run.
    pub fn run<'a>(&'a self, core: &'a Core, params: RunParams) -> BoxStream<'a, RunEvent> {
        self.run_from(core, params, crate::WriteOrigin::none())
    }

    /// [`Workspace::run`] for the window or tab `origin`: the history
    /// row's `StorageChanged` event carries it (phase 5d).
    pub fn run_from<'a>(
        &'a self,
        core: &'a Core,
        params: RunParams,
        origin: crate::WriteOrigin,
    ) -> BoxStream<'a, RunEvent> {
        let RunParams {
            connection_id,
            stream_id,
            text,
            target,
            params: values,
            page_size,
            confirmed,
            defer_writes,
            history,
        } = params;
        let target_kind = match target {
            RunTarget::All => "all",
            RunTarget::Current { .. } => "current",
        };
        info!(activity = "db.run", connection_id = connection_id.as_str(), stream_id = stream_id.as_str(), target = target_kind, text_len = text.len(), params = values.as_ref().map(Vec::len), page_size = page_size, confirmed = confirmed, defer_writes = defer_writes, history = history.is_some(); "Run");
        let Some(executor) = core.executor.clone() else {
            return one(no_executor());
        };
        let owner = Some(self.id());
        if let Err(e) = core.connection_as(&connection_id, owner) {
            return one(RunEvent::error(e.code, e.message));
        }
        let (token, closed, guard) =
            match core.register_stream((owner, stream_id), Some(connection_id.clone())) {
                Ok(registered) => registered,
                Err(e) => return one(RunEvent::error(e.code, e.message)),
            };
        Box::pin(async_stream::stream! {
            let _guard = guard;
            if token.is_cancelled() {
                if closed.load(Ordering::SeqCst) {
                    yield closed_event();
                }
                return;
            }
            let connection = match core.connection_as(&connection_id, owner) {
                Ok(connection) => connection,
                Err(e) => {
                    yield RunEvent::error(e.code, e.message);
                    return;
                }
            };
            let Some(engine) = connection.sql_engine else {
                yield no_sql_rules();
                return;
            };
            let values: Option<Vec<(String, Value)>> =
                values.map(|v| v.into_iter().map(|p| (p.name, p.value)).collect());
            let plan = match plan(&text, &target, values.as_deref(), engine, PlanOptions { page_size, defer_writes, max_page_size: max_page_size(), limits: core.run_limits() }) {
                Ok(plan) => plan,
                Err(e) => {
                    debug!(activity = "db.run", code = e.code.as_str(); "Run refused");
                    yield RunEvent::error(e.code, e.message);
                    return;
                }
            };
            debug!(activity = "db.run", statements = plan.statements.len(), destructive = plan.destructive.len(); "Planned");
            if !plan.destructive.is_empty() && !confirmed {
                yield RunEvent::confirm_required(plan.destructive);
                return;
            }

            let planned_count = u32::try_from(plan.statements.len()).unwrap_or(u32::MAX);
            let mut ran = 0usize;
            let mut failed = false;
            let mut first: Option<Outcome> = None;
            let mut first_shown: Option<Outcome> = None;
            for planned in plan.statements {
                if token.is_cancelled() {
                    break;
                }
                let index = planned.index;
                match planned.step {
                    Step::Defer { source, query_type } => {
                        yield RunEvent::StatementDeferred { index, sql: planned.sql, source, query_type };
                    }
                    Step::Fail { code, message } => {
                        failed = true;
                        yield RunEvent::StatementError { index, code, message, elapsed_ms: 0.0, sql: Some(planned.sql) };
                    }
                    Step::Run { source, query_type, kind, table, column_refs } => {
                        yield RunEvent::StatementStart {
                            index,
                            sql: planned.sql,
                            source: source.clone(),
                            query_type,
                            kind,
                            page: 1,
                            page_size,
                            table,
                            column_refs,
                        };
                        let mut had_rows = false;
                        let mut events = execute(&connection, engine, &*executor, &token, index, kind, source, 1, page_size);
                        while let Some(event) = events.next().await {
                            match &event {
                                RunEvent::Batch(_) => had_rows = true,
                                RunEvent::StatementDone { elapsed_ms, total_rows, rows_affected, .. } => {
                                    ran += 1;
                                    let outcome = || Outcome {
                                        elapsed_ms: *elapsed_ms,
                                        row_count: rows_affected.unwrap_or(*total_rows),
                                        hidden_utility: kind == StatementKind::Utility && !had_rows,
                                    };
                                    if first.is_none() {
                                        first = Some(outcome());
                                    }
                                    if first_shown.is_none() && !outcome().hidden_utility {
                                        first_shown = Some(outcome());
                                    }
                                }
                                RunEvent::StatementError { .. } => {
                                    ran += 1;
                                    failed = true;
                                }
                                _ => {}
                            }
                            yield event;
                        }
                    }
                }
            }
            if token.is_cancelled() {
                info!(activity = "db.run", ran = ran; "Run cancelled");
                if closed.load(Ordering::SeqCst) {
                    yield closed_event();
                }
                return;
            }
            let succeeded = ran > 0 && !failed;
            let recorded = match (succeeded, history, first_shown.or(first)) {
                (true, Some(ctx), Some(outcome)) => {
                    self.append_history(&ctx, &plan.history_query, &outcome, &*executor, &origin)
                        .await
                }
                _ => None,
            };
            info!(activity = "db.run", statements = planned_count, ran = ran, succeeded = succeeded, recorded = recorded.is_some(); "Run done");
            yield RunEvent::Done { statements: planned_count, succeeded, history: recorded };
        })
    }

    /// One statement again (`db.page`), from the `source` its run sent:
    /// page `params.page` at `params.page_size`, or streamed whole at page
    /// size 0 or when it limits its own rows. Only a SELECT: anything else
    /// is `INVALID_ARGUMENT`, as are page 0, a page size past the cap and an
    /// offset that overflows. Never records history.
    ///
    /// Events: `statementStart` (its `sql` is the source's), `batch`es,
    /// `statementDone` or `statementError`, then `done`. Registered,
    /// cancelled and closed like [`Workspace::run`].
    pub fn page<'a>(&'a self, core: &'a Core, params: PageParams) -> BoxStream<'a, RunEvent> {
        let PageParams {
            connection_id,
            stream_id,
            source,
            page,
            page_size,
        } = params;
        info!(activity = "db.page", connection_id = connection_id.as_str(), stream_id = stream_id.as_str(), sql_len = source.sql.len(), params = source.params.len(), page = page, page_size = page_size; "Page");
        let Some(executor) = core.executor.clone() else {
            return one(no_executor());
        };
        let owner = Some(self.id());
        if let Err(e) = core.connection_as(&connection_id, owner) {
            return one(RunEvent::error(e.code, e.message));
        }
        let (token, closed, guard) =
            match core.register_stream((owner, stream_id), Some(connection_id.clone())) {
                Ok(registered) => registered,
                Err(e) => return one(RunEvent::error(e.code, e.message)),
            };
        Box::pin(async_stream::stream! {
            let _guard = guard;
            if token.is_cancelled() {
                if closed.load(Ordering::SeqCst) {
                    yield closed_event();
                }
                return;
            }
            let connection = match core.connection_as(&connection_id, owner) {
                Ok(connection) => connection,
                Err(e) => {
                    yield RunEvent::error(e.code, e.message);
                    return;
                }
            };
            let Some(engine) = connection.sql_engine else {
                yield no_sql_rules();
                return;
            };
            if let Err(e) = page_offset(page, page_size, max_page_size())
                .and_then(|_| check_text_size(&source.sql, core.run_limits()))
            {
                yield RunEvent::error(e.code, e.message);
                return;
            }
            // One statement, split as the run splits (a MySQL `/*! … */`
            // or MariaDB `/*M! … */` is code), so nothing can ride along
            // behind the SELECT.
            if split_statements(&source.sql, engine).len() != 1 {
                yield RunEvent::error(INVALID_ARGUMENT, "Only one statement can be paged");
                return;
            }
            let query_type = query_type(&source.sql, engine);
            if query_type != QueryType::Select {
                yield RunEvent::error(INVALID_ARGUMENT, "Only a SELECT can be paged");
                return;
            }
            let kind = select_kind(&source.sql, engine, page_size);
            yield RunEvent::StatementStart {
                index: 0,
                sql: source.sql.clone(),
                table: table_from_select(&source.sql, engine),
                column_refs: seaquel_sql::ast::column_refs(&source.sql, engine),
                source: source.clone(),
                query_type,
                kind,
                page,
                page_size,
            };
            let mut failed = false;
            let mut events = execute(&connection, engine, &*executor, &token, 0, kind, source, page, page_size);
            while let Some(event) = events.next().await {
                failed |= matches!(event, RunEvent::StatementError { .. });
                yield event;
            }
            if token.is_cancelled() {
                if closed.load(Ordering::SeqCst) {
                    yield closed_event();
                }
                return;
            }
            yield RunEvent::Done { statements: 1, succeeded: !failed, history: None };
        })
    }

    /// Append the run's history row; `None` (logged with its code) when the
    /// append fails, e.g. for a connection that isn't saved.
    #[cfg(feature = "storage")]
    async fn append_history(
        &self,
        ctx: &HistoryContext,
        query: &str,
        outcome: &Outcome,
        executor: &dyn Executor,
        origin: &crate::WriteOrigin,
    ) -> Option<seaquel_types::storage::PersistedQueryHistoryItem> {
        let item = history_item(
            ctx,
            query,
            outcome.elapsed_ms,
            outcome.row_count,
            executor.unix_time(),
            format!("hist-{}", uuid::Uuid::new_v4()),
        );
        match seaquel_storage::query_history::append(self.storage(), &item).await {
            Ok(()) => {
                // History appends emit too, with
                // the running window's origin.
                self.record_storage_write(
                    origin,
                    crate::StoredKind::History,
                    Some(item.connection_id.clone()),
                    Some(vec![item.id.clone()]),
                );
                Some(item)
            }
            Err(e) => {
                warn!(activity = "db.run", code = e.code(); "Recording the run in history failed");
                None
            }
        }
    }

    /// No storage in this build: nothing is recorded.
    #[cfg(not(feature = "storage"))]
    async fn append_history(
        &self,
        _ctx: &HistoryContext,
        _query: &str,
        _outcome: &Outcome,
        _executor: &dyn Executor,
        _origin: &crate::WriteOrigin,
    ) -> Option<seaquel_types::storage::PersistedQueryHistoryItem> {
        let _ = history_item;
        None
    }
}

pub(crate) fn no_executor() -> RunEvent {
    RunEvent::error(
        NOT_SUPPORTED,
        "Running queries isn't enabled here (no executor is set)",
    )
}

pub(crate) fn no_sql_rules() -> RunEvent {
    RunEvent::error(
        NOT_SUPPORTED,
        "Seaquel has no SQL rules for this connection's engine",
    )
}

/// `fut`, unless `token` is cancelled first (then `None`, and `fut` is
/// dropped, which is what cancels it).
pub(crate) async fn until_cancelled<T>(
    token: &CancellationToken,
    fut: impl std::future::Future<Output = T>,
) -> Option<T> {
    let mut once = std::pin::pin!(futures::stream::once(fut).take_until(token.cancelled()));
    once.next().await
}

/// A count that failed with one of these lost the connection: the page's
/// statement fails with it instead of estimating.
fn connection_lost(code: &str) -> bool {
    matches!(code, "CONNECTION_CLOSED" | "CONNECTION_NOT_FOUND")
}

/// A count query's total: the first cell of the first row as a whole number
/// that isn't negative (`Int`, or `Decimal`/`Text` holding the digits, as a
/// bigint arrives). Anything else is a failed count.
fn count_of(result: &QueryResult) -> Option<u64> {
    match result.rows.first()?.first()? {
        Value::Int(n) => u64::try_from(*n).ok(),
        Value::Decimal(s) | Value::Text(s) => {
            let s = s.trim();
            if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            s.parse().ok()
        }
        _ => None,
    }
}

/// One statement's events after its `statementStart`: `batch`es, then
/// `statementDone` or `statementError`. Ends with neither when `token` is
/// cancelled; the driver's call is dropped before the stream ends.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute<'a>(
    connection: &'a Connection,
    engine: SqlEngine,
    executor: &'a dyn Executor,
    token: &'a CancellationToken,
    index: u32,
    kind: StatementKind,
    source: PageSource,
    page: u32,
    page_size: u32,
) -> BoxStream<'a, RunEvent> {
    debug!(activity = "db.run.statement", index = index, kind = kind_name(kind), sql_len = source.sql.len(), params = source.params.len(); "Statement");
    let driver = connection.driver.clone();
    Box::pin(async_stream::stream! {
        let start = executor.monotonic();
        let error = |e: DbError, end: Duration| RunEvent::StatementError {
            index,
            code: e.code,
            message: e.message,
            elapsed_ms: elapsed_ms(start, end),
            sql: None,
        };
        let PageSource { sql, params } = source;
        match kind {
            StatementKind::Stream => {
                let mut rows = 0u64;
                // The driver's stream is dropped at the end of this block,
                // before any closing event: that's what stops the statement
                // on the server. Keep this loop in step with the one in
                // `Core::query_stream_as`.
                let failure = {
                    let mut batches = std::pin::pin!(driver
                        .query_stream(sql, params, token.clone())
                        .take_until(token.cancelled()));
                    let mut failure = None;
                    while let Some(item) = batches.next().await {
                        if token.is_cancelled() {
                            break;
                        }
                        match item {
                            Ok(batch) => {
                                rows += batch.rows.len() as u64;
                                yield RunEvent::Batch(batch);
                            }
                            Err(e) => {
                                failure = Some(e);
                                break;
                            }
                        }
                    }
                    failure
                };
                if token.is_cancelled() {
                    return;
                }
                let end = executor.monotonic();
                match failure {
                    Some(e) => yield error(e, end),
                    None => yield done(index, elapsed_ms(start, end), rows, 1, false, None, None),
                }
            }
            StatementKind::Page => {
                let dialect = connection.engine.dialect();
                let offset = page_offset(page, page_size, max_page_size());
                let (Some(dialect), Ok(offset)) = (dialect, offset) else {
                    let end = executor.monotonic();
                    yield error(seaquel_engine::not_supported("Paging on this engine"), end);
                    return;
                };
                let size = u64::from(page_size);
                let paged = dialect.paginate(&sql, size + 1, offset);
                let mut columns: Option<Vec<String>> = None;
                let mut rows: Vec<Vec<Value>> = Vec::new();
                // Keep this loop in step with the one in
                // `Core::query_stream_as`. It stops at the row past the page
                // whatever the SQL made of the limit, and dropping the
                // driver's stream at the end of the block stops the rest on
                // the server.
                let fetched = {
                    let mut batches = std::pin::pin!(driver
                        .query_stream(paged, params.clone(), token.clone())
                        .take_until(token.cancelled()));
                    let mut fetched = Ok(());
                    while let Some(item) = batches.next().await {
                        match item {
                            Ok(StreamBatch { columns: c, rows: r, .. }) => {
                                if columns.is_none() {
                                    columns = c;
                                }
                                rows.extend(r);
                                if rows.len() as u64 > size {
                                    break;
                                }
                            }
                            Err(e) => {
                                fetched = Err(e);
                                break;
                            }
                        }
                    }
                    fetched
                };
                if token.is_cancelled() {
                    return;
                }
                if let Err(e) = fetched {
                    let end = executor.monotonic();
                    yield error(e, end);
                    return;
                }
                let mut estimated = false;
                // A full page says there's more; an empty one past the start
                // (a stale page number, rows deleted since) says nothing
                // about the total: both count. Any other page is
                // the last, and its offset plus its rows is the total.
                let full = rows.len() as u64 > size;
                let total = if full || (rows.is_empty() && offset > 0) {
                    rows.truncate(page_size as usize);
                    let counted = until_cancelled(
                        token,
                        driver.query(&count_query(&end_line_comment(&sql, engine), engine), params),
                    )
                    .await;
                    if token.is_cancelled() {
                        return;
                    }
                    let counted = match counted {
                        Some(Ok(result)) => count_of(&result).ok_or("COUNT_NOT_NUMERIC".to_string()),
                        // A lost connection isn't a count to estimate: the
                        // statement fails with it, so the client sees the
                        // connection is gone.
                        Some(Err(e)) if connection_lost(&e.code) => {
                            let end = executor.monotonic();
                            yield error(e, end);
                            return;
                        }
                        Some(Err(e)) => Err(e.code),
                        None => return,
                    };
                    match counted {
                        Ok(total) => total,
                        Err(code) => {
                            warn!(activity = "db.run.count", code = code.as_str(); "Row count failed; estimating");
                            estimated = true;
                            if full {
                                offset.saturating_add(size).saturating_add(1)
                            } else {
                                // At most `offset` rows: the page before is
                                // the last one there can be.
                                offset
                            }
                        }
                    }
                } else {
                    offset.saturating_add(rows.len() as u64)
                };
                let pages = if size == 0 { 1 } else { total.div_ceil(size).max(1) };
                let end = executor.monotonic();
                yield RunEvent::Batch(StreamBatch {
                    columns: Some(columns.unwrap_or_default()),
                    rows,
                    is_final: true,
                    truncated: false,
                });
                yield done(
                    index,
                    elapsed_ms(start, end),
                    total,
                    u32::try_from(pages).unwrap_or(u32::MAX),
                    estimated,
                    None,
                    None,
                );
            }
            StatementKind::Write => {
                let result = until_cancelled(token, driver.execute(&sql, params)).await;
                if token.is_cancelled() {
                    return;
                }
                let end = executor.monotonic();
                match result {
                    Some(Ok(r)) => yield done(index, elapsed_ms(start, end), 0, 1, false, Some(r.rows_affected), r.last_insert_id),
                    Some(Err(e)) => yield error(e, end),
                    None => {}
                }
            }
            StatementKind::Utility => {
                let result = until_cancelled(token, driver.query(&sql, params)).await;
                if token.is_cancelled() {
                    return;
                }
                let end = executor.monotonic();
                match result {
                    // Rows when it returned columns.
                    Some(Ok(QueryResult { columns, rows }))
                        if !columns.is_empty() && !is_status(engine, &sql, &columns, &rows) =>
                    {
                        let n = rows.len() as u64;
                        yield RunEvent::Batch(StreamBatch {
                            columns: Some(columns),
                            rows,
                            is_final: true,
                            truncated: false,
                        });
                        yield done(index, elapsed_ms(start, end), n, 1, false, None, None);
                    }
                    Some(Ok(_)) => yield done(index, elapsed_ms(start, end), 0, 1, false, None, None),
                    Some(Err(e)) => yield error(e, end),
                    None => {}
                }
            }
        }
    })
}

/// `sql` with a newline after a trailing line comment, so the count query
/// that wraps it in `( … )` keeps its closing parenthesis (seaquel-sql's
/// `count_query` wraps the text as it is, as its frozen fixtures pin).
fn end_line_comment(sql: &str, engine: SqlEngine) -> std::borrow::Cow<'_, str> {
    let ends_in_line_comment = scan(sql, engine, ScanOptions::default())
        .last()
        .is_some_and(|t| {
            let text = t.text(sql);
            t.kind == TokenKind::Comment
                && t.end == sql.len()
                && (text.starts_with("--") || text.starts_with('#'))
                && !text.ends_with('\n')
        });
    if ends_in_line_comment {
        std::borrow::Cow::Owned(format!("{sql}\n"))
    } else {
        std::borrow::Cow::Borrowed(sql)
    }
}

/// Statements that start with these return a query's rows on DuckDB, so a
/// `Success` or `Count` column there is a real result. `PRAGMA` isn't one:
/// a setting (`PRAGMA threads = 2`) answers `Success`, and a PRAGMA that
/// lists something has other columns, which the shape check keeps.
const DUCKDB_QUERY_WORDS: [&str; 12] = [
    "WITH",
    "FROM",
    "VALUES",
    "TABLE",
    "SELECT",
    "SHOW",
    "DESCRIBE",
    "SUMMARIZE",
    "EXPLAIN",
    "CALL",
    "PIVOT",
    "UNPIVOT",
];

/// DuckDB answers a statement that returns no rows (`SET`, `CREATE`,
/// `ATTACH`, `CHECKPOINT`, …) with one status column: `Success` with no
/// rows, or `Count` with no rows (`CREATE TABLE`, `CREATE VIEW`) or one
/// integer row (the rows `CREATE TABLE … AS` wrote).
/// That is no result to show: such a statement stays a hidden utility
/// result, as on the other engines, whose drivers return no columns for
/// it. duckdb-rs doesn't expose the statement's result type, so this reads
/// the answer's shape, and never for a statement that starts like a query
/// (`WITH … SELECT count(*) AS Count` keeps its row).
fn is_status(engine: SqlEngine, sql: &str, columns: &[String], rows: &[Vec<Value>]) -> bool {
    if engine != SqlEngine::Duckdb {
        return false;
    }
    let status = match (columns, rows) {
        ([only], []) => only == "Success" || only == "Count",
        ([only], [row]) => only == "Count" && matches!(row.as_slice(), [Value::Int(_)]),
        _ => false,
    };
    if !status {
        return false;
    }
    match tokens(sql, engine).first() {
        Some(t) if t.kind == TokenKind::Word => {
            let word = t.text(sql).to_ascii_uppercase();
            !DUCKDB_QUERY_WORDS.contains(&word.as_str())
        }
        _ => false,
    }
}

fn done(
    index: u32,
    elapsed_ms: f64,
    total_rows: u64,
    total_pages: u32,
    count_estimated: bool,
    rows_affected: Option<u64>,
    last_insert_id: Option<i64>,
) -> RunEvent {
    RunEvent::StatementDone {
        index,
        elapsed_ms,
        total_rows,
        total_pages,
        count_estimated,
        rows_affected,
        last_insert_id,
    }
}
