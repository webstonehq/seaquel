//! The grid's edits and the data tab's query, planned (phase 5c).
//!
//! The GUI sends intents ([`Edit`]): a table, a key, a column and a value.
//! [`plan_edit`] turns one into its statement with the connection's
//! [`Dialect`] builders and the table's metadata, which Core reads from the
//! database per call (Decision 3): the Postgres cast map ([`cast_map`]),
//! SQLite's default expression for Set default, the primary key the key
//! must be (Decision 4), and the JSON columns whose values bind as JSON
//! (Decision 19). [`plan_sql`] checks a statement the editor deferred or the
//! table editor generated. [`classify`] decides how a batch applies
//! (Decision 5), and [`table_select`] builds the data tab's SELECT from a
//! typed [`TableQuery`] (Decision 9).
//!
//! Everything here is pure (no I/O), builds for wasm32 and never panics on
//! its input. The rules are pinned by the fixtures in `tests/fixtures/edits`
//! (their README lists where Core is meant to differ from the TypeScript it
//! replaces, in `changes.json`).
//!
//! The wire types are serialised to the GUIs through `seaquel-rpc`. Their
//! `Debug` shows table and column names, counts, modes and codes, never
//! values, keys or SQL (Decision 18).

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use seaquel_engine::crud::{at_placeholder, dollar_placeholder, question_placeholder};
use seaquel_engine::select::{build_table_select, CompareOp, Condition, SelectColumn, TableSelect};
use seaquel_engine::{CastMap, Dialect, RowValues, SchemaColumn};
/// A sort column's direction: `"ASC"` or `"DESC"`, the query builder's.
pub use seaquel_sql::ast::SortDirection;
use seaquel_sql::scan::split_statements;
use seaquel_sql::statements::{change_summary, query_type, ChangeSummary, QueryType};
use seaquel_sql::SqlEngine;
use seaquel_types::storage::PersistedQueryHistoryItem;
use seaquel_types::Value;

use crate::run::{param_bytes, DestructiveStatement, HistoryContext, PageSource, PlanError};
pub use crate::run::{CONFIRM_REQUIRED, INVALID_ARGUMENT, MAX_DESTRUCTIVE_LISTED};

/// An edit Core won't build: the table isn't there or has no primary key,
/// or the key sent isn't the table's primary key (Decision 4).
pub const NOT_EDITABLE: &str = "NOT_EDITABLE";
/// A keyed edit (update, Set default, delete) that matched no row: its key
/// went stale (Decision 4). The same code as a transaction's
/// `expect_rows` shortfall.
pub const NO_ROWS_AFFECTED: &str = "NO_ROWS_AFFECTED";

fn error(code: &str, message: impl Into<String>) -> PlanError {
    PlanError {
        code: code.to_string(),
        message: message.into(),
    }
}

// ── The wire ──

/// A table (or view) as the GUI's schema cache lists it: DuckDB's attached
/// catalogs as `catalog.schema`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TableTarget {
    pub schema: String,
    pub table: String,
}

/// What the sidebar drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ObjectKind {
    Table,
    View,
    MaterializedView,
}

/// An edit intent (Decision 1). Keys are `[column, value]` pairs picked out
/// of the row by the GUI's routing, in the order it has them; values are in
/// the cell wire format. `Debug` shows the table, the column and counts.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Edit {
    UpdateCell {
        target: TableTarget,
        #[cfg_attr(feature = "ts", ts(type = "Array<[string, unknown]>"))]
        key: RowValues,
        column: String,
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        value: Value,
    },
    SetDefault {
        target: TableTarget,
        #[cfg_attr(feature = "ts", ts(type = "Array<[string, unknown]>"))]
        key: RowValues,
        column: String,
    },
    InsertRow {
        target: TableTarget,
        /// In the grid's column order.
        #[cfg_attr(feature = "ts", ts(type = "Array<[string, unknown]>"))]
        values: RowValues,
    },
    DeleteRow {
        target: TableTarget,
        #[cfg_attr(feature = "ts", ts(type = "Array<[string, unknown]>"))]
        key: RowValues,
    },
    /// The sidebar's TRUNCATE (SQLite: `DELETE FROM`, which counts as DML).
    TruncateTable { target: TableTarget },
    /// The sidebar's DROP.
    DropObject {
        target: TableTarget,
        kind: ObjectKind,
    },
}

impl Edit {
    pub fn target(&self) -> &TableTarget {
        match self {
            Edit::UpdateCell { target, .. }
            | Edit::SetDefault { target, .. }
            | Edit::InsertRow { target, .. }
            | Edit::DeleteRow { target, .. }
            | Edit::TruncateTable { target }
            | Edit::DropObject { target, .. } => target,
        }
    }

    /// The table whose metadata [`plan_edit`] needs: every grid edit's.
    /// The sidebar's TRUNCATE and DROP need none.
    pub fn metadata_target(&self) -> Option<&TableTarget> {
        match self {
            Edit::TruncateTable { .. } | Edit::DropObject { .. } => None,
            other => Some(other.target()),
        }
    }

    /// An update, Set default or delete by key: it must affect a row
    /// (Decision 4).
    pub fn is_keyed(&self) -> bool {
        matches!(
            self,
            Edit::UpdateCell { .. } | Edit::SetDefault { .. } | Edit::DeleteRow { .. }
        )
    }

    fn kind(&self) -> &'static str {
        match self {
            Edit::UpdateCell { .. } => "updateCell",
            Edit::SetDefault { .. } => "setDefault",
            Edit::InsertRow { .. } => "insertRow",
            Edit::DeleteRow { .. } => "deleteRow",
            Edit::TruncateTable { .. } => "truncateTable",
            Edit::DropObject { .. } => "dropObject",
        }
    }

    /// Every value the edit carries: keys, the new value, an insert's
    /// values.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        let (pairs, value): (&[(String, Value)], Option<&Value>) = match self {
            Edit::UpdateCell { key, value, .. } => (key, Some(value)),
            Edit::SetDefault { key, .. } | Edit::DeleteRow { key, .. } => (key, None),
            Edit::InsertRow { values, .. } => (values, None),
            Edit::TruncateTable { .. } | Edit::DropObject { .. } => (&[], None),
        };
        pairs.iter().map(|(_, v)| v).chain(value)
    }
}

