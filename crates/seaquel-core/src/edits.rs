//! The grid's edits and the data tab (phase 5c): [`Workspace::plan_edits`],
//! [`Workspace::apply_changes`], [`Workspace::table_page`] and
//! [`Workspace::duckdb_extension`].
//!
//! Planning is `seaquel_workspace::edits` (pure); here it gets the table
//! metadata it needs, read from the database once per table per call
//! (Decision 3), and is carried out on one of the workspace's connections.
//! Everything is validated (limits, metadata, keys, one statement per typed
//! change, the destructive check) before the first statement runs, so a
//! refusal applies nothing. A DML-only batch runs in one transaction
//! (`Driver::transaction`); a batch with anything else runs in order and
//! stops at the first failure (Decision 5).
//!
//! The calls are unary: dropping the future drops the driver call in
//! flight, which rolls an open transaction back. An atomic apply dropped
//! midway therefore leaves nothing. An in-order (or single) apply dropped
//! midway leaves the statements that already ran committed (each runs on
//! its own), the one in flight to the database's cancel, and no history
//! row and no outcome for any of them: the caller never learns which ran,
//! and must reload what it shows (phase 5c plan, Task 6). `table_page` is a stream
//! registered like a run's, so `Workspace::cancel`, `disconnect` and
//! `close_all` reach it. Nothing here logs SQL, keys or values: activity
//! names, ids, counts, modes and codes only (Decision 18).

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use futures::StreamExt;
use log::{debug, info};
use seaquel_engine::{
    BatchStatement, BoxStream, DbError, Dialect, ExpectRows, QueryResult, TransactionError,
};
use seaquel_runtime::Executor;
use seaquel_sql::statements::{destructive_reason, QueryType, TableRef};
use seaquel_sql::SqlEngine;
use seaquel_types::storage::PersistedQueryHistoryItem;
use seaquel_types::Value;
use seaquel_workspace::edits::{
    check_change_limits, check_edit_limits, check_query_limits, classify, extension_statements,
    is_dml, plan_edit, plan_sql, table_select, ApplyChangesParams, ApplyFailure, ApplyMode,
    ApplyOutcome, Change, ChangeResult, Edit, ExtensionAction, PlanEditsParams, PlannedChange,
    TableMeta, TablePageParams, TableTarget, MAX_DESTRUCTIVE_LISTED, NO_ROWS_AFFECTED,
};
use seaquel_workspace::run::{
    history_item, page_offset, DestructiveStatement, HistoryContext, PlanError, RunEvent,
    StatementKind, INVALID_ARGUMENT,
};

use crate::run::{
    closed_event, elapsed_ms, execute, max_page_size, no_executor, no_sql_rules, one,
    until_cancelled, NOT_SUPPORTED,
};
use crate::{Connection, ConnectionHandle, Core, CoreError, Workspace, WorkspaceId};

fn core_error(e: DbError) -> CoreError {
    CoreError::new(e.code, e.message)
}

fn plan_error(e: PlanError) -> CoreError {
    CoreError::new(e.code, e.message)
}

fn not_supported(what: &str) -> CoreError {
    core_error(seaquel_engine::not_supported(what))
}

/// A connection's SQL rules and dialect, or `NOT_SUPPORTED`.
fn rules(connection: &Connection) -> Result<(SqlEngine, &dyn Dialect), CoreError> {
    let engine = connection
        .sql_engine
        .ok_or_else(|| not_supported("Editing on this engine"))?;
    let dialect = connection
        .engine
        .dialect()
        .ok_or_else(|| not_supported("The Rust SQL dialect"))?;
    Ok((engine, dialect))
}

fn mode_name(mode: ApplyMode) -> &'static str {
    match mode {
        ApplyMode::Single => "single",
        ApplyMode::Atomic => "atomic",
        ApplyMode::InOrder => "inOrder",
    }
}

fn index_u32(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

/// The metadata one edit call has read, by table: each table once
/// (Decision 3), a failed read kept as its error.
struct Metadata<'a> {
    handle: ConnectionHandle<'a>,
    tables: HashMap<TableTarget, Result<TableMeta, DbError>>,
}

