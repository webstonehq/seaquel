//! The editor's run, planned (phase 5b, Decision 4).
//!
//! `db.run` hands Core the editor's whole text, a target (every statement,
//! or the one at a UTF-16 cursor), the parameter values from the dialog and
//! the page size. [`plan`] turns that into the statements to run, each with
//! its SQL after `{{param}}` substitution, its bind values, its query type
//! and how it runs ([`StatementKind`]), plus the destructive statements the
//! run must be confirmed for and the text a history row records. It does no
//! I/O and never panics on its input. Core's `Workspace::run` carries a plan
//! out on one connection; `Workspace::page` re-pages one statement from the
//! [`PageSource`] the run sent.
//!
//! The rules are today's TypeScript runner's, pinned by the fixtures in
//! `tests/fixtures/run` (their README lists the few places Core is meant to
//! differ, in `changes.json`).
//!
//! The wire types here are serialised to the GUIs through `seaquel-rpc`.
//! Their `Debug` never shows the editor's text, SQL or parameter values.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use seaquel_sql::ast::{column_refs, ColumnRef};
use seaquel_sql::offsets::utf16_to_byte;
use seaquel_sql::params::{substitute_with, SizeBound, Values};
use seaquel_sql::scan::{has_row_limit, split_statements, statement_at, Statement};
use seaquel_sql::statements::{
    destructive_reason, query_type, table_from_select, DestructiveReason, QueryType, TableRef,
};
use seaquel_sql::SqlEngine;
use seaquel_types::storage::PersistedQueryHistoryItem;
use seaquel_types::{StreamBatch, Value};

/// `db.run` without `confirmed` when the run holds a destructive statement.
pub const CONFIRM_REQUIRED: &str = "CONFIRM_REQUIRED";

/// The most destructive statements a `CONFIRM_REQUIRED` refusal lists; its
/// `destructiveTotal` counts them all. A run of 645,000 DROPs made one
/// 38.6 MB frame before runs were capped (phase 5b probe, N3), and the
/// dialog shows a few at a time anyway.
pub const MAX_DESTRUCTIVE_LISTED: usize = 100;
/// A page size past the cap, page 0, an offset that overflows, a `db.page`
/// whose statement isn't a SELECT, or a run past its interface's
/// [`RunLimits`].
pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
/// A parameter value that can't be substituted. At the cursor it fails the
/// run; in run all it is that statement's error.
pub const INVALID_PARAMETERS: &str = "INVALID_PARAMETERS";

/// What one interface lets a run carry ([`plan`]; Core's page checks only
/// the text size). The default is no limit: the desktop app, the CLI and
/// the MCP server run a script of any size with any values, as before 5b.
/// The web server sets all four (`WEB_RUN_LIMITS`; owner, 2026-10-02),
/// since its frames are 8 MiB and planning them blocked a worker (phase 5b
/// probe I1, review C1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunLimits {
    /// The longest run text, or page SQL, in bytes: `INVALID_ARGUMENT`
    /// past it, checked before anything scans the text. Planning scans the
    /// text several times and the scanner keeps ~32 bytes per token: one
    /// 8 MiB statement took ~560 MB and ~0.5 s, one of 2 MiB ~140 MB and
    /// ~150 ms.
    pub max_text_bytes: Option<usize>,
    /// The most statements a run of the whole text may hold:
    /// `INVALID_ARGUMENT` past it. Each is a result in the GUI and a round
    /// trip to the database.
    pub max_statements: Option<usize>,
    /// The most parameter values a run may send: `INVALID_PARAMETERS` past
    /// it. Runs only: `db.page` is exempt (it substitutes nothing, and its
    /// binds are per-use copies on MySQL).
    pub max_param_values: Option<usize>,
    /// The most bytes of parameter values (their text; see
    /// [`param_bytes`]) a run may send: `INVALID_PARAMETERS` past it. Runs
    /// only, as [`RunLimits::max_param_values`].
    pub max_param_bytes: Option<usize>,
}

/// The options [`plan`] takes besides the text, target, values and engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanOptions {
    /// 0 streams every SELECT.
    pub page_size: u32,
    /// Pending changes: every statement that isn't a SELECT is deferred.
    pub defer_writes: bool,
    /// Core passes `max_query_rows() - 1`: a page fetches one row more.
    pub max_page_size: u32,
    /// The interface's [`RunLimits`].
    pub limits: RunLimits,
}