impl fmt::Debug for Edit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct(self.kind());
        s.field("target", self.target());
        match self {
            Edit::UpdateCell { key, column, .. } | Edit::SetDefault { key, column, .. } => {
                s.field("key_columns", &key.len()).field("column", column);
            }
            Edit::DeleteRow { key, .. } => {
                s.field("key_columns", &key.len());
            }
            Edit::InsertRow { values, .. } => {
                s.field("values", &values.len());
            }
            Edit::TruncateTable { .. } => {}
            Edit::DropObject { kind, .. } => {
                s.field("kind", kind);
            }
        }
        s.finish_non_exhaustive()
    }
}

/// A pending-changes queue entry as `db.applyChanges` takes it back
/// (Decision 2). `id` is the GUI's. `Debug` shows no SQL or values.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Change {
    Edit {
        id: String,
        edit: Edit,
    },
    /// A statement the editor deferred, or one the table editor generated.
    Sql {
        id: String,
        sql: String,
        #[serde(default)]
        #[cfg_attr(feature = "ts", ts(type = "Array<unknown>"))]
        params: Vec<Value>,
    },
}

impl Change {
    pub fn id(&self) -> &str {
        match self {
            Change::Edit { id, .. } | Change::Sql { id, .. } => id,
        }
    }
}

impl fmt::Debug for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Change::Edit { id, edit } => f
                .debug_struct("Edit")
                .field("id", id)
                .field("edit", edit)
                .finish(),
            Change::Sql { id, sql, params } => f
                .debug_struct("Sql")
                .field("id", id)
                .field("sql_len", &sql.len())
                .field("params", &params.len())
                .finish(),
        }
    }
}

/// `db.planEdits`: the queue entries' display fields for these edits.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PlanEditsParams {
    pub connection_id: String,
    pub edits: Vec<Edit>,
}

impl fmt::Debug for PlanEditsParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlanEditsParams")
            .field("connection_id", &self.connection_id)
            .field("edits", &self.edits.len())
            .finish()
    }
}

/// One planned change: its SQL and binds (for display; Core builds again
/// at apply time), its query type, whether it counts as DML (Decision 5)
/// and what it does (Decision 12). `Debug` shows no SQL or values.
#[derive(Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PlannedChange {
    pub sql: String,
    #[cfg_attr(feature = "ts", ts(type = "Array<unknown>"))]
    pub params: Vec<Value>,
    pub query_type: QueryType,
    pub dml: bool,
    /// Absent when the statement is none of `ChangeVerb`'s.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub summary: Option<ChangeSummary>,
}

impl fmt::Debug for PlannedChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlannedChange")
            .field("sql_len", &self.sql.len())
            .field("params", &self.params.len())
            .field("query_type", &self.query_type)
            .field("dml", &self.dml)
            .field("verb", &self.summary.as_ref().map(|s| s.verb))
            .finish()
    }
}

/// `db.applyChanges`: the queue, in order.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ApplyChangesParams {
    pub connection_id: String,
    pub changes: Vec<Change>,
    /// The user confirmed the destructive statements (Decision 7). Absent
    /// is false.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub confirmed: bool,
    /// Record each applied change in history under this saved connection
    /// (Decision 8). Without it nothing is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub history: Option<HistoryContext>,
}

impl fmt::Debug for ApplyChangesParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApplyChangesParams")
            .field("connection_id", &self.connection_id)
            .field("changes", &self.changes.len())
            .field("confirmed", &self.confirmed)
            .field("history", &self.history.is_some())
            .finish()
    }
}

/// How a batch applies (Decision 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ApplyMode {
    /// One change, through `execute`.
    Single,
    /// Two or more, all DML: one transaction, all or nothing.
    Atomic,
    /// Two or more, any not DML: one by one, stopping at the first failure.
    InOrder,
}

/// What `db.applyChanges` did.
#[derive(Clone, Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ApplyOutcome {
    Applied {
        mode: ApplyMode,
        /// Changes that took effect: the queue's prefix to remove.
        applied: u32,
        /// Per applied change, `single` and `inOrder` only (empty for
        /// `atomic`, whose statements run inside the driver).
        results: Vec<ChangeResult>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        failed: Option<ApplyFailure>,
        /// An applied change wasn't DML: reload the schema.
        ddl: bool,
        /// The history rows appended (Decision 8), in queue order.
        history: Vec<PersistedQueryHistoryItem>,
    },
    /// Nothing ran: the batch holds destructive statements and wasn't
    /// confirmed (Decision 7).
    ConfirmRequired {
        /// The first [`MAX_DESTRUCTIVE_LISTED`]; `index` is the change's.
        destructive: Vec<DestructiveStatement>,
        destructive_total: u32,
    },
}

impl fmt::Debug for ApplyOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyOutcome::Applied {
                mode,
                applied,
                results,
                failed,
                ddl,
                history,
            } => f
                .debug_struct("Applied")
                .field("mode", mode)
                .field("applied", applied)
                .field("results", &results.len())
                .field("failed", failed)
                .field("ddl", ddl)
                .field("history", &history.len())
                .finish(),
            ApplyOutcome::ConfirmRequired {
                destructive,
                destructive_total,
            } => f
                .debug_struct("ConfirmRequired")
                .field("destructive", destructive)
                .field("destructive_total", destructive_total)
                .finish(),
        }
    }
}

/// One applied change's result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChangeResult {
    pub id: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub rows_affected: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
    pub last_insert_id: Option<i64>,
}

/// The change an apply stopped at, or the refusal before anything ran.
/// `Debug` shows no message.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ApplyFailure {
    /// `None` when the driver can't say which statement failed (BEGIN or
    /// COMMIT, a transaction already open).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub index: Option<u32>,
    /// `NO_ROWS_AFFECTED`, `NOT_EDITABLE`, `INVALID_ARGUMENT`,
    /// `EXECUTE_ERROR`, a metadata read's code, …
    pub code: String,
    /// The database's message; Core's own hold no key or row values.
    pub message: String,
}

