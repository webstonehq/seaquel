//! What each tool answers, from what Core read: MCP's JSON,
//! within the profile's budget. Cells and plans are [`super::format`]'s
//! (MCP's `format.rs`, moved), re-exported here.
//!
//! **Budgets.** MCP keeps its 4 MB for query results and plans, and lists
//! everything else whole, as before. The assistant's every result fits in
//! 256 KB ([`ASSISTANT_RESULT_BYTES`]), with its notes worded in KB:
//! `run_query` and `run_saved_query` keep the rows that fit,
//! `explain_query` cuts the plan's text, and `list_tables`,
//! `describe_table` and `list_saved_queries` keep their leading whole items
//! with `"truncated": true` and a message.

pub use super::format::*;

use seaquel_types::storage::PersistedSavedQuery;
use seaquel_types::{ExplainResult, SchemaColumn, SchemaIndex, SchemaTable, Value};
use serde_json::{json, Value as Json};

use super::saved::{AMBIGUOUS_SAVED_QUERY, SAVED_QUERY_NOT_FOUND};
use super::{Profile, Tool, ToolError, ToolOutput, INVALID_ARGUMENT};
use crate::limits::{ASSISTANT_RESULT_BYTES, MAX_FETCH_BYTES, MAX_MAX_ROWS, MCP_RESULT_BYTES};

pub const TABLE_NOT_FOUND: &str = "TABLE_NOT_FOUND";
pub const AMBIGUOUS_TABLE: &str = "AMBIGUOUS_TABLE";

impl Profile {
    /// About the most bytes a result's JSON may take.
    pub fn result_bytes(self) -> usize {
        match self {
            Profile::Mcp => MCP_RESULT_BYTES,
            Profile::Assistant => ASSISTANT_RESULT_BYTES,
        }
    }

    /// The budget as its notes say it: MCP's in MB as before, the
    /// assistant's in KB (never "0 MB").
    fn result_size(self) -> String {
        match self {
            Profile::Mcp => format!("{} MB", MCP_RESULT_BYTES / (1024 * 1024)),
            Profile::Assistant => format!("{} KB", ASSISTANT_RESULT_BYTES / 1024),
        }
    }

    /// The budget listings are cut at: none for MCP, which lists whole as
    /// before.
    fn list_budget(self) -> Option<usize> {
        match self {
            Profile::Mcp => None,
            Profile::Assistant => Some(ASSISTANT_RESULT_BYTES),
        }
    }
}

/// Room a cut listing keeps for its `truncated` flag and message, so the
/// whole result stays within the budget.
const LIST_NOTE_ROOM: usize = 512;

/// A query result being built, within its row limit and the profile's
/// byte budget: columns once, rows as arrays, cells cut at
/// [`MAX_CELL_BYTES`]. Rows are formatted as they arrive, so the caller
/// drops the driver's values one row at a time.
pub struct Rows {
    profile: Profile,
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
    pub fn new(profile: Profile, max_rows: usize) -> Self {
        Self {
            profile,
            max_rows,
            columns: Vec::new(),
            rows: Vec::new(),
            bytes: 0,
            truncated: false,
            too_big: false,
            cut_cells: 0,
        }
    }

    pub fn set_columns(&mut self, columns: Vec<String>) {
        self.bytes = json_len(&json!(columns));
        self.columns = columns;
    }

    /// The driver stopped early (`StreamBatch::truncated`).
    pub fn mark_truncated(&mut self) {
        self.truncated = true;
    }

    /// Add a row; `false` once no more fit (by count or size).
    pub fn push(&mut self, row: &[Value]) -> bool {
        // Once a row didn't fit, the rows shown stay a prefix of the result.
        if self.too_big {
            return false;
        }
        if self.rows.len() >= self.max_rows {
            // Core already stops at `max_rows`; this only guards the contract.
            self.truncated = true;
            return false;
        }
        let mut cut = 0;
        let row = Json::Array(
            row.iter()
                .map(|v| {
                    let (cell, was_cut) = cell(v);
                    cut += usize::from(was_cut);
                    cell
                })
                .collect(),
        );
        let size = json_len(&row) + 1;
        if self.bytes + size > self.profile.result_bytes() {
            self.too_big = true;
            return false;
        }
        self.bytes += size;
        self.cut_cells += cut;
        self.rows.push(row);
        true
    }

