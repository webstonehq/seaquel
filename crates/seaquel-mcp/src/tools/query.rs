//! `run_query` and `explain_query`: refused when the connection doesn't
//! share data. Queries run through Core's read-only path (the token check,
//! then the database's read-only mode) with a row limit, and EXPLAIN through
//! Core's read-only EXPLAIN. Both pass the call timeout to the database too.

use futures::StreamExt;
use seaquel_core::QueryOptions;
use seaquel_types::{StreamEvent, Value};
use serde_json::{json, Value as Json};

use super::{require_data, ExplainArgs, RunQueryArgs};
use crate::error::{ToolError, INVALID_ARGUMENT};
use crate::exposed::Exposed;
use crate::format;
use crate::server::Inner;

/// `max_rows` when the host passes none.
pub const DEFAULT_MAX_ROWS: u32 = 100;
/// The largest `max_rows` a host may ask for.
pub const MAX_MAX_ROWS: u32 = 1000;

/// A stream that ended without `Done` or an error: it was cancelled.
pub const CANCELLED: &str = "CANCELLED";

pub(crate) fn max_rows(requested: Option<u32>) -> Result<usize, ToolError> {
    match requested.unwrap_or(DEFAULT_MAX_ROWS) {
        n @ 1..=MAX_MAX_ROWS => Ok(n as usize),
        n => Err(ToolError::new(
            INVALID_ARGUMENT,
            format!("max_rows must be between 1 and {MAX_MAX_ROWS}, got {n}"),
        )),
    }
}

pub(crate) async fn run_query(inner: &Inner, args: RunQueryArgs) -> Result<Json, ToolError> {
    let c = inner.resolve(&args.connection)?;
    let max_rows = max_rows(args.max_rows)?;
    require_data(inner, c).await?;
    run_rows(inner, c, args.sql, Vec::new(), max_rows).await
}

/// About the most bytes a query result's JSON may take: rows stop being
/// added before one would take it past this, and the result says so. With
/// cells cut at [`format::MAX_CELL_BYTES`], this bounds what the server
/// builds and sends whatever the query returns.
pub const MAX_RESULT_BYTES: usize = 4 * 1024 * 1024;

/// The byte budget the driver gets (Core's `QueryOptions::max_bytes`): it
/// stops fetching once the decoded rows it kept reach this, so peak memory
/// no longer follows the result's size. Above [`MAX_RESULT_BYTES`], so on
/// most results the output cap is the one that shows; it shows alone when
/// cells are cut (at [`format::MAX_CELL_BYTES`]) or a result is very wide.
/// One row can still exceed it: a cell isn't split.
pub const MAX_FETCH_BYTES: usize = 8 * 1024 * 1024;

/// Run `sql` read-only with at most `max_rows` rows, under the per-call
/// timeout, and format the result: columns once, rows as arrays, cells cut
/// at [`format::MAX_CELL_BYTES`], and rows up to [`MAX_RESULT_BYTES`].
///
/// Rows are formatted as they arrive and the driver's values dropped one
/// row at a time. The driver itself fetches up to `max_rows` rows and stops
/// once they hold [`MAX_FETCH_BYTES`].
pub(crate) async fn run_rows(
    inner: &Inner,
    c: &Exposed,
    sql: String,
    params: Vec<Value>,
    max_rows: usize,
) -> Result<Json, ToolError> {
    let query_id = inner.query_id();
    let rows = inner
        .timed(async {
            let connection_id = inner.connection(c).await?;
            // The call timeout also goes to the database (a statement
            // timeout), as a backstop for `timed`'s deadline: a query the
            // cancel doesn't reach still ends there, with `TIMEOUT`.
            let options = QueryOptions::default()
                .with_read_only(true)
                .with_max_rows(Some(max_rows))
                .with_max_bytes(Some(MAX_FETCH_BYTES))
                .with_timeout(Some(inner.options.call_timeout));
            let mut stream = inner.workspace.query_stream(
                &inner.core,
                query_id.clone(),
                connection_id,
                sql,
                params,
                options,
            );
            let mut rows = Rows::new(max_rows);
            while let Some(event) = stream.next().await {
                match event {
                    StreamEvent::Batch(batch) => {
                        if let Some(cols) = batch.columns {
                            rows.set_columns(cols);
                        }
                        rows.truncated |= batch.truncated;
                        for row in batch.rows {
                            if !rows.push(row) {
                                // Full: dropping the stream stops the driver.
                                return Ok(rows);
                            }
                        }
                    }
                    StreamEvent::Done => return Ok(rows),
                    StreamEvent::Error { code, message } => {
                        return Err(ToolError::new(code, message))
                    }
                }
            }
            Err(ToolError::new(CANCELLED, "The query was cancelled"))
        })
        .await?;
    Ok(rows.into_json())
}