impl fmt::Debug for ApplyFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApplyFailure")
            .field("id", &self.id)
            .field("index", &self.index)
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

/// `db.tablePage`: one page of a data tab. `Debug` shows the table, counts
/// and the page, never filter values.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TablePageParams {
    pub connection_id: String,
    pub stream_id: String,
    pub query: TableQuery,
    /// 1-based.
    pub page: u32,
    /// 1 to `max_query_rows() - 1`.
    pub page_size: u32,
}

impl fmt::Debug for TablePageParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TablePageParams")
            .field("connection_id", &self.connection_id)
            .field("stream_id", &self.stream_id)
            .field("query", &self.query)
            .field("page", &self.page)
            .field("page_size", &self.page_size)
            .finish()
    }
}

/// The data tab's query: its enabled filters (each with a column), how they
/// join, and its sort.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TableQuery {
    pub target: TableTarget,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub logic: FilterLogic,
    #[serde(default)]
    pub sort: Vec<Sort>,
}

impl fmt::Debug for TableQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableQuery")
            .field("target", &self.target)
            .field("filters", &self.filters.len())
            .field("logic", &self.logic)
            .field("sort", &self.sort.len())
            .finish()
    }
}

/// One filter. `value` is the text the user typed (a comma-separated list
/// for `IN`/`NOT IN`, ignored by `IS [NOT] NULL`). `Debug` shows no value.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Filter {
    pub column: String,
    pub op: FilterOp,
    #[serde(default)]
    pub value: String,
}

impl fmt::Debug for Filter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Filter")
            .field("column", &self.column)
            .field("op", &self.op)
            .field("value_len", &self.value.len())
            .finish()
    }
}

/// A filter's operator, serialised as the data tab's `DataFilterOperator`
/// text. Anything else is refused when the request is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum FilterOp {
    #[serde(rename = "=")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = "LIKE")]
    Like,
    #[serde(rename = "NOT LIKE")]
    NotLike,
    #[serde(rename = "IN")]
    In,
    #[serde(rename = "NOT IN")]
    NotIn,
    #[serde(rename = "IS NULL")]
    IsNull,
    #[serde(rename = "IS NOT NULL")]
    IsNotNull,
}

impl FilterOp {
    fn text(self) -> &'static str {
        match self {
            FilterOp::Eq => "=",
            FilterOp::Ne => "!=",
            FilterOp::Gt => ">",
            FilterOp::Lt => "<",
            FilterOp::Ge => ">=",
            FilterOp::Le => "<=",
            FilterOp::Like => "LIKE",
            FilterOp::NotLike => "NOT LIKE",
            FilterOp::In => "IN",
            FilterOp::NotIn => "NOT IN",
            FilterOp::IsNull => "IS NULL",
            FilterOp::IsNotNull => "IS NOT NULL",
        }
    }
}

/// How filters join.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum FilterLogic {
    #[default]
    #[serde(rename = "AND")]
    And,
    #[serde(rename = "OR")]
    Or,
}

/// One sort column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Sort {
    pub column: String,
    pub direction: SortDirection,
}

// ── Limits ──

/// What one interface lets an edit call carry (Decision 17), set with
/// `CoreBuilder::edit_limits`. The default is no limit: the desktop app,
/// the CLI and the MCP server. The web server sets all seven
/// (`WEB_EDIT_LIMITS`). Every check runs before anything is planned or read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditLimits {
    /// The most changes (apply) or edits (plan) one call may carry.
    pub max_changes: Option<usize>,
    /// The most distinct tables one call's edits may touch: each is one
    /// metadata read (Decision 3). Checked before any is read.
    pub max_tables: Option<usize>,
    /// The most bytes of typed SQL an apply may carry, all changes together.
    pub max_sql_bytes: Option<usize>,
    /// The most bytes of values (keys, cell values, insert values, typed
    /// parameters; counted as [`param_bytes`] counts them) one call may
    /// carry.
    pub max_value_bytes: Option<usize>,
    /// The most filters one table page may have. Its sort columns are held
    /// to the same number.
    pub max_filters: Option<usize>,
    /// The most `IN`/`NOT IN` items one table page may bind, all filters
    /// together.
    pub max_in_values: Option<usize>,
    /// The longest filter value, in bytes.
    pub max_filter_value_bytes: Option<usize>,
}

fn over(limit: Option<usize>, n: usize) -> Option<usize> {
    limit.filter(|&max| n > max)
}

/// Every name an edit puts into SQL: its schema and table, its column, and
/// its key and insert columns.
fn edit_names(edit: &Edit) -> impl Iterator<Item = &str> {
    let target = edit.target();
    let (pairs, column): (&[(String, Value)], Option<&String>) = match edit {
        Edit::UpdateCell { key, column, .. } | Edit::SetDefault { key, column, .. } => {
            (key, Some(column))
        }
        Edit::DeleteRow { key, .. } => (key, None),
        Edit::InsertRow { values, .. } => (values, None),
        Edit::TruncateTable { .. } | Edit::DropObject { .. } => (&[], None),
    };
    [target.schema.as_str(), target.table.as_str()]
        .into_iter()
        .chain(column.map(String::as_str))
        .chain(pairs.iter().map(|(name, _)| name.as_str()))
}

/// `INVALID_ARGUMENT` for a NUL in any of `names` (probe M2). No engine
/// takes one in an identifier: Postgres refuses the byte with a protocol
/// error, and quoting can't make it safe. The message doesn't repeat the
/// name.
fn check_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<(), PlanError> {
    if names.into_iter().any(|n| n.contains('\0')) {
        return Err(error(
            INVALID_ARGUMENT,
            "A schema, table or column name can't contain a NUL character.",
        ));
    }
    Ok(())
}

/// `INVALID_ARGUMENT` for a NUL in any name `edit` carries (probe M2).
pub fn check_edit_names(edit: &Edit) -> Result<(), PlanError> {
    check_names(edit_names(edit))
}