/// The bytes of parameter values, as [`RunLimits::max_param_bytes`] counts
/// them: text and decimals by their length, bytes by theirs, JSON by its
/// text, anything else 8.
pub fn param_bytes<'a>(values: impl IntoIterator<Item = &'a Value>) -> usize {
    fn one(v: &Value) -> usize {
        match v {
            Value::Text(s) | Value::Decimal(s) => s.len(),
            Value::Bytes(b) => b.len(),
            Value::Json(j) => j.to_string().len(),
            Value::Array(items) => items.iter().map(one).fold(0, usize::saturating_add),
            _ => 8,
        }
    }
    values.into_iter().map(one).fold(0, usize::saturating_add)
}

/// `INVALID_PARAMETERS` for more parameter values, or more bytes of them,
/// than `limits` allows.
pub fn check_param_values<'a>(
    values: impl IntoIterator<Item = &'a Value> + Clone,
    limits: RunLimits,
) -> Result<(), PlanError> {
    if let Some(max) = limits.max_param_values {
        let n = values.clone().into_iter().count();
        if n > max {
            return Err(PlanError::new(
                INVALID_PARAMETERS,
                format!(
                    "A run can send at most {} parameter values; this one sends {}.",
                    thousands(max),
                    thousands(n)
                ),
            ));
        }
    }
    if let Some(max) = limits.max_param_bytes {
        if param_bytes(values) > max {
            return Err(PlanError::new(
                INVALID_PARAMETERS,
                format!("The parameter values are larger than {}.", byte_size(max)),
            ));
        }
    }
    Ok(())
}

/// How much a run may grow when its parameter values are filled in: the
/// statements' [`substituted_size_bound`] less their own length, on every
/// interface. A value is copied (SQL Server, DuckDB) or bound
/// (MySQL/MariaDB) once per use, so without it a 1 MiB value used 300,000
/// times would make terabytes before anything ran (phase 5b probe, N3).
/// Only the growth counts, so a script of any size with a few parameters
/// runs; 32 MiB still takes a 1 MiB value used a dozen times.
pub const MAX_RUN_SUBSTITUTED_BYTES: usize = 32 * 1024 * 1024;

/// `INVALID_PARAMETERS` when filling `values` into `statements` could grow
/// them by more than [`MAX_RUN_SUBSTITUTED_BYTES`] in all. Only the growth
/// counts (each statement's [`substituted_size_bound`] less its length), so
/// the statements' own size never matters. Pure and cheap: nothing is
/// substituted. Each value is costed once however many statements use it
/// (phase 5b review, C1).
pub fn check_substitution_budget<'a>(
    statements: impl IntoIterator<Item = &'a str>,
    values: &Values<'_>,
) -> Result<(), PlanError> {
    let mut bound = SizeBound::new(values);
    let growth = statements.into_iter().fold(0usize, |total, sql| {
        total.saturating_add(bound.bound(sql).saturating_sub(sql.len()))
    });
    if growth > MAX_RUN_SUBSTITUTED_BYTES {
        return Err(PlanError::new(
            INVALID_PARAMETERS,
            "Filling in the parameter values would add more than 32 MiB to the query. \
             Use them fewer times, or run it in parts.",
        ));
    }
    Ok(())
}

/// `INVALID_ARGUMENT` for text past `limits.max_text_bytes`.
pub fn check_text_size(text: &str, limits: RunLimits) -> Result<(), PlanError> {
    match limits.max_text_bytes {
        Some(max) if text.len() > max => Err(PlanError::new(
            INVALID_ARGUMENT,
            format!(
                "The query text is longer than {}. Run it in parts.",
                byte_size(max)
            ),
        )),
        _ => Ok(()),
    }
}

/// `n` bytes as `2 MiB`, `64 KiB` or `100 bytes`.
fn byte_size(n: usize) -> String {
    const KIB: usize = 1024;
    if n >= KIB * KIB && n.is_multiple_of(KIB * KIB) {
        format!("{} MiB", n / (KIB * KIB))
    } else if n >= KIB && n.is_multiple_of(KIB) {
        format!("{} KiB", n / KIB)
    } else {
        format!("{n} bytes")
    }
}