impl<'a> Metadata<'a> {
    fn new(core: &'a Core, connection_id: &str, owner: WorkspaceId) -> Self {
        Self {
            handle: core.connection_handle_as(connection_id, Some(owner)),
            tables: HashMap::new(),
        }
    }

    /// `target`'s metadata, read now if this call hasn't yet.
    async fn get(&mut self, target: &TableTarget) -> Result<&TableMeta, DbError> {
        if !self.tables.contains_key(target) {
            let read = self
                .handle
                .table_metadata(&target.schema, &target.table)
                .await
                .map(|(columns, _)| TableMeta { columns });
            if let Err(e) = &read {
                debug!(activity = "db.edits.metadata", code = e.code.as_str(); "Reading a table's metadata failed");
            }
            self.tables.insert(target.clone(), read);
        }
        match self.tables.get(target) {
            Some(Ok(meta)) => Ok(meta),
            Some(Err(e)) => Err(DbError {
                code: e.code.clone(),
                message: e.message.clone(),
            }),
            None => Err(DbError::query_error("metadata missing")),
        }
    }

    /// How many tables this call read.
    fn reads(&self) -> usize {
        self.tables.len()
    }
}

/// Plan `edit` with its table's metadata, read through `metadata`.
async fn plan_one(
    edit: &Edit,
    metadata: &mut Metadata<'_>,
    dialect: &dyn Dialect,
    engine: SqlEngine,
) -> Result<PlannedChange, PlanError> {
    let meta = match edit.metadata_target() {
        Some(target) => Some(metadata.get(target).await.map_err(|e| PlanError {
            code: e.code,
            message: e.message,
        })?),
        None => None,
    };
    plan_edit(edit, meta, dialect, engine)
}

/// A change planned for an apply.
struct Ready {
    id: String,
    planned: PlannedChange,
    /// A keyed edit: it must affect a row (Decision 4).
    keyed: bool,
}

/// The `Applied` outcome of an apply that stopped before anything ran, at
/// change `index`.
fn refused(mode: ApplyMode, index: usize, id: &str, e: PlanError) -> ApplyOutcome {
    ApplyOutcome::Applied {
        mode,
        applied: 0,
        results: Vec::new(),
        failed: Some(ApplyFailure {
            id: Some(id.to_string()),
            index: Some(index_u32(index)),
            code: e.code,
            message: e.message,
        }),
        ddl: false,
        history: Vec::new(),
    }
}

/// The failure of a keyed edit that affected no row, run on its own.
fn no_row_matched(index: usize, id: &str) -> ApplyFailure {
    ApplyFailure {
        id: Some(id.to_string()),
        index: Some(index_u32(index)),
        code: NO_ROWS_AFFECTED.to_string(),
        message: format!(
            "Change {} matched no row. It may have been changed or deleted since it was \
             loaded; refresh and try again.",
            index + 1
        ),
    }
}

/// The error of an apply the workspace's `close_all` stopped: an atomic
/// one (`index` `None`) had its transaction dropped, which rolls it back
/// unless its commit raced the close; an in-order one stopped at change
/// `index`, which may or may not have run. The GUI treats the atomic one
/// as interrupted.
fn closed_mid_apply(index: Option<usize>) -> DbError {
    let message = match index {
        None => "The server closed this workspace during the apply, so its changes may \
                 not have been saved. Reload and check the data before applying again."
            .to_string(),
        Some(i) => format!(
            "The server closed this workspace during the apply, at change {}. \
             The changes before it were applied; that one may not have been.",
            i + 1
        ),
    };
    DbError {
        code: crate::WORKSPACE_CLOSED.to_string(),
        message,
    }
}

/// What an applied change came to, for its history row.
#[cfg_attr(not(feature = "storage"), allow(dead_code))]
struct Ran {
    sql: String,
    /// The values it was bound with, kept in its history row (cleanup
    /// pass B). Never logged.
    params: Vec<Value>,
    rows: u64,
    elapsed_ms: f64,
}