/// `INVALID_ARGUMENT` when `edits` need the metadata of more distinct
/// tables than `limits.max_tables`.
fn check_tables<'a>(
    edits: impl IntoIterator<Item = &'a Edit>,
    limits: EditLimits,
) -> Result<(), PlanError> {
    if let Some(max) = limits.max_tables {
        let tables = metadata_targets(edits).len();
        if tables > max {
            return Err(error(
                INVALID_ARGUMENT,
                format!("One call can edit at most {max} tables; this one edits {tables}."),
            ));
        }
    }
    Ok(())
}

/// `INVALID_ARGUMENT` for more edits, or more bytes of their values, than
/// `limits` allows (`db.planEdits`), and for a NUL in any name
/// ([`check_edit_names`]), whatever the limits.
pub fn check_edit_limits(edits: &[Edit], limits: EditLimits) -> Result<(), PlanError> {
    if let Some(max) = over(limits.max_changes, edits.len()) {
        return Err(error(
            INVALID_ARGUMENT,
            format!("At most {max} edits can be planned at once."),
        ));
    }
    check_tables(edits, limits)?;
    if let Some(max) = limits.max_value_bytes {
        if param_bytes(edits.iter().flat_map(Edit::values)) > max {
            return Err(error(
                INVALID_ARGUMENT,
                format!("The edits' values are larger than {max} bytes."),
            ));
        }
    }
    edits.iter().try_for_each(check_edit_names)
}

/// `INVALID_ARGUMENT` for more changes, more bytes of typed SQL or more
/// bytes of values than `limits` allows (`db.applyChanges`), and for a NUL
/// in any edit's name ([`check_edit_names`]), whatever the limits. Linear
/// in the request, and run before anything is planned.
pub fn check_change_limits(changes: &[Change], limits: EditLimits) -> Result<(), PlanError> {
    if let Some(max) = over(limits.max_changes, changes.len()) {
        return Err(error(
            INVALID_ARGUMENT,
            format!(
                "At most {max} changes can be applied at once; this apply has {}. \
                 Apply them in parts.",
                changes.len()
            ),
        ));
    }
    check_tables(
        changes.iter().filter_map(|c| match c {
            Change::Edit { edit, .. } => Some(edit),
            Change::Sql { .. } => None,
        }),
        limits,
    )?;
    if let Some(max) = limits.max_sql_bytes {
        let bytes = changes
            .iter()
            .map(|c| match c {
                Change::Sql { sql, .. } => sql.len(),
                Change::Edit { .. } => 0,
            })
            .fold(0usize, usize::saturating_add);
        if bytes > max {
            return Err(error(
                INVALID_ARGUMENT,
                format!("The queued statements are longer than {max} bytes. Apply them in parts."),
            ));
        }
    }
    if let Some(max) = limits.max_value_bytes {
        let values = changes
            .iter()
            .flat_map(|c| -> Box<dyn Iterator<Item = &Value>> {
                match c {
                    Change::Edit { edit, .. } => Box::new(edit.values()),
                    Change::Sql { params, .. } => Box::new(params.iter()),
                }
            });
        if param_bytes(values) > max {
            return Err(error(
                INVALID_ARGUMENT,
                format!("The changes' values are larger than {max} bytes. Apply them in parts."),
            ));
        }
    }
    changes.iter().try_for_each(|c| match c {
        Change::Edit { edit, .. } => check_edit_names(edit),
        Change::Sql { .. } => Ok(()),
    })
}

// ── Planning ──

/// A table as Core read it from the database for one edit call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TableMeta {
    pub columns: Vec<SchemaColumn>,
}

/// Without a `castType`, text-like, user-defined and array columns bind
/// without a cast.
const UNCAST_TYPES: [&str; 4] = ["text", "character varying", "user-defined", "array"];

/// The Postgres cast map (`castMapForColumns`): a column's `castType` as it
/// is; without one, its `type`, unless text-like, with `bit` and `character`
/// widened to `bit varying` and `bpchar` (a cast to their unbounded
/// information_schema names truncates to one character). Columns that need
/// no cast are absent. Only Postgres gets one; the types come from the
/// catalog Core read, never from the wire (Decision 3).
pub fn cast_map(columns: &[SchemaColumn]) -> CastMap {
    columns
        .iter()
        .filter_map(|c| {
            if let Some(cast) = c.cast_type.as_deref().filter(|t| !t.is_empty()) {
                return Some((c.name.clone(), cast.to_string()));
            }
            let lower = c.ty.to_lowercase();
            if UNCAST_TYPES.contains(&lower.as_str()) {
                return None;
            }
            let ty = match lower.as_str() {
                "bit" => "bit varying".to_string(),
                "character" => "bpchar".to_string(),
                _ => c.ty.clone(),
            };
            Some((c.name.clone(), ty))
        })
        .collect()
}

/// Whether a column holds JSON, by its metadata type: Postgres `json` and
/// `jsonb`, MySQL/MariaDB `json`, SQLite's declared `JSON`, DuckDB `JSON`.
fn is_json_column(column: &SchemaColumn) -> bool {
    let ty = column.ty.trim();
    ty.eq_ignore_ascii_case("json") || ty.eq_ignore_ascii_case("jsonb")
}

/// A value as JSON, for [`json_value`]: `None` for what JSON can't hold
/// exactly (bytes, a non-finite float, a decimal JSON can't parse).
fn to_json(value: &Value) -> Option<serde_json::Value> {
    Some(match value {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::from(*i),
        Value::Float(f) => serde_json::Value::from(serde_json::Number::from_f64(*f)?),
        Value::Decimal(d) => match serde_json::from_str::<serde_json::Value>(d.trim()) {
            // Integers parse exactly (`u64` past `i64` too); a fraction only
            // when f64 holds it exactly.
            Ok(serde_json::Value::Number(n)) if n.is_i64() || n.is_u64() => n.into(),
            Ok(n @ serde_json::Value::Number(_)) if f64_exact(d) => n,
            _ => return None,
        },
        Value::Text(s) => serde_json::Value::String(s.clone()),
        Value::Json(j) => j.clone(),
        Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(to_json).collect::<Option<_>>()?)
        }
        Value::Bytes(_) => return None,
    })
}