/// `n` with a comma between thousands: `10,000`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ── The wire ──

/// `db.run`. `Debug` is by hand: it shows the target, page size and flags,
/// never `text`, parameter values or the history context.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct RunParams {
    /// Core's connection id.
    pub connection_id: String,
    /// Fresh per run; `db.cancel` takes it. Unique per run, as a
    /// `queryStream`'s id.
    pub stream_id: String,
    /// The editor's whole text, well-formed (a lone surrogate replaced by
    /// U+FFFD, one UTF-16 unit, so the cursor still lines up).
    pub text: String,
    pub target: RunTarget,
    /// The dialog's values. Present only when the dialog was shown: without
    /// them, `{{name}}` goes to the database as typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub params: Option<Vec<ParamValue>>,
    /// Rows per page; 0 streams every SELECT. At most Core's page cap
    /// (`max_query_rows() - 1`, since a page fetches one row more).
    pub page_size: u32,
    /// The user confirmed the destructive statements. Absent is false.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub confirmed: bool,
    /// Pending changes are on: every statement that isn't a SELECT comes
    /// back as `statementDeferred` and doesn't run. Absent is false.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub defer_writes: bool,
    /// Record the run in history under this saved connection. Without it
    /// nothing is recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub history: Option<HistoryContext>,
}

impl fmt::Debug for RunParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunParams")
            .field("connection_id", &self.connection_id)
            .field("stream_id", &self.stream_id)
            .field("text_len", &self.text.len())
            .field("target", &self.target)
            .field("params", &self.params.as_ref().map(Vec::len))
            .field("page_size", &self.page_size)
            .field("confirmed", &self.confirmed)
            .field("defer_writes", &self.defer_writes)
            .field("history", &self.history.is_some())
            .finish()
    }
}

/// What a run runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum RunTarget {
    /// Every statement in the text ("Run").
    All,
    /// The statement at the cursor ("Run current"). `cursor` is a UTF-16
    /// offset into the text, Monaco's unit.
    Current {
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        cursor: u64,
    },
}

/// One parameter value: `WireParameterValue`'s shape, the value in the cell
/// wire format. A missing value is NULL.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ParamValue {
    pub name: String,
    #[serde(default = "null")]
    #[cfg_attr(feature = "ts", ts(optional, type = "unknown"))]
    pub value: Value,
}

fn null() -> Value {
    Value::Null
}

/// Whose history a run goes in: the saved connection, as the GUI knows it.
/// Core's connection id isn't the saved connection's, and the labels live in
/// the GUI's state.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HistoryContext {
    /// The saved connection's id.
    pub connection_id: String,
    pub connection_name: String,
    /// `ConnectionLabel[]`, stored as given. Anything but an array is
    /// refused when the request is read.
    #[cfg_attr(
        feature = "ts",
        ts(as = "Vec<seaquel_types::storage::ConnectionLabel>")
    )]
    #[serde(deserialize_with = "json_array")]
    pub connection_labels: Box<RawValue>,
}

/// A raw JSON value that must be an array.
fn json_array<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Box<RawValue>, D::Error> {
    let raw = Box::<RawValue>::deserialize(d)?;
    serde_json::from_str::<Vec<serde::de::IgnoredAny>>(raw.get())
        .map_err(|_| serde::de::Error::custom("connectionLabels must be an array"))?;
    Ok(raw)
}

impl fmt::Debug for HistoryContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HistoryContext")
            .field("connection_id", &self.connection_id)
            .finish_non_exhaustive()
    }
}

/// `db.page`: one statement again, from the source its `statementStart`
/// carried. `Debug` shows no SQL or values.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PageParams {
    pub connection_id: String,
    pub stream_id: String,
    pub source: PageSource,
    /// 1-based.
    pub page: u32,
    /// 0 streams.
    pub page_size: u32,
}

impl fmt::Debug for PageParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageParams")
            .field("connection_id", &self.connection_id)
            .field("stream_id", &self.stream_id)
            .field("source", &self.source)
            .field("page", &self.page)
            .field("page_size", &self.page_size)
            .finish()
    }
}

/// What a statement ran: its SQL after substitution and its bind values.
/// `Debug` shows only their sizes.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PageSource {
    pub sql: String,
    #[cfg_attr(feature = "ts", ts(type = "Array<unknown>"))]
    pub params: Vec<Value>,
}