impl Workspace {
    /// Plan edits on one of this workspace's connections (`db.planEdits`):
    /// each edit's SQL and binds, query type, DML flag and summary, for the
    /// pending-changes queue (Decisions 1–3, 12). Reads each table's
    /// metadata once. Runs nothing.
    ///
    /// Errors: `CONNECTION_NOT_FOUND` for a connection this workspace
    /// doesn't own, `INVALID_ARGUMENT` past the interface's [`crate::EditLimits`],
    /// `NOT_SUPPORTED` for an engine without SQL rules or a dialect, a
    /// metadata read's code, and `NOT_EDITABLE` (Decision 4). The first
    /// failing edit fails the call.
    pub async fn plan_edits(
        &self,
        core: &Core,
        params: PlanEditsParams,
    ) -> Result<Vec<PlannedChange>, CoreError> {
        let PlanEditsParams {
            connection_id,
            edits,
        } = params;
        info!(activity = "db.planEdits", connection_id = connection_id.as_str(), edits = edits.len(); "Plan edits");
        let connection = core
            .connection_as(&connection_id, Some(self.id()))
            .map_err(core_error)?;
        check_edit_limits(&edits, core.edit_limits()).map_err(plan_error)?;
        let (engine, dialect) = rules(&connection)?;
        let mut metadata = Metadata::new(core, &connection_id, self.id());
        let mut planned = Vec::with_capacity(edits.len());
        for (index, edit) in edits.iter().enumerate() {
            match plan_one(edit, &mut metadata, dialect, engine).await {
                Ok(p) => planned.push(p),
                Err(e) => {
                    debug!(activity = "db.planEdits", index = index, code = e.code.as_str(); "Edit refused");
                    return Err(plan_error(e));
                }
            }
        }
        debug!(activity = "db.planEdits", edits = planned.len(), metadata_reads = metadata.reads(); "Planned");
        Ok(planned)
    }

    /// Apply the pending-changes queue on one of this workspace's
    /// connections (`db.applyChanges`, Decisions 4, 5, 7 and 8).
    ///
    /// Everything is checked before the first statement runs: the limits,
    /// each table's metadata (read once), each edit's key, each typed
    /// change being one statement, and the destructive check. A refusal is
    /// `Applied` with `applied: 0` and `failed` naming the change; an
    /// unconfirmed batch holding a destructive statement is
    /// `ConfirmRequired`. Then:
    ///
    /// - one change runs through `execute` (`single`);
    /// - two or more, all DML, run in one transaction (`atomic`): all or
    ///   nothing, keyed edits with `expect_rows: {min: 1}`;
    /// - otherwise one by one (`inOrder`), stopping at the first failure.
    ///
    /// A keyed edit that affects no row fails with `NO_ROWS_AFFECTED`. With
    /// `history`, each applied change appends a row (its SQL, rows
    /// affected, Core's time) in one storage transaction; a failed append
    /// is logged with its code and doesn't fail the apply.
    ///
    /// Errors (nothing ran): `CONNECTION_NOT_FOUND`, `INVALID_ARGUMENT`
    /// past the limits, `NOT_SUPPORTED` (no executor, no SQL rules or
    /// dialect).
    pub async fn apply_changes(
        &self,
        core: &Core,
        params: ApplyChangesParams,
    ) -> Result<ApplyOutcome, CoreError> {
        self.apply_changes_from(core, params, &crate::WriteOrigin::none())
            .await
    }