/// Whether decimal text round-trips exactly through f64: at most 15
/// significant digits (`f64::DIGITS`) and an exponent well inside f64's
/// range. `12.50` does; `0.1000000000000000000001` and
/// `12345678901234567.5` don't, and stay decimals.
fn f64_exact(text: &str) -> bool {
    let text = text.trim();
    let text = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (mantissa, exponent) = match text.split_once(['e', 'E']) {
        Some((m, e)) => match e.parse::<i32>() {
            Ok(e) => (m, e),
            Err(_) => return false,
        },
        None => (text, 0),
    };
    if !mantissa.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return false;
    }
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let significant = digits.trim_start_matches('0').trim_end_matches('0');
    significant.len() <= f64::DIGITS as usize && exponent.abs() < 290
}

/// Decision 19: a JSON column's array, number or bool binds as JSON. The
/// providers decode a JSON cell to its plain value and the wire sends an
/// array as a SQL array and a number as a number, which no engine casts to
/// JSON. `Text` stays text (the grid sends typed JSON as a string, which
/// the database parses), and so do `Json`, `NULL` and bytes.
fn json_value(value: Value, json_column: bool) -> Value {
    if !json_column {
        return value;
    }
    match &value {
        Value::Array(_) | Value::Int(_) | Value::Float(_) | Value::Decimal(_) | Value::Bool(_) => {
            to_json(&value).map_or(value, Value::Json)
        }
        _ => value,
    }
}

/// The placeholder and filter text type [`table_select`] uses on `engine`:
/// the engine's `crud.rs` placeholders (Decision 9).
fn select_style(engine: SqlEngine) -> (fn(usize) -> String, &'static str) {
    match engine {
        SqlEngine::Postgres | SqlEngine::Sqlite => (dollar_placeholder, "TEXT"),
        SqlEngine::Mysql | SqlEngine::Mariadb => (question_placeholder, "CHAR"),
        SqlEngine::Duckdb => (question_placeholder, "TEXT"),
        SqlEngine::Mssql => (at_placeholder, "NVARCHAR(MAX)"),
    }
}

fn qualified(target: &TableTarget) -> String {
    format!("{}.{}", target.schema, target.table)
}

/// Plan one edit (Decisions 1, 3, 4, 11 and 19). `meta` is the table's
/// metadata as Core read it this call; the sidebar's TRUNCATE and DROP
/// ([`Edit::metadata_target`] `None`) take none. Pure. Refusals, before
/// anything runs:
///
/// - `INVALID_ARGUMENT`: a NUL in a name ([`check_edit_names`]).
/// - `NOT_EDITABLE`: a grid edit without metadata, or with no columns (the
///   table isn't there); a keyed edit on a table without a primary key, or
///   whose key columns aren't exactly its primary key's (compared as a set;
///   the WHERE follows the key's order as sent).
pub fn plan_edit(
    edit: &Edit,
    meta: Option<&TableMeta>,
    dialect: &dyn Dialect,
    engine: SqlEngine,
) -> Result<PlannedChange, PlanError> {
    check_edit_names(edit)?;
    let target = edit.target();
    let (schema, table) = (target.schema.as_str(), target.table.as_str());
    let qs = |s: &str| dialect.quote_schema(s);
    let qi = |s: &str| dialect.quote_ident(s);
    let built = match edit {
        Edit::TruncateTable { .. } => {
            let name = format!("{}.{}", qs(schema), qi(table));
            let sql = if engine == SqlEngine::Sqlite {
                format!("DELETE FROM {name}")
            } else {
                format!("TRUNCATE TABLE {name}")
            };
            (sql, Vec::new())
        }
        Edit::DropObject { kind, .. } => {
            let what = match kind {
                ObjectKind::Table => "TABLE",
                ObjectKind::View => "VIEW",
                ObjectKind::MaterializedView => "MATERIALIZED VIEW",
            };
            (
                format!("DROP {what} {}.{}", qs(schema), qi(table)),
                Vec::new(),
            )
        }
        _ => {
            let meta = meta.filter(|m| !m.columns.is_empty()).ok_or_else(|| {
                error(
                    NOT_EDITABLE,
                    format!("Table {} wasn't found.", qualified(target)),
                )
            })?;
            check_columns(target, edit, meta)?;
            let json_columns: HashSet<&str> = meta
                .columns
                .iter()
                .filter(|c| is_json_column(c))
                .map(|c| c.name.as_str())
                .collect();
            let convert = |pairs: &RowValues| -> RowValues {
                pairs
                    .iter()
                    .map(|(c, v)| {
                        (
                            c.clone(),
                            json_value(v.clone(), json_columns.contains(c.as_str())),
                        )
                    })
                    .collect()
            };
            let casts = (engine == SqlEngine::Postgres).then(|| cast_map(&meta.columns));
            let casts = casts.as_ref();
            let with_key = |key: &RowValues| -> Result<(Vec<String>, RowValues), PlanError> {
                check_key(target, key, meta)?;
                Ok((key.iter().map(|(c, _)| c.clone()).collect(), convert(key)))
            };
            let sql = match edit {
                Edit::UpdateCell {
                    key, column, value, ..
                } => {
                    let (pks, row) = with_key(key)?;
                    let value = json_value(value.clone(), json_columns.contains(column.as_str()));
                    dialect.build_update(schema, table, column, value, &pks, &row, casts)
                }
                Edit::SetDefault { key, column, .. } => {
                    let (pks, row) = with_key(key)?;
                    // A column without a default is set to NULL (a blank
                    // expression); engines with `DEFAULT` ignore it.
                    let default = meta
                        .columns
                        .iter()
                        .find(|c| &c.name == column)
                        .and_then(|c| c.default_value.as_deref())
                        .unwrap_or("");
                    dialect.build_set_default_expr(
                        schema,
                        table,
                        column,
                        Some(default),
                        &pks,
                        &row,
                        casts,
                    )
                }
                Edit::DeleteRow { key, .. } => {
                    let (pks, row) = with_key(key)?;
                    dialect.build_delete(schema, table, &pks, &row, casts)
                }
                Edit::InsertRow { values, .. } => {
                    dialect.build_insert(schema, table, &convert(values), casts)
                }
                // Built above; never reached.
                Edit::TruncateTable { .. } | Edit::DropObject { .. } => {
                    return Err(error(INVALID_ARGUMENT, "Not a grid edit."))
                }
            };
            (sql.sql, sql.bind_values.unwrap_or_default())
        }
    };
    let (sql, params) = built;
    let query_type = query_type(&sql, engine);
    let dml = match edit {
        Edit::TruncateTable { .. } => engine == SqlEngine::Sqlite,
        Edit::DropObject { .. } => false,
        _ => true,
    };
    Ok(PlannedChange {
        summary: change_summary(&sql, engine),
        sql,
        params,
        query_type,
        dml,
    })
}