impl fmt::Debug for PageSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageSource")
            .field("sql_len", &self.sql.len())
            .field("params", &self.params.len())
            .finish()
    }
}

/// How a statement runs (Decision 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum StatementKind {
    /// A SELECT, paged: `pageSize + 1` rows, and a count when the page was
    /// full, or empty past the first page.
    Page,
    /// A SELECT streamed whole: page size 0, or its own row limit.
    Stream,
    /// INSERT, UPDATE or DELETE.
    Write,
    /// Anything else, run unpaged; its rows show when it returns columns.
    Utility,
}

/// A statement the run must be confirmed for. `Debug` shows no SQL.
#[derive(Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DestructiveStatement {
    /// The statement's position in the whole text, as the dialog shows it.
    pub index: u32,
    /// The statement as typed, before substitution.
    pub sql: String,
    pub reason: DestructiveReason,
}

/// What `db.run` and `db.page` send, in order: per statement a
/// `statementStart`, its `batch`es, then `statementDone` or
/// `statementError` (or only `statementDeferred`, or only `statementError`
/// for a planned failure), and then exactly one `done` or `error`. A
/// cancelled run ends with neither.
#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum RunEvent {
    StatementStart {
        /// The statement's position in the run (0 at the cursor).
        index: u32,
        /// The statement as typed, before substitution.
        sql: String,
        source: PageSource,
        query_type: QueryType,
        kind: StatementKind,
        page: u32,
        page_size: u32,
        /// The source table for inline edits (a SELECT only).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        table: Option<TableRef>,
        /// Each output column's base column (a SELECT only).
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        column_refs: Option<Vec<Option<ColumnRef>>>,
    },
    /// Rows for the latest `statementStart`: `StreamBatch` flattened,
    /// snake_case (`is_final`), as in `StreamEvent`.
    Batch(StreamBatch),
    StatementDone {
        index: u32,
        /// The statement's time in Core (a page includes its count).
        elapsed_ms: f64,
        /// Page: counted, or estimated (`countEstimated`). Stream, and a
        /// utility statement that returned rows: the rows sent. Otherwise 0.
        #[cfg_attr(feature = "ts", ts(type = "number"))]
        total_rows: u64,
        /// 1 unless paged.
        total_pages: u32,
        count_estimated: bool,
        /// A write only.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
        rows_affected: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
        last_insert_id: Option<i64>,
    },
    StatementError {
        index: u32,
        code: String,
        message: String,
        elapsed_ms: f64,
        /// The statement as typed, only on a planned failure (a value that
        /// can't be substituted in run all), which has no `statementStart`.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        sql: Option<String>,
    },
    /// Pending changes are on and the statement isn't a SELECT: queue it.
    StatementDeferred {
        index: u32,
        sql: String,
        source: PageSource,
        query_type: QueryType,
    },
    Done {
        /// Statements planned (run, deferred or failed); 0 when there was
        /// nothing to run.
        statements: u32,
        /// At least one statement ran and none failed.
        succeeded: bool,
        /// The history row as appended, when the run recorded one.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        history: Option<PersistedQueryHistoryItem>,
    },
    Error {
        code: String,
        message: String,
        /// `CONFIRM_REQUIRED` only: what to confirm, the first
        /// [`MAX_DESTRUCTIVE_LISTED`] in text order.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        destructive: Option<Vec<DestructiveStatement>>,
        /// `CONFIRM_REQUIRED` only: how many destructive statements the run
        /// holds, listed or not.
        #[serde(skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        destructive_total: Option<u32>,
    },
}

impl RunEvent {
    /// A terminal `error` with no destructive list.
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        RunEvent::Error {
            code: code.into(),
            message: message.into(),
            destructive: None,
            destructive_total: None,
        }
    }

    /// The `CONFIRM_REQUIRED` refusal for a run holding `destructive`: the
    /// first [`MAX_DESTRUCTIVE_LISTED`] of them and their total.
    pub fn confirm_required(mut destructive: Vec<DestructiveStatement>) -> Self {
        let n = destructive.len();
        destructive.truncate(MAX_DESTRUCTIVE_LISTED);
        RunEvent::Error {
            code: CONFIRM_REQUIRED.to_string(),
            message: format!(
                "{n} destructive statement{} must be confirmed before this run",
                if n == 1 { "" } else { "s" }
            ),
            destructive: Some(destructive),
            destructive_total: Some(index_u32(n)),
        }
    }

    /// `done` and `error` end a run.
    pub fn is_terminal(&self) -> bool {
        matches!(self, RunEvent::Done { .. } | RunEvent::Error { .. })
    }
}