    /// [`Workspace::apply_changes`] for the window or tab `origin`: the
    /// history rows' `StorageChanged` event carries it (phase 5d, Decision
    /// 18).
    pub async fn apply_changes_from(
        &self,
        core: &Core,
        params: ApplyChangesParams,
        origin: &crate::WriteOrigin,
    ) -> Result<ApplyOutcome, CoreError> {
        let ApplyChangesParams {
            connection_id,
            changes,
            confirmed,
            history,
        } = params;
        info!(activity = "db.applyChanges", connection_id = connection_id.as_str(), changes = changes.len(), confirmed = confirmed, history = history.is_some(); "Apply changes");
        let owner = self.id();
        let connection = core
            .connection_as(&connection_id, Some(owner))
            .map_err(core_error)?;
        check_change_limits(&changes, core.edit_limits()).map_err(plan_error)?;
        let executor = core
            .executor
            .clone()
            .ok_or_else(|| not_supported("Applying changes without an executor"))?;
        let (engine, dialect) = rules(&connection)?;
        let dml: Vec<bool> = changes.iter().map(|c| is_dml(c, engine)).collect();
        let mode = classify(&dml);

        // Validation, before anything runs.
        let mut metadata = Metadata::new(core, &connection_id, owner);
        let mut ready = Vec::with_capacity(changes.len());
        for (index, change) in changes.iter().enumerate() {
            let (planned, keyed) = match change {
                Change::Edit { edit, .. } => (
                    plan_one(edit, &mut metadata, dialect, engine).await,
                    edit.is_keyed(),
                ),
                Change::Sql { sql, params, .. } => (plan_sql(sql, params, engine), false),
            };
            match planned {
                Ok(planned) => ready.push(Ready {
                    id: change.id().to_string(),
                    planned,
                    keyed,
                }),
                Err(e) => {
                    info!(activity = "db.applyChanges", index = index, code = e.code.as_str(), mode = mode_name(mode); "Apply refused before anything ran");
                    return Ok(refused(mode, index, change.id(), e));
                }
            }
        }
        let destructive: Vec<DestructiveStatement> = ready
            .iter()
            .enumerate()
            .filter_map(|(index, r)| {
                destructive_reason(&r.planned.sql, engine).map(|reason| DestructiveStatement {
                    index: index_u32(index),
                    sql: r.planned.sql.clone(),
                    reason,
                })
            })
            .collect();
        debug!(activity = "db.applyChanges", mode = mode_name(mode), destructive = destructive.len(), metadata_reads = metadata.reads(); "Validated");
        if !destructive.is_empty() && !confirmed {
            let total = index_u32(destructive.len());
            let mut destructive = destructive;
            destructive.truncate(MAX_DESTRUCTIVE_LISTED);
            return Ok(ApplyOutcome::ConfirmRequired {
                destructive,
                destructive_total: total,
            });
        }

        let handle = core.connection_handle_as(&connection_id, Some(owner));
        let mut ran: Vec<Ran> = Vec::new();
        let mut results = Vec::new();
        let mut failed = None;
        let mut ddl = false;
        match mode {
            ApplyMode::Atomic => {
                let start = executor.monotonic();
                let statements = ready
                    .iter()
                    .map(|r| BatchStatement {
                        sql: r.planned.sql.clone(),
                        params: r.planned.params.clone(),
                        expect_rows: r.keyed.then_some(ExpectRows { min: 1 }),
                    })
                    .collect();
                // Raced against `close_all` (probe M1): dropping the
                // transaction rolls it back.
                match until_cancelled(self.closing(), handle.transaction(statements))
                    .await
                    .unwrap_or_else(|| Err(TransactionError::from(closed_mid_apply(None))))
                {
                    Ok(counts) => {
                        let elapsed = elapsed_ms(start, executor.monotonic());
                        for (r, rows) in ready
                            .iter()
                            .zip(counts.into_iter().chain(std::iter::repeat(0)))
                        {
                            ran.push(Ran {
                                sql: r.planned.sql.clone(),
                                params: r.planned.params.clone(),
                                rows,
                                elapsed_ms: elapsed,
                            });
                        }
                    }
                    Err(e) => {
                        failed = Some(ApplyFailure {
                            id: e.index.and_then(|i| ready.get(i)).map(|r| r.id.clone()),
                            index: e.index.map(index_u32),
                            code: e.error.code,
                            message: e.error.message,
                        });
                    }
                }
            }
            ApplyMode::Single | ApplyMode::InOrder => {
                for (index, r) in ready.iter().enumerate() {
                    let start = executor.monotonic();
                    match until_cancelled(
                        self.closing(),
                        handle.execute(&r.planned.sql, r.planned.params.clone()),
                    )
                    .await
                    .unwrap_or_else(|| Err(closed_mid_apply(Some(index))))
                    {
                        Ok(result) if r.keyed && result.rows_affected == 0 => {
                            failed = Some(no_row_matched(index, &r.id));
                        }
                        Ok(result) => {
                            ddl |= !r.planned.dml;
                            ran.push(Ran {
                                sql: r.planned.sql.clone(),
                                params: r.planned.params.clone(),
                                rows: result.rows_affected,
                                elapsed_ms: elapsed_ms(start, executor.monotonic()),
                            });
                            results.push(ChangeResult {
                                id: r.id.clone(),
                                rows_affected: result.rows_affected,
                                last_insert_id: result.last_insert_id,
                            });
                        }
                        Err(e) => {
                            failed = Some(ApplyFailure {
                                id: Some(r.id.clone()),
                                index: Some(index_u32(index)),
                                code: e.code,
                                message: e.message,
                            });
                        }
                    }
                    if failed.is_some() {
                        break;
                    }
                }
            }
        }
        let applied = index_u32(ran.len());
        info!(activity = "db.applyChanges", mode = mode_name(mode), applied = applied, failed = failed.as_ref().map(|f| f.code.as_str()), failed_index = failed.as_ref().and_then(|f| f.index); "Apply done");
        let history = match history {
            Some(ctx) if !ran.is_empty() => {
                self.append_edit_history(&ctx, &ran, &*executor, origin)
                    .await
            }
            _ => Vec::new(),
        };
        Ok(ApplyOutcome::Applied {
            mode,
            applied,
            results,
            failed,
            ddl,
            history,
        })
    }