/// `NOT_EDITABLE` for an updated, defaulted or inserted column that isn't
/// one of the table's (an exact, case-sensitive match), before anything is
/// built: a case-mismatched column would otherwise reach the database as
/// another name, or (SQLite Set default) be set to NULL for want of its
/// default expression.
fn check_columns(target: &TableTarget, edit: &Edit, meta: &TableMeta) -> Result<(), PlanError> {
    let known = |name: &str| meta.columns.iter().any(|c| c.name == name);
    let missing = match edit {
        Edit::UpdateCell { column, .. } | Edit::SetDefault { column, .. } => {
            (!known(column)).then_some(column.as_str())
        }
        Edit::InsertRow { values, .. } => {
            values.iter().map(|(c, _)| c.as_str()).find(|c| !known(c))
        }
        _ => None,
    };
    match missing {
        Some(column) => Err(error(
            NOT_EDITABLE,
            format!(
                "{} has no column {column:?}. Refresh the schema and try again.",
                qualified(target)
            ),
        )),
        None => Ok(()),
    }
}

/// `NOT_EDITABLE` unless `key`'s columns are exactly the table's primary
/// key's (Decision 4), as a set: each once, none missing, none extra.
fn check_key(target: &TableTarget, key: &RowValues, meta: &TableMeta) -> Result<(), PlanError> {
    let primary: Vec<&str> = meta
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| c.name.as_str())
        .collect();
    if primary.is_empty() {
        return Err(error(
            NOT_EDITABLE,
            format!(
                "{} has no primary key, so its rows can't be edited here.",
                qualified(target)
            ),
        ));
    }
    let sent: HashSet<&str> = key.iter().map(|(c, _)| c.as_str()).collect();
    let wanted: HashSet<&str> = primary.iter().copied().collect();
    if sent.len() != key.len() || sent != wanted {
        return Err(error(
            NOT_EDITABLE,
            format!(
                "The row's key isn't the primary key of {} ({}). Refresh the schema and try again.",
                qualified(target),
                primary.join(", ")
            ),
        ));
    }
    Ok(())
}

/// Plan a typed statement (the editor's deferred statements, the table
/// editor's): it must be exactly one statement, split as the run splits (a
/// MySQL `/*! … */` or MariaDB `/*M! … */` is code), else
/// `INVALID_ARGUMENT` (Decision 5). DML is `insert`, `update` or `delete`
/// by its first word; everything else applies in order.
pub fn plan_sql(
    sql: &str,
    params: &[Value],
    engine: SqlEngine,
) -> Result<PlannedChange, PlanError> {
    if split_statements(sql, engine).len() != 1 {
        return Err(error(
            INVALID_ARGUMENT,
            "A queued change must hold exactly one statement.",
        ));
    }
    let query_type = query_type(sql, engine);
    Ok(PlannedChange {
        sql: sql.to_string(),
        params: params.to_vec(),
        query_type,
        dml: matches!(
            query_type,
            QueryType::Insert | QueryType::Update | QueryType::Delete
        ),
        summary: change_summary(sql, engine),
    })
}

/// How a batch whose changes are DML or not (`dml`, in order) applies
/// (Decision 5): one change is `single`; two or more are `atomic` when all
/// are DML, else `inOrder`.
pub fn classify(dml: &[bool]) -> ApplyMode {
    match dml {
        [] | [_] => ApplyMode::Single,
        _ if dml.iter().all(|d| *d) => ApplyMode::Atomic,
        _ => ApplyMode::InOrder,
    }
}

// ── The data tab ──

/// The four SQL Server types tiberius 0.12 can't read the metadata of: a
/// table holding one is listed column by column with those cast to text.
const TIBERIUS_UNREADABLE: [&str; 4] = ["sql_variant", "geography", "geometry", "hierarchyid"];

fn tiberius_unreadable(ty: &str) -> bool {
    let ty = ty.trim();
    TIBERIUS_UNREADABLE
        .iter()
        .any(|t| t.eq_ignore_ascii_case(ty))
}

/// `INVALID_ARGUMENT` for a table page past `limits`, before anything is
/// built: too many filters or sort columns, a filter value too long, or
/// too many `IN` items. Also, whatever the limits, for a NUL in the
/// schema, the table or a filter or sort column (probe M2).
pub fn check_query_limits(query: &TableQuery, limits: EditLimits) -> Result<(), PlanError> {
    if let Some(max) = over(limits.max_filters, query.filters.len()) {
        return Err(error(
            INVALID_ARGUMENT,
            format!("A data tab can have at most {max} filters."),
        ));
    }
    if let Some(max) = over(limits.max_filters, query.sort.len()) {
        return Err(error(
            INVALID_ARGUMENT,
            format!("A data tab can sort by at most {max} columns."),
        ));
    }
    if let Some(max) = limits.max_filter_value_bytes {
        if let Some(f) = query.filters.iter().find(|f| f.value.len() > max) {
            return Err(error(
                INVALID_ARGUMENT,
                format!("The filter on {} is longer than {max} bytes.", f.column),
            ));
        }
    }
    if let Some(max) = limits.max_in_values {
        let items = query
            .filters
            .iter()
            .filter(|f| matches!(f.op, FilterOp::In | FilterOp::NotIn))
            .map(|f| in_items(&f.value).count())
            .fold(0usize, usize::saturating_add);
        if items > max {
            return Err(error(
                INVALID_ARGUMENT,
                format!("The IN filters can list at most {max} values together."),
            ));
        }
    }
    check_names(
        [query.target.schema.as_str(), query.target.table.as_str()]
            .into_iter()
            .chain(query.filters.iter().map(|f| f.column.as_str()))
            .chain(query.sort.iter().map(|s| s.column.as_str())),
    )
}