// ── Planning ──

/// A run-level failure: the run fails before anything executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    pub code: String,
    pub message: String,
}

impl PlanError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PlanError {}

/// What [`plan`] worked out. `Debug` shows no SQL.
#[derive(Clone, PartialEq)]
pub struct RunPlan {
    pub statements: Vec<Planned>,
    /// Every destructive statement in the run, deferred ones included,
    /// checked on the text before substitution.
    pub destructive: Vec<DestructiveStatement>,
    /// What a history row records: the whole text for run all, the
    /// statement as typed at the cursor.
    pub history_query: String,
}

/// One statement of a run. `Debug` shows no SQL.
#[derive(Clone, PartialEq)]
pub struct Planned {
    /// Its position in the run (0 at the cursor).
    pub index: u32,
    /// Its position in the whole text.
    pub text_index: u32,
    /// As typed, before substitution.
    pub sql: String,
    pub step: Step,
}

/// What to do with a statement. `Debug` shows no SQL or values.
#[derive(Clone, PartialEq)]
pub enum Step {
    Run {
        source: PageSource,
        query_type: QueryType,
        kind: StatementKind,
        table: Option<TableRef>,
        column_refs: Option<Vec<Option<ColumnRef>>>,
    },
    Defer {
        source: PageSource,
        query_type: QueryType,
    },
    /// Reported as the statement's error without touching the database.
    Fail { code: String, message: String },
}

/// Plan a run. Pure. `Err` only for run-level failures:
/// `INVALID_PARAMETERS` for a value that can't be substituted at the
/// cursor, `INVALID_ARGUMENT` for a page size past `max_page_size` (Core
/// passes `max_query_rows() - 1`: a page fetches one row more), for text
/// past `limits.max_text_bytes` and for run all past
/// `limits.max_statements`; `INVALID_PARAMETERS` for more parameter values
/// than the limits allow, or a run that would grow by more than
/// [`MAX_RUN_SUBSTITUTED_BYTES`] once substituted.
///
/// - `params` is `None` when the dialog wasn't shown: nothing is
///   substituted.
/// - A cursor with no statement (only comments or whitespace) plans
///   nothing: `db.run` ends with `done`, `statements: 0`.
pub fn plan(
    text: &str,
    target: &RunTarget,
    params: Option<&[(String, Value)]>,
    engine: SqlEngine,
    options: PlanOptions,
) -> Result<RunPlan, PlanError> {
    let PlanOptions {
        page_size,
        defer_writes,
        max_page_size,
        limits,
    } = options;
    check_page_size(page_size, max_page_size)?;
    check_text_size(text, limits)?;
    if let Some(values) = params {
        check_param_values(values.iter().map(|(_, v)| v), limits)?;
    }
    // Looked up once for every statement (phase 5b review, C1).
    let values = params.map(Values::new);
    let chosen: Vec<(u32, Statement)> = match target {
        RunTarget::All => {
            let all = split_statements(text, engine);
            if let Some(max) = limits.max_statements.filter(|&max| all.len() > max) {
                return Err(PlanError::new(
                    INVALID_ARGUMENT,
                    format!(
                        "A run can hold at most {} statements; this one has {}. Run it in parts.",
                        thousands(max),
                        thousands(all.len())
                    ),
                ));
            }
            all.into_iter().map(|s| (index_u32(s.index), s)).collect()
        }
        RunTarget::Current { cursor } => {
            let cursor = usize::try_from(*cursor).unwrap_or(usize::MAX);
            statement_at(text, utf16_to_byte(text, cursor), engine)
                .into_iter()
                .map(|s| (0, s))
                .collect()
        }
    };

    if let Some(values) = &values {
        check_substitution_budget(
            chosen
                .iter()
                .map(|(_, s)| text.get(s.text.clone()).unwrap_or_default()),
            values,
        )?;
    }

    let mut statements = Vec::with_capacity(chosen.len());
    let mut destructive = Vec::new();
    for (index, s) in chosen {
        let sql = text.get(s.text.clone()).unwrap_or_default().to_string();
        let text_index = index_u32(s.index);
        if let Some(reason) = destructive_reason(&sql, engine) {
            destructive.push(DestructiveStatement {
                index: text_index,
                sql: sql.clone(),
                reason,
            });
        }
        let source = match &values {
            None => PageSource {
                sql: sql.clone(),
                params: Vec::new(),
            },
            Some(values) => match substitute_with(&sql, values, engine, false) {
                Ok(out) => PageSource {
                    sql: out.sql,
                    params: out.bind_values,
                },
                Err(e) => {
                    if matches!(target, RunTarget::Current { .. }) {
                        return Err(PlanError::new(INVALID_PARAMETERS, e.message));
                    }
                    statements.push(Planned {
                        index,
                        text_index,
                        sql,
                        step: Step::Fail {
                            code: INVALID_PARAMETERS.to_string(),
                            message: e.message,
                        },
                    });
                    continue;
                }
            },
        };
        let step = step_for(source, engine, page_size, defer_writes);
        statements.push(Planned {
            index,
            text_index,
            sql,
            step,
        });
    }

    let history_query = match target {
        RunTarget::All => text.to_string(),
        RunTarget::Current { .. } => statements
            .first()
            .map(|p| p.sql.clone())
            .unwrap_or_default(),
    };
    Ok(RunPlan {
        statements,
        destructive,
        history_query,
    })
}