    pub fn into_json(self) -> Json {
        let truncated = self.truncated || self.too_big;
        let row_count = self.rows.len();
        let mut out = json!({
            "columns": self.columns,
            "rows": self.rows,
            "rowCount": row_count,
            "truncated": truncated,
        });
        let size = self.profile.result_size();
        let mut notes = Vec::new();
        if self.too_big {
            notes.push(if row_count == 0 {
                format!(
                    "No rows are shown: the first row alone would take the result past the \
                     {size} limit. Select fewer or narrower columns."
                )
            } else {
                format!(
                    "Only the first {row_count} rows are shown: the next would take the result \
                     past the {size} limit. Select fewer or narrower columns, or fewer rows."
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
                MAX_CELL_BYTES / 1024
            ));
            out["truncatedCells"] = json!(self.cut_cells);
        }
        if !notes.is_empty() {
            out["message"] = json!(notes.join(" "));
        }
        out
    }
}

/// `explain_query`'s answer: the plan as text, cut at the profile's budget
/// on a character boundary.
pub fn explain(profile: Profile, result: &ExplainResult) -> Json {
    let mut plan = explain_text(result);
    let max = profile.result_bytes();
    if plan.len() > max {
        let mut end = max;
        while !plan.is_char_boundary(end) {
            end -= 1;
        }
        plan.truncate(end);
        return json!({
            "plan": plan,
            "truncated": true,
            "message": format!("The plan is cut at {}.", profile.result_size()),
        });
    }
    json!({ "plan": plan })
}

/// `list_schemas`' answer.
pub fn schemas(schemas: &[String]) -> Json {
    json!({ "schemas": schemas })
}

fn kind(t: &SchemaTable) -> Json {
    serde_json::to_value(t.kind).unwrap_or(Json::Null)
}

/// Items in order while they fit in `budget` after `base` bytes (each costs
/// its JSON and a comma). Returns how many fit.
fn fitting<'a>(items: impl IntoIterator<Item = &'a Json>, base: usize, budget: usize) -> usize {
    let mut used = base;
    let mut n = 0;
    for item in items {
        used += json_len(item) + 1;
        if used > budget {
            break;
        }
        n += 1;
    }
    n
}

/// `list_tables`' answer: every table (of `schema`, when given), with its
/// approximate row count when the engine knows it.
pub fn tables(profile: Profile, tables: &[SchemaTable], schema: Option<&str>) -> Json {
    let mut list: Vec<Json> = tables
        .iter()
        .filter(|t| schema.is_none_or(|s| s == t.schema))
        .map(|t| {
            let mut entry = json!({ "schema": t.schema, "name": t.name, "type": kind(t) });
            if let Some(rows) = t.row_count {
                entry["approxRows"] = json!(rows);
            }
            entry
        })
        .collect();
    let Some(budget) = profile.list_budget() else {
        return json!({ "tables": list });
    };
    let total = list.len();
    let base = json_len(&json!({ "tables": [] })) + LIST_NOTE_ROOM;
    let shown = fitting(&list, base, budget);
    if shown == total {
        return json!({ "tables": list });
    }
    list.truncate(shown);
    json!({
        "tables": list,
        "truncated": true,
        "message": format!(
            "Only the first {shown} of {total} tables are shown: the next would take the result \
             past the {} limit. Pass `schema` to list one schema's tables.",
            profile.result_size()
        ),
    })
}