/// An `IN`/`NOT IN` filter's items: comma-separated, each trimmed, empty
/// ones skipped (a trailing comma).
fn in_items(value: &str) -> impl Iterator<Item = &str> {
    value.split(',').map(str::trim).filter(|s| !s.is_empty())
}

/// The data tab's SELECT for `query` (Decision 9), with no paging: Core
/// pages and counts it as a run's SELECT. `columns` is the table's
/// metadata, read on SQL Server only: a table with a column tiberius can't
/// read is listed column by column with those cast to `NVARCHAR(MAX)`.
/// Pure. `INVALID_ARGUMENT` past `limits` ([`check_query_limits`]) and for
/// an `IN`/`NOT IN` filter with no items.
pub fn table_select(
    query: &TableQuery,
    columns: Option<&[SchemaColumn]>,
    dialect: &dyn Dialect,
    engine: SqlEngine,
    limits: EditLimits,
) -> Result<PageSource, PlanError> {
    check_query_limits(query, limits)?;
    let mut conditions = Vec::with_capacity(query.filters.len());
    for f in &query.filters {
        let column = f.column.as_str();
        let compare = |op| Condition::Compare {
            column,
            op,
            value: Value::Text(f.value.clone()),
        };
        conditions.push(match f.op {
            FilterOp::Eq => compare(CompareOp::Eq),
            FilterOp::Ne => compare(CompareOp::Ne),
            FilterOp::Gt => compare(CompareOp::Gt),
            FilterOp::Lt => compare(CompareOp::Lt),
            FilterOp::Ge => compare(CompareOp::Ge),
            FilterOp::Le => compare(CompareOp::Le),
            FilterOp::Like => compare(CompareOp::Like),
            FilterOp::NotLike => compare(CompareOp::NotLike),
            FilterOp::IsNull => Condition::IsNull(column),
            FilterOp::IsNotNull => Condition::IsNotNull(column),
            FilterOp::In | FilterOp::NotIn => {
                let values: Vec<Value> = in_items(&f.value)
                    .map(|v| Value::Text(v.to_string()))
                    .collect();
                if values.is_empty() {
                    return Err(error(
                        INVALID_ARGUMENT,
                        format!(
                            "The {} filter on {column} has no values. Separate values with commas.",
                            f.op.text()
                        ),
                    ));
                }
                Condition::In {
                    column,
                    negated: f.op == FilterOp::NotIn,
                    values,
                }
            }
        });
    }
    let listed = columns
        .filter(|c| c.iter().any(|c| tiberius_unreadable(&c.ty)))
        .map(|columns| {
            columns
                .iter()
                .map(|c| SelectColumn {
                    name: &c.name,
                    cast: tiberius_unreadable(&c.ty),
                })
                .collect()
        });
    let select = TableSelect {
        schema: &query.target.schema,
        table: &query.target.table,
        columns: if engine == SqlEngine::Mssql {
            listed
        } else {
            None
        },
        conditions,
        or: query.logic == FilterLogic::Or,
        order: query
            .sort
            .iter()
            .map(|s| (s.column.as_str(), s.direction == SortDirection::Desc))
            .collect(),
    };
    let (placeholder, text_type) = select_style(engine);
    let built = build_table_select(
        &select,
        &|s| dialect.quote_ident(s),
        &|s| dialect.quote_schema(s),
        &placeholder,
        text_type,
    );
    Ok(PageSource {
        sql: built.sql,
        params: built.bind_values.unwrap_or_default(),
    })
}

/// The tables a batch's edits need metadata for, each once, in the order
/// the batch first names them.
pub fn metadata_targets<'a>(edits: impl IntoIterator<Item = &'a Edit>) -> Vec<&'a TableTarget> {
    let mut seen: HashSet<&TableTarget> = HashSet::new();
    let mut out = Vec::new();
    for target in edits.into_iter().filter_map(Edit::metadata_target) {
        if seen.insert(target) {
            out.push(target);
        }
    }
    out
}

/// Whether a change counts as DML (Decision 5), read before it is planned:
/// a grid edit, a SQLite TRUNCATE (built as `DELETE FROM`), or typed SQL
/// whose first word is INSERT, UPDATE or DELETE. [`plan_edit`] and
/// [`plan_sql`] set [`PlannedChange::dml`] the same way.
pub fn is_dml(change: &Change, engine: SqlEngine) -> bool {
    match change {
        Change::Edit {
            edit: Edit::TruncateTable { .. },
            ..
        } => engine == SqlEngine::Sqlite,
        Change::Edit {
            edit: Edit::DropObject { .. },
            ..
        } => false,
        Change::Edit { .. } => true,
        Change::Sql { sql, .. } => matches!(
            query_type(sql, engine),
            QueryType::Insert | QueryType::Update | QueryType::Delete
        ),
    }
}

// ── DuckDB extensions ──

/// What the DuckDB extensions tab asks for (`db.duckdbExtension`, Q9).
/// `name` must be `^[A-Za-z0-9_]+$`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ExtensionAction {
    /// `SELECT * FROM duckdb_extensions()`: its rows come back.
    List,
    Install {
        name: String,
    },
    Load {
        name: String,
    },
    Update {
        name: String,
    },
    /// INSTALL … FROM community, then LOAD.
    InstallCommunity {
        name: String,
    },
    /// INSTALL, then LOAD.
    InstallAndLoad {
        name: String,
    },
}