/// How `source` runs: its query type decides, then the page size and its
/// own row limit (Decision 5).
fn step_for(source: PageSource, engine: SqlEngine, page_size: u32, defer_writes: bool) -> Step {
    let query_type = query_type(&source.sql, engine);
    if defer_writes && query_type != QueryType::Select {
        return Step::Defer { source, query_type };
    }
    let (kind, table, column_refs) = match query_type {
        QueryType::Select => (
            select_kind(&source.sql, engine, page_size),
            table_from_select(&source.sql, engine),
            column_refs(&source.sql, engine),
        ),
        QueryType::Insert | QueryType::Update | QueryType::Delete => {
            (StatementKind::Write, None, None)
        }
        QueryType::Other => (StatementKind::Utility, None, None),
    };
    Step::Run {
        source,
        query_type,
        kind,
        table,
        column_refs,
    }
}

/// A SELECT streams at page size 0 or when it limits its own rows
/// (LIMIT/OFFSET/FETCH, TOP on SQL Server); otherwise it's paged.
pub fn select_kind(sql: &str, engine: SqlEngine, page_size: u32) -> StatementKind {
    if page_size == 0 || has_row_limit(sql, engine) {
        StatementKind::Stream
    } else {
        StatementKind::Page
    }
}

/// `INVALID_ARGUMENT` for a page size past `max`.
pub fn check_page_size(page_size: u32, max: u32) -> Result<(), PlanError> {
    if page_size > max {
        return Err(PlanError::new(
            INVALID_ARGUMENT,
            format!("The page size can be at most {max} rows"),
        ));
    }
    Ok(())
}

/// The row offset of `page` (1-based) at `page_size`: `INVALID_ARGUMENT`
/// for page 0, a page size past `max_page_size`, or an offset past `u64`.
pub fn page_offset(page: u32, page_size: u32, max_page_size: u32) -> Result<u64, PlanError> {
    check_page_size(page_size, max_page_size)?;
    if page == 0 {
        return Err(PlanError::new(INVALID_ARGUMENT, "Pages start at 1"));
    }
    u64::from(page - 1)
        .checked_mul(u64::from(page_size))
        .ok_or_else(|| PlanError::new(INVALID_ARGUMENT, "The page is out of range"))
}

fn index_u32(i: usize) -> u32 {
    u32::try_from(i).unwrap_or(u32::MAX)
}

// ── Debug without SQL, values or names ──

impl fmt::Debug for DestructiveStatement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DestructiveStatement")
            .field("index", &self.index)
            .field("sql_len", &self.sql.len())
            .field("reason", &self.reason)
            .finish()
    }
}