    /// Append one history row per applied change, in one storage
    /// transaction. Empty (logged with its code) when the append fails.
    #[cfg(feature = "storage")]
    async fn append_edit_history(
        &self,
        ctx: &HistoryContext,
        ran: &[Ran],
        executor: &dyn Executor,
        origin: &crate::WriteOrigin,
    ) -> Vec<PersistedQueryHistoryItem> {
        let now = executor.unix_time();
        let items: Vec<PersistedQueryHistoryItem> = ran
            .iter()
            .map(|r| PersistedQueryHistoryItem {
                params: (!r.params.is_empty()).then(|| r.params.clone()),
                ..history_item(
                    ctx,
                    &r.sql,
                    r.elapsed_ms,
                    r.rows,
                    now,
                    format!("hist-{}", uuid::Uuid::new_v4()),
                )
            })
            .collect();
        match seaquel_storage::query_history::append_many(self.storage(), &items).await {
            Ok(()) => {
                // Phase 5d, Decision 16: one event for the batch, with the
                // applying window's origin.
                self.record_storage_write(
                    origin,
                    crate::StoredKind::History,
                    Some(ctx.connection_id.clone()),
                    Some(items.iter().map(|i| i.id.clone()).collect()),
                );
                items
            }
            Err(e) => {
                log::warn!(activity = "db.applyChanges", code = e.code(); "Recording the apply in history failed");
                Vec::new()
            }
        }
    }

    /// No storage in this build: nothing is recorded.
    #[cfg(not(feature = "storage"))]
    async fn append_edit_history(
        &self,
        _ctx: &HistoryContext,
        _ran: &[Ran],
        _executor: &dyn Executor,
        _origin: &crate::WriteOrigin,
    ) -> Vec<PersistedQueryHistoryItem> {
        let _ = history_item;
        Vec::new()
    }