/// The statements for `action`, run one at a time: `INVALID_ARGUMENT` for a
/// name that isn't letters, digits and `_`.
pub fn extension_statements(action: &ExtensionAction) -> Result<Vec<String>, PlanError> {
    let name = match action {
        ExtensionAction::List => return Ok(vec!["SELECT * FROM duckdb_extensions()".into()]),
        ExtensionAction::Install { name }
        | ExtensionAction::Load { name }
        | ExtensionAction::Update { name }
        | ExtensionAction::InstallCommunity { name }
        | ExtensionAction::InstallAndLoad { name } => name,
    };
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(error(INVALID_ARGUMENT, "Invalid extension name."));
    }
    Ok(match action {
        ExtensionAction::List => vec![],
        ExtensionAction::Install { .. } => vec![format!("INSTALL '{name}'")],
        ExtensionAction::Load { .. } => vec![format!("LOAD '{name}'")],
        ExtensionAction::Update { .. } => vec![format!("UPDATE EXTENSIONS ({name})")],
        ExtensionAction::InstallCommunity { .. } => vec![
            format!("INSTALL '{name}' FROM community"),
            format!("LOAD '{name}'"),
        ],
        ExtensionAction::InstallAndLoad { .. } => {
            vec![format!("INSTALL '{name}'"), format!("LOAD '{name}'")]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_names_are_checked() {
        let install = |name: &str| ExtensionAction::InstallCommunity { name: name.into() };
        assert_eq!(
            extension_statements(&install("h3_x9")).unwrap(),
            ["INSTALL 'h3_x9' FROM community", "LOAD 'h3_x9'"]
        );
        for bad in ["", "a'b", "a;b", "a b", "é", "x')--"] {
            assert_eq!(
                extension_statements(&install(bad)).unwrap_err().code,
                INVALID_ARGUMENT,
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_nul_in_any_name_is_refused() {
        let users = TableTarget {
            schema: "public".into(),
            table: "users".into(),
        };
        let nul = |s: &str| s.replace('_', "\0");
        let edits = [
            Edit::UpdateCell {
                target: TableTarget {
                    schema: nul("pub_lic"),
                    table: "users".into(),
                },
                key: vec![("id".into(), Value::Int(1))],
                column: "name".into(),
                value: Value::Text("a\0b".into()),
            },
            Edit::SetDefault {
                target: users.clone(),
                key: vec![(nul("i_d"), Value::Int(1))],
                column: "name".into(),
            },
            Edit::InsertRow {
                target: users.clone(),
                values: vec![(nul("_"), Value::Null)],
            },
            Edit::DeleteRow {
                target: TableTarget {
                    schema: "public".into(),
                    table: nul("u_"),
                },
                key: vec![],
            },
            Edit::DropObject {
                target: TableTarget {
                    schema: "public".into(),
                    table: nul("_"),
                },
                kind: ObjectKind::View,
            },
        ];
        for edit in &edits {
            let e = check_edit_names(edit).unwrap_err();
            assert_eq!(e.code, INVALID_ARGUMENT, "{edit:?}");
            let e =
                check_edit_limits(std::slice::from_ref(edit), EditLimits::default()).unwrap_err();
            assert_eq!(e.code, INVALID_ARGUMENT);
            let pg = seaquel_engine_postgres::engine();
            let e = plan_edit(edit, None, pg.dialect().unwrap(), SqlEngine::Postgres).unwrap_err();
            assert_eq!(e.code, INVALID_ARGUMENT);
        }
        // A NUL in a value isn't a name.
        let fine = Edit::UpdateCell {
            target: users,
            key: vec![("id".into(), Value::Int(1))],
            column: "name".into(),
            value: Value::Text("a\0b".into()),
        };
        assert!(check_edit_names(&fine).is_ok());
    }

    #[test]
    fn classify_by_count_and_dml() {
        assert_eq!(classify(&[]), ApplyMode::Single);
        assert_eq!(classify(&[false]), ApplyMode::Single);
        assert_eq!(classify(&[true, true]), ApplyMode::Atomic);
        assert_eq!(classify(&[true, false, true]), ApplyMode::InOrder);
    }

    #[test]
    fn json_values_bind_as_json_only_in_json_columns() {
        let arr = Value::Array(vec![Value::Int(1), Value::Text("a".into())]);
        assert_eq!(
            json_value(arr.clone(), true),
            Value::Json(serde_json::json!([1, "a"]))
        );
        assert_eq!(json_value(arr.clone(), false), arr);
        assert_eq!(
            json_value(Value::Decimal("12.50".into()), true),
            Value::Json(serde_json::json!(12.5))
        );
        assert_eq!(
            json_value(Value::Decimal("18446744073709551615".into()), true),
            Value::Json(serde_json::json!(18446744073709551615u64))
        );
        // Past f64's precision: kept as it is.
        for d in [
            "0.1000000000000000000001",
            "12345678901234567.5",
            "1e400",
            "1.5e-310",
        ] {
            assert_eq!(
                json_value(Value::Decimal(d.into()), true),
                Value::Decimal(d.into()),
                "{d}"
            );
        }
        assert!(f64_exact("-0.000123456789012345"));
        assert!(!f64_exact("0.1234567890123456"));
        assert_eq!(
            json_value(Value::Bool(true), true),
            Value::Json(serde_json::json!(true))
        );
        for kept in [
            Value::Text("{\"a\":1}".into()),
            Value::Null,
            Value::Bytes(vec![1]),
            Value::Float(f64::NAN),
            Value::Array(vec![Value::Bytes(vec![1])]),
        ] {
            let got = json_value(kept.clone(), true);
            match (&kept, &got) {
                (Value::Float(a), Value::Float(b)) => assert!(a.is_nan() && b.is_nan()),
                _ => assert_eq!(got, kept),
            }
        }
    }

    #[test]
    fn metadata_targets_are_deduped_in_order() {
        let t = |s: &str| TableTarget {
            schema: "s".into(),
            table: s.into(),
        };
        let edits = vec![
            Edit::DeleteRow {
                target: t("b"),
                key: vec![],
            },
            Edit::TruncateTable { target: t("z") },
            Edit::InsertRow {
                target: t("a"),
                values: vec![],
            },
            Edit::DeleteRow {
                target: t("b"),
                key: vec![],
            },
        ];
        let got: Vec<&str> = metadata_targets(&edits)
            .into_iter()
            .map(|t| t.table.as_str())
            .collect();
        assert_eq!(got, ["b", "a"]);
    }
}