/// The listed table `describe_table` describes: the one named `table` (in
/// `schema`, when given). `TABLE_NOT_FOUND` when none is, and
/// `AMBIGUOUS_TABLE` when the name is in several schemas and none was given.
pub fn find_table<'a>(
    tables: &'a [SchemaTable],
    schema: Option<&str>,
    table: &str,
) -> Result<&'a SchemaTable, ToolError> {
    let matches: Vec<&SchemaTable> = tables
        .iter()
        .filter(|t| t.name == table && schema.is_none_or(|s| s == t.schema))
        .collect();
    match matches.as_slice() {
        [t] => Ok(t),
        [] => Err(ToolError::new(
            TABLE_NOT_FOUND,
            match schema {
                Some(s) => format!("No table {table:?} in schema {s:?}"),
                None => format!("No table {table:?}"),
            },
        )),
        many => Err(ToolError::new(
            AMBIGUOUS_TABLE,
            format!(
                "Tables named {table:?} exist in several schemas ({}); pass `schema`",
                many.iter()
                    .map(|t| format!("{:?}", t.schema))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// `describe_table`'s answer: `table` as listed (its schema and kind), the
/// name the call gave, and the columns and indexes `table_metadata` read.
/// The assistant keeps the leading whole items that fit: columns, then
/// indexes, then foreign keys.
pub fn describe(
    profile: Profile,
    table: &SchemaTable,
    name: &str,
    columns: &[SchemaColumn],
    indexes: &[SchemaIndex],
) -> Json {
    let mut foreign_keys: Vec<Json> = columns
        .iter()
        .filter_map(|col| {
            col.foreign_key_ref.as_ref().map(|r| {
                json!({
                    "column": col.name,
                    "referencedSchema": r.referenced_schema,
                    "referencedTable": r.referenced_table,
                    "referencedColumn": r.referenced_column,
                })
            })
        })
        .collect();
    let mut columns: Vec<Json> = columns
        .iter()
        .map(|col| {
            json!({
                "name": col.name,
                "type": col.ty,
                "nullable": col.nullable,
                "default": col.default_value,
                "primaryKey": col.is_primary_key,
            })
        })
        .collect();
    let mut indexes: Vec<Json> = indexes
        .iter()
        .map(|i| json!({ "name": i.name, "columns": i.columns, "unique": i.unique, "type": i.ty }))
        .collect();
    let mut out = json!({
        "schema": table.schema,
        "table": name,
        "type": kind(table),
        "columns": [],
        "indexes": [],
        "foreignKeys": [],
    });
    let Some(budget) = profile.list_budget() else {
        out["columns"] = Json::Array(columns);
        out["indexes"] = Json::Array(indexes);
        out["foreignKeys"] = Json::Array(foreign_keys);
        return out;
    };
    let total = columns.len() + indexes.len() + foreign_keys.len();
    let base = json_len(&out) + LIST_NOTE_ROOM;
    let shown = fitting(
        columns.iter().chain(&indexes).chain(&foreign_keys),
        base,
        budget,
    );
    let mut left = shown;
    for list in [&mut columns, &mut indexes, &mut foreign_keys] {
        let keep = left.min(list.len());
        list.truncate(keep);
        left -= keep;
    }
    out["columns"] = Json::Array(columns);
    out["indexes"] = Json::Array(indexes);
    out["foreignKeys"] = Json::Array(foreign_keys);
    if shown < total {
        out["truncated"] = json!(true);
        out["message"] = json!(format!(
            "Only the first {shown} of the table's {total} columns, indexes and foreign keys are \
             shown: the next would take the result past the {} limit.",
            profile.result_size()
        ));
    }
    out
}

/// `list_saved_queries`' answer: the entries ([`super::saved::describe`]),
/// and, when some were left out for sharing, how many. The assistant keeps
/// the leading whole entries that fit.
pub fn saved_queries(profile: Profile, mut entries: Vec<Json>, hidden: usize) -> Json {
    let mut notes = Vec::new();
    let mut truncated = false;
    if let Some(budget) = profile.list_budget() {
        let total = entries.len();
        let base = json_len(&json!({ "savedQueries": [] })) + LIST_NOTE_ROOM;
        let shown = fitting(&entries, base, budget);
        if shown < total {
            entries.truncate(shown);
            truncated = true;
            notes.push(format!(
                "Only the first {shown} of {total} saved queries are shown: the next would take \
                 the result past the {} limit.",
                profile.result_size()
            ));
        }
    }
    if hidden > 0 {
        notes.push(format!(
            "{hidden} saved queries are not listed: no exposed connection of their project \
             shares its schema with AI tools. The user can turn schema sharing on in the \
             Seaquel app (the connection's AI settings, or Settings > AI for the default)."
        ));
    }
    let mut out = json!({ "savedQueries": entries });
    if truncated {
        out["truncated"] = json!(true);
    }
    if !notes.is_empty() {
        out["message"] = json!(notes.join(" "));
    }
    out
}

/// The saved query `wanted` names among a project's: by id, else by exact
/// name. `SAVED_QUERY_NOT_FOUND` names the project and the connection;
/// `AMBIGUOUS_SAVED_QUERY` lists the ids that share the name.
pub fn find_saved_query<'a>(
    queries: &'a [PersistedSavedQuery],
    wanted: &str,
    project: &str,
    connection: &str,
) -> Result<&'a PersistedSavedQuery, ToolError> {
    if let Some(q) = queries.iter().find(|q| q.id == wanted) {
        return Ok(q);
    }
    let named: Vec<&PersistedSavedQuery> = queries.iter().filter(|q| q.name == wanted).collect();
    match named.as_slice() {
        [q] => Ok(q),
        [] => Err(ToolError::new(
            SAVED_QUERY_NOT_FOUND,
            format!(
                "No saved query {wanted:?} in project {project:?}, the project of connection \
                 {connection:?}"
            ),
        )),
        many => Err(ToolError::new(
            AMBIGUOUS_SAVED_QUERY,
            format!(
                "{} saved queries are named {wanted:?}; pass one of their ids: {}",
                many.len(),
                many.iter()
                    .map(|q| format!("{:?}", q.id))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// A client tool's result from the page's answer: the page's
/// text, an error when it is an object with `error`. Without schema
/// sharing, `get_dashboard`'s widgets lose their `query`,
/// re-serialized with sorted keys; an answer that doesn't parse (or nests
/// past serde_json's limit) can't be stripped, so it becomes an error and
/// none of it is sent. Every result is cut at 256 KB on a character
/// boundary, with a note. The wire marks an error itself, so no prefix is
/// added here.
pub fn client_result(tool: Tool, answer: &str, share_schema: bool) -> ToolOutput {
    let parsed = serde_json::from_str::<Json>(answer).ok();
    let is_error = parsed
        .as_ref()
        .is_some_and(|p| p.is_object() && p.get("error").is_some());
    if tool == Tool::GetDashboard && !share_schema && !is_error {
        let Some(mut parsed) = parsed else {
            return ToolOutput::error(&ToolError::new(
                INVALID_ARGUMENT,
                "The page's answer to get_dashboard isn't valid JSON.",
            ));
        };
        if let Some(widgets) = parsed.get_mut("widgets").and_then(Json::as_array_mut) {
            for w in widgets.iter_mut().filter_map(Json::as_object_mut) {
                w.remove("query");
            }
        }
        return ToolOutput {
            text: cut_text(parsed.to_string(), ASSISTANT_RESULT_BYTES),
            is_error: false,
        };
    }
    ToolOutput {
        text: cut_text(answer.to_string(), ASSISTANT_RESULT_BYTES),
        is_error,
    }
}

/// `text` within `max` bytes: as it is, or its start (on a character
/// boundary) and a note naming its full length.
fn cut_text(mut text: String, max: usize) -> String {
    if text.len() <= max {
        return text;
    }
    let full = text.len();
    let note = format!("\n(cut at {} KB of {full} bytes)", max / 1024);
    let mut end = max - note.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(&note);
    text
}