    /// One page of a data tab (`db.tablePage`, Decision 9), under
    /// `params.stream_id` in this workspace's scope: Core builds the SELECT
    /// from the typed query (on SQL Server after reading the table's column
    /// types), then pages it as a run's SELECT: `pageSize + 1` rows at the
    /// page's offset, and the count (`count_query` over the SELECT) only
    /// when the page came back full or empty past the first page, estimated
    /// and flagged when it fails.
    ///
    /// Events: `statementStart` (`kind: page`, the built SQL and binds as
    /// its source), a `batch`, `statementDone` or `statementError`, then
    /// `done`; or one `error` for a refusal (`CONNECTION_NOT_FOUND`,
    /// `INVALID_ARGUMENT` for page 0, a page size of 0 or past the cap, the
    /// interface's limits or an empty `IN` list; `NOT_SUPPORTED`; a
    /// metadata read's code). Registered, cancelled and closed like
    /// [`Workspace::page`]; never records history.
    pub fn table_page<'a>(
        &'a self,
        core: &'a Core,
        params: TablePageParams,
    ) -> BoxStream<'a, RunEvent> {
        let TablePageParams {
            connection_id,
            stream_id,
            query,
            page,
            page_size,
        } = params;
        info!(activity = "db.tablePage", connection_id = connection_id.as_str(), stream_id = stream_id.as_str(), filters = query.filters.len(), sort = query.sort.len(), page = page, page_size = page_size; "Table page");
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
            let Some(dialect) = connection.engine.dialect() else {
                yield RunEvent::error(NOT_SUPPORTED, "Paging on this engine isn't supported");
                return;
            };
            let checked = if page_size == 0 {
                Err(PlanError { code: INVALID_ARGUMENT.to_string(), message: "The page size must be at least 1 row".to_string() })
            } else {
                page_offset(page, page_size, max_page_size())
                    .and_then(|_| check_query_limits(&query, core.edit_limits()))
            };
            if let Err(e) = checked {
                yield RunEvent::error(e.code, e.message);
                return;
            }
            // SQL Server lists a table's columns when tiberius can't read
            // one of them, which needs their types.
            let columns = if engine == SqlEngine::Mssql {
                let read = until_cancelled(
                    &token,
                    connection.driver.table_metadata(&query.target.schema, &query.target.table),
                )
                .await;
                match read {
                    _ if token.is_cancelled() => {
                        if closed.load(Ordering::SeqCst) {
                            yield closed_event();
                        }
                        return;
                    }
                    Some(Ok((columns, _))) => Some(columns),
                    Some(Err(e)) => {
                        yield RunEvent::error(e.code, e.message);
                        return;
                    }
                    None => return,
                }
            } else {
                None
            };
            let source = match table_select(&query, columns.as_deref(), dialect, engine, core.edit_limits()) {
                Ok(source) => source,
                Err(e) => {
                    debug!(activity = "db.tablePage", code = e.code.as_str(); "Table page refused");
                    yield RunEvent::error(e.code, e.message);
                    return;
                }
            };
            yield RunEvent::StatementStart {
                index: 0,
                sql: source.sql.clone(),
                source: source.clone(),
                query_type: QueryType::Select,
                kind: StatementKind::Page,
                page,
                page_size,
                table: Some(TableRef {
                    schema: Some(query.target.schema.clone()),
                    table: query.target.table.clone(),
                }),
                column_refs: None,
            };
            let mut failed = false;
            let mut events = execute(&connection, engine, &*executor, &token, 0, StatementKind::Page, source, page, page_size);
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

    /// The DuckDB extensions tab's action (`db.duckdbExtension`, Q9): its
    /// statements run one at a time on one of this workspace's DuckDB
    /// connections. `List` returns `duckdb_extensions()`'s rows; the others
    /// `None`. `INVALID_ARGUMENT` for a name that isn't `^[A-Za-z0-9_]+$`,
    /// `NOT_SUPPORTED` on any other engine, `CONNECTION_NOT_FOUND` for a
    /// connection this workspace doesn't own; a failing statement stops
    /// the rest.
    pub async fn duckdb_extension(
        &self,
        core: &Core,
        connection_id: &str,
        action: ExtensionAction,
    ) -> Result<Option<QueryResult>, CoreError> {
        info!(activity = "db.duckdbExtension", connection_id = connection_id; "DuckDB extension");
        let connection = core
            .connection_as(connection_id, Some(self.id()))
            .map_err(core_error)?;
        if connection.sql_engine != Some(SqlEngine::Duckdb) {
            return Err(not_supported("Extensions on this engine"));
        }
        let statements = extension_statements(&action).map_err(plan_error)?;
        let handle = core.connection_handle_as(connection_id, Some(self.id()));
        let mut last = None;
        for sql in &statements {
            last = Some(handle.query(sql, Vec::new()).await.map_err(core_error)?);
        }
        Ok(match action {
            ExtensionAction::List => last,
            _ => None,
        })
    }
}