impl fmt::Debug for RunEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunEvent::StatementStart {
                index,
                source,
                query_type,
                kind,
                page,
                page_size,
                ..
            } => f
                .debug_struct("StatementStart")
                .field("index", index)
                .field("source", source)
                .field("query_type", query_type)
                .field("kind", kind)
                .field("page", page)
                .field("page_size", page_size)
                .finish_non_exhaustive(),
            RunEvent::Batch(b) => f
                .debug_struct("Batch")
                .field("columns", &b.columns.as_ref().map(Vec::len))
                .field("rows", &b.rows.len())
                .field("is_final", &b.is_final)
                .finish(),
            RunEvent::StatementDone {
                index,
                elapsed_ms,
                total_rows,
                total_pages,
                count_estimated,
                rows_affected,
                last_insert_id,
            } => f
                .debug_struct("StatementDone")
                .field("index", index)
                .field("elapsed_ms", elapsed_ms)
                .field("total_rows", total_rows)
                .field("total_pages", total_pages)
                .field("count_estimated", count_estimated)
                .field("rows_affected", rows_affected)
                .field("last_insert_id", last_insert_id)
                .finish(),
            RunEvent::StatementError {
                index, code, sql, ..
            } => f
                .debug_struct("StatementError")
                .field("index", index)
                .field("code", code)
                .field("planned", &sql.is_some())
                .finish_non_exhaustive(),
            RunEvent::StatementDeferred {
                index,
                source,
                query_type,
                ..
            } => f
                .debug_struct("StatementDeferred")
                .field("index", index)
                .field("source", source)
                .field("query_type", query_type)
                .finish_non_exhaustive(),
            RunEvent::Done {
                statements,
                succeeded,
                history,
            } => f
                .debug_struct("Done")
                .field("statements", statements)
                .field("succeeded", succeeded)
                .field("history", &history.is_some())
                .finish(),
            RunEvent::Error {
                code,
                destructive,
                destructive_total,
                ..
            } => f
                .debug_struct("Error")
                .field("code", code)
                .field("destructive", destructive)
                .field("destructive_total", destructive_total)
                .finish_non_exhaustive(),
        }
    }
}

impl fmt::Debug for RunPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunPlan")
            .field("statements", &self.statements)
            .field("destructive", &self.destructive)
            .field("history_query_len", &self.history_query.len())
            .finish()
    }
}

impl fmt::Debug for Planned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Planned")
            .field("index", &self.index)
            .field("text_index", &self.text_index)
            .field("sql_len", &self.sql.len())
            .field("step", &self.step)
            .finish()
    }
}

impl fmt::Debug for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::Run {
                source,
                query_type,
                kind,
                ..
            } => f
                .debug_struct("Run")
                .field("source", source)
                .field("query_type", query_type)
                .field("kind", kind)
                .finish_non_exhaustive(),
            Step::Defer { source, query_type } => f
                .debug_struct("Defer")
                .field("source", source)
                .field("query_type", query_type)
                .finish(),
            Step::Fail { code, .. } => f
                .debug_struct("Fail")
                .field("code", code)
                .finish_non_exhaustive(),
        }
    }
}

// ── History ──

/// The history row a run records (Decision 11): `hist-<uuid>` as `id`, the
/// time as `Date.toISOString()` writes it, and the saved connection's id,
/// name and labels from `ctx`.
pub fn history_item(
    ctx: &HistoryContext,
    query: &str,
    elapsed_ms: f64,
    row_count: u64,
    unix_time: std::time::Duration,
    id: String,
) -> PersistedQueryHistoryItem {
    PersistedQueryHistoryItem {
        id,
        query: query.to_string(),
        timestamp: iso_timestamp(unix_time),
        execution_time: elapsed_ms,
        // A JS number, as the TS stored it.
        row_count: row_count as f64,
        connection_id: ctx.connection_id.clone(),
        favorite: false,
        connection_labels_snapshot: Some(ctx.connection_labels.clone()),
        connection_name_snapshot: ctx.connection_name.clone(),
        // A run records its text with `{{param}}`s; its values aren't kept.
        params: None,
    }
}