/// A result being built, within its row limit and byte budget.
struct Rows {
    max_rows: usize,
    columns: Vec<String>,
    rows: Vec<Json>,
    /// The JSON size of `columns` and `rows` so far.
    bytes: usize,
    /// The query had more rows than `max_rows`, or than the driver's
    /// [`MAX_FETCH_BYTES`] let through.
    truncated: bool,
    /// A row was left out for the byte budget.
    too_big: bool,
    cut_cells: usize,
}

impl Rows {
    fn new(max_rows: usize) -> Self {
        Self {
            max_rows,
            columns: Vec::new(),
            rows: Vec::new(),
            bytes: 0,
            truncated: false,
            too_big: false,
            cut_cells: 0,
        }
    }

    fn set_columns(&mut self, columns: Vec<String>) {
        self.bytes = format::json_len(&json!(columns));
        self.columns = columns;
    }

    /// Add a row; `false` once no more fit (by count or size).
    fn push(&mut self, row: Vec<Value>) -> bool {
        if self.rows.len() >= self.max_rows {
            // Core already stops at `max_rows`; this only guards the contract.
            self.truncated = true;
            return false;
        }
        let mut cut = 0;
        let row = Json::Array(
            row.iter()
                .map(|v| {
                    let (cell, was_cut) = format::cell(v);
                    cut += usize::from(was_cut);
                    cell
                })
                .collect(),
        );
        let size = format::json_len(&row) + 1;
        if self.bytes + size > MAX_RESULT_BYTES {
            self.too_big = true;
            return false;
        }
        self.bytes += size;
        self.cut_cells += cut;
        self.rows.push(row);
        true
    }

    fn into_json(self) -> Json {
        let truncated = self.truncated || self.too_big;
        let row_count = self.rows.len();
        let mut out = json!({
            "columns": self.columns,
            "rows": self.rows,
            "rowCount": row_count,
            "truncated": truncated,
        });
        let mut notes = Vec::new();
        if self.too_big {
            notes.push(if row_count == 0 {
                format!(
                    "No rows are shown: the first row alone would take the result past the \
                     {} MB limit. Select fewer or narrower columns.",
                    MAX_RESULT_BYTES / (1024 * 1024)
                )
            } else {
                format!(
                    "Only the first {row_count} rows are shown: the next would take the result \
                     past the {} MB limit. Select fewer or narrower columns, or fewer rows.",
                    MAX_RESULT_BYTES / (1024 * 1024)
                )
            });
        } else if self.truncated && row_count < self.max_rows {
            // The driver stopped at its byte budget, before `max_rows`.
            notes.push(format!(
                "Only the first {row_count} rows are shown: the rows fetched reached the {} MB \
                 limit on what a query may read. Select fewer or narrower columns, or fewer \
                 rows.",
                MAX_FETCH_BYTES / (1024 * 1024)
            ));
        } else if self.truncated {
            notes.push(format!(
                "Only the first {} rows are shown; the query returned more. Narrow it or raise \
                 max_rows (at most {MAX_MAX_ROWS}).",
                self.max_rows
            ));
        }
        if self.cut_cells > 0 {
            notes.push(format!(
                "{} cell(s) were longer than {} KB and are cut: each is an object \
                 {{\"truncated\": true, \"bytes\": <full length>, \"text\": <its start>}}. \
                 Select a substring to see more.",
                self.cut_cells,
                format::MAX_CELL_BYTES / 1024
            ));
            out["truncatedCells"] = json!(self.cut_cells);
        }
        if !notes.is_empty() {
            out["message"] = json!(notes.join(" "));
        }
        out
    }
}

pub(crate) async fn explain_query(inner: &Inner, args: ExplainArgs) -> Result<Json, ToolError> {
    let c = inner.resolve(&args.connection)?;
    require_data(inner, c).await?;
    // Core's read-only EXPLAIN: the same token check as `run_query`, one
    // statement only (`READ_ONLY` otherwise), never ANALYZE, and planning
    // inside the engine's read-only transaction where it can run user code.
    // The call timeout also goes to the database, as a backstop for
    // `timed`'s deadline; either way the error is `TIMEOUT`.
    let timeout = inner.options.call_timeout;
    let result = inner
        .timed(async {
            let id = inner.connection(c).await?;
            Ok(inner
                .workspace
                .engine(&inner.core, &id)?
                .explain_read_only(&args.sql, Vec::new(), Some(timeout))
                .await?)
        })
        .await?;
    let mut plan = format::explain_text(&result);
    if plan.len() > MAX_RESULT_BYTES {
        let mut end = MAX_RESULT_BYTES;
        while !plan.is_char_boundary(end) {
            end -= 1;
        }
        plan.truncate(end);
        return Ok(json!({
            "plan": plan,
            "truncated": true,
            "message": format!(
                "The plan is cut at {} MB.",
                MAX_RESULT_BYTES / (1024 * 1024)
            ),
        }));
    }
    Ok(json!({ "plan": plan }))
}