/// `unix_time` as `Date.toISOString()` prints it:
/// `2026-10-02T12:34:56.789Z`, milliseconds truncated. Past year 9999 (never
/// from a real clock) it clamps to the last millisecond of 9999.
pub fn iso_timestamp(unix_time: std::time::Duration) -> String {
    const MAX_MS: u128 = 253_402_300_799_999; // 9999-12-31T23:59:59.999Z
    let ms = unix_time.as_millis().min(MAX_MS);
    let nanos = i128::try_from(ms).unwrap_or_default() * 1_000_000;
    let dt = time::OffsetDateTime::from_unix_timestamp_nanos(nanos)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        dt.year(),
        u8::from(dt.month()),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_match_to_iso_string() {
        assert_eq!(iso_timestamp(Duration::ZERO), "1970-01-01T00:00:00.000Z");
        // new Date(1790000000123).toISOString()
        assert_eq!(
            iso_timestamp(Duration::from_millis(1_790_000_000_123)),
            "2026-09-21T14:13:20.123Z"
        );
        // Sub-millisecond parts are dropped, as Date has none.
        assert_eq!(
            iso_timestamp(Duration::from_nanos(1_790_000_000_123_999_999)),
            "2026-09-21T14:13:20.123Z"
        );
        assert_eq!(iso_timestamp(Duration::MAX), "9999-12-31T23:59:59.999Z");
    }

    #[test]
    fn connection_labels_must_be_an_array() {
        let ctx = |labels: serde_json::Value| {
            serde_json::from_value::<HistoryContext>(serde_json::json!({
                "connectionId": "c", "connectionName": "n", "connectionLabels": labels,
            }))
        };
        assert!(ctx(serde_json::json!([])).is_ok());
        assert!(ctx(serde_json::json!([{"id": "l", "name": "x", "color": "red"}])).is_ok());
        for bad in [
            serde_json::json!({}),
            serde_json::json!("x"),
            serde_json::json!(null),
            serde_json::json!(1),
        ] {
            assert!(ctx(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn debug_shows_no_text_values_or_sql() {
        let params: RunParams = serde_json::from_value(serde_json::json!({
            "connectionId": "c", "streamId": "s", "text": "SELECT 'canary-text'",
            "target": {"type": "current", "cursor": 3},
            "params": [{"name": "a", "value": "canary-value"}],
            "pageSize": 100,
            "history": {"connectionId": "saved", "connectionName": "canary-name",
                        "connectionLabels": [{"name": "canary-label"}]}
        }))
        .unwrap();
        let page: PageParams = serde_json::from_value(serde_json::json!({
            "connectionId": "c", "streamId": "s",
            "source": {"sql": "SELECT 'canary-sql'", "params": ["canary-bind"]},
            "page": 2, "pageSize": 10
        }))
        .unwrap();
        let plan = plan(
            "SELECT 'canary-a' AS x; DELETE FROM canary_t; SELECT {{v}}",
            &RunTarget::All,
            Some(&[("v".to_string(), Value::Bytes(vec![1]))]),
            SqlEngine::Postgres,
            PlanOptions {
                page_size: 100,
                max_page_size: 1000,
                ..PlanOptions::default()
            },
        )
        .unwrap();
        let events = [
            RunEvent::StatementStart {
                index: 0,
                sql: "SELECT 'canary-a'".into(),
                source: PageSource {
                    sql: "canary".into(),
                    params: vec![Value::Text("canary".into())],
                },
                query_type: QueryType::Select,
                kind: StatementKind::Page,
                page: 1,
                page_size: 1,
                table: Some(TableRef {
                    schema: None,
                    table: "canary_t".into(),
                }),
                column_refs: None,
            },
            RunEvent::Batch(StreamBatch {
                columns: Some(vec!["canary".into()]),
                rows: vec![vec![Value::Text("canary".into())]],
                is_final: true,
                truncated: false,
            }),
            RunEvent::StatementError {
                index: 0,
                code: "X".into(),
                message: "canary".into(),
                elapsed_ms: 0.0,
                sql: Some("canary".into()),
            },
            RunEvent::Error {
                code: "X".into(),
                message: "canary".into(),
                destructive: Some(plan.destructive.clone()),
                destructive_total: Some(1),
            },
        ];
        let debug = format!("{params:?} {params:#?} {page:?} {page:#?} {plan:?} {events:?}");
        assert!(!debug.contains("canary"), "{debug}");
        assert!(debug.contains("page_size"), "{debug}");
    }
}
