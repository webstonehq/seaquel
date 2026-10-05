//! Browse in `update`: a table opened from panel 2, its page
//! through Core's `table_page`, its metadata (`table_metadata`, and the
//! approximate DDL Core builds from it), the cell cursor, the `/` filter
//! over the loaded page, the `F` server-side filter, sort, paging, and the
//! grid's staging keys: `e` edits a cell, `d` stages a delete, `a` an
//! insert, `D` Set default, `u` undoes. Pure: every Core call is an
//! [`Effect`], every answer a [`Msg`].
//!
//! **The TUI builds no SQL.** The page is a typed `TableQuery` (Core writes
//! the SELECT); an edit is an `Edit` whose key is the row's primary-key
//! columns, in the metadata's order, with their values as loaded; Core
//! plans it (`plan_edits`) and the command log shows Core's SQL. A key Core
//! refuses (a keyless table, a view) comes back `NOT_EDITABLE`, and its
//! message is what the TUI says. Typed values go in as text (the cell wire
//! format's `Text`); Core casts them per the metadata, as the GUI's edits.
//!
//! Edits are always staged: the GUI's pending-changes setting
//! doesn't apply here.

use std::fmt;

use seaquel_core::domain::edits::{
    Filter, FilterLogic, FilterOp, PlannedChange, Sort, SortDirection, TableQuery, TableTarget,
};
use seaquel_core::Value;
use seaquel_types::{SchemaColumn, SchemaIndex};

use super::app::{Effect, Model, Panel, Stamp, TablesTab};
use super::dialogs::CallError;
use super::grid::{self, Page};
use super::log::{LogLine, Tag};
use super::panels::{Row, TableKind};
use super::pending::{Outcome, PlanRequest, Planned, RowValues, Staging};
use super::text;

/// The rows a page holds by default (the GUI's page size).
pub const DEFAULT_PAGE_SIZE: u32 = 100;
/// The most `--page-size` may ask for (Core's cap is one below
/// `max_query_rows()`, 100,000 by default; Core refuses past its own).
pub const MAX_PAGE_SIZE: u32 = 99_999;

/// `--page-size`, within 1 and [`MAX_PAGE_SIZE`]; 100 when not given.
pub fn page_size(arg: Option<u32>) -> u32 {
    arg.unwrap_or(DEFAULT_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)
}

/// The operators `F` offers, in the order the arrows cycle them.
pub const FILTER_OPS: [FilterOp; 12] = [
    FilterOp::Eq,
    FilterOp::Ne,
    FilterOp::Gt,
    FilterOp::Lt,
    FilterOp::Ge,
    FilterOp::Le,
    FilterOp::Like,
    FilterOp::NotLike,
    FilterOp::In,
    FilterOp::NotIn,
    FilterOp::IsNull,
    FilterOp::IsNotNull,
];

/// An operator as the form and the filter line show it (the generated
/// `FilterOp`'s text).
pub fn op_text(op: FilterOp) -> &'static str {
    match op {
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

/// Whether an operator reads the value.
pub fn op_takes_value(op: FilterOp) -> bool {
    !matches!(op, FilterOp::IsNull | FilterOp::IsNotNull)
}

/// The table the grid shows.
#[derive(Clone, PartialEq, Eq)]
pub struct Opened {
    pub target: TableTarget,
    pub kind: TableKind,
    /// Core's connection the page and plans run on.
    pub core_id: String,
}

impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Opened")
            .field("kind", &self.kind)
            .field("core_id", &self.core_id)
            .finish_non_exhaustive()
    }
}

/// A table's metadata as Core read it, and the approximate DDL Core built
/// from it (the DDL tab; `create_table` over the columns, indexes and foreign keys).
#[derive(Clone, PartialEq)]
pub struct TableMeta {
    pub columns: Vec<SchemaColumn>,
    pub indexes: Vec<SchemaIndex>,
    pub ddl: Result<String, CallError>,
}

impl fmt::Debug for TableMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TableMeta")
            .field("columns", &self.columns.len())
            .field("indexes", &self.indexes.len())
            .field("ddl", &self.ddl.as_ref().map(String::len))
            .finish()
    }
}

impl TableMeta {
    /// The primary key's columns, in the metadata's order.
    pub fn primary_key(&self) -> Vec<&str> {
        self.columns
            .iter()
            .filter(|c| c.is_primary_key)
            .map(|c| c.name.as_str())
            .collect()
    }

    pub fn column(&self, name: &str) -> Option<&SchemaColumn> {
        self.columns.iter().find(|c| c.name == name)
    }
}

/// The metadata's state.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Meta {
    #[default]
    Idle,
    Loading,
    Loaded(TableMeta),
    Failed(CallError),
}

/// A row of the grid: a staged insert (by entry id) or a page row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GridRow {
    Insert(String),
    Page(usize),
}

/// A cell being edited.
#[derive(Clone, PartialEq, Eq)]
pub struct CellEdit {
    pub row: GridRow,
    pub column: String,
    pub text: String,
    /// The text it started with: Enter on it unchanged stages nothing.
    pub start: String,
}

impl fmt::Debug for CellEdit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CellEdit")
            .field("row", &self.row)
            .field("text_len", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// The `F` form's fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormField {
    Column,
    Op,
    Value,
}

/// The `F` form: a column, an operator and a value.
#[derive(Clone, PartialEq, Eq)]
pub struct FilterForm {
    pub field: FormField,
    /// Into the grid's columns.
    pub column: usize,
    /// Into [`FILTER_OPS`].
    pub op: usize,
    pub value: String,
}

impl fmt::Debug for FilterForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FilterForm")
            .field("field", &self.field)
            .field("op", &FILTER_OPS[self.op])
            .field("value_len", &self.value.len())
            .finish_non_exhaustive()
    }
}

/// What the grid keeps per page (M1): each column's width over the whole
/// page (staged values included), whether it lines up on the right, and
/// the page rows the `/` filter keeps. [`refresh`] rebuilds it when the
/// page, the filter, the queue or the connection changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridCache {
    key: Option<(u64, String, u64, bool, bool)>,
    pub widths: Vec<usize>,
    pub right: Vec<bool>,
    pub visible: Vec<usize>,
}

/// Browse's state.
#[derive(Clone, Default)]
pub struct Browse {
    pub opened: Option<Opened>,
    pub meta: Meta,
    /// The page shown (kept while the next one loads).
    pub page: Option<Page>,
    /// The page call in flight.
    pub loading: Option<u64>,
    /// The last page call failed.
    pub failed: Option<CallError>,
    pub next_op: u64,
    /// The page asked for (1-based).
    pub page_no: u32,
    /// The server-side filter (`F`) and sort (`s`).
    pub filter: Option<Filter>,
    pub sort: Option<Sort>,
    /// The cell cursor, into [`rows`] and [`columns`].
    pub row: usize,
    pub col: usize,
    /// The `/` filter over the loaded page.
    pub find: String,
    pub finding: bool,
    pub editing: Option<CellEdit>,
    pub form: Option<FilterForm>,
    /// Moves on with every page shown.
    pub page_gen: u64,
    pub cache: GridCache,
}

impl fmt::Debug for Browse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Browse")
            .field("opened", &self.opened)
            .field("meta", &self.meta)
            .field("page", &self.page)
            .field("loading", &self.loading)
            .field("page_no", &self.page_no)
            .field("filter", &self.filter)
            .field("sort", &self.sort)
            .field("row", &self.row)
            .field("col", &self.col)
            .field("finding", &self.finding)
            .field("editing", &self.editing)
            .field("form", &self.form)
            .finish_non_exhaustive()
    }
}

/// A page for the runtime to read (`table_page`).
#[derive(Debug, Clone, PartialEq)]
pub struct PageCall {
    pub op: u64,
    pub core_id: String,
    pub query: TableQuery,
    pub page: u32,
    pub page_size: u32,
}

/// A plan for the runtime to ask Core for (`plan_edits`).
#[derive(Debug, Clone, PartialEq)]
pub struct PlanCall {
    pub core_id: String,
    pub request: PlanRequest,
}

/// The table panel 2 has selected, if it's a table or view.
pub fn selected_target(model: &Model) -> Option<(TableTarget, TableKind)> {
    let Some(Row::Item(i)) = model.selected_table_row() else {
        return None;
    };
    let item = model.schema.get(i)?;
    Some((
        TableTarget {
            schema: item.schema.clone(),
            table: item.name.clone(),
        },
        item.kind,
    ))
}

/// Whether the queue's overlays apply here: it holds nothing, or it
/// belongs to the connection panel 1 has (I3).
pub fn here(model: &Model) -> bool {
    model
        .queue
        .connection()
        .is_none_or(|c| Some(c) == model.conn.id())
}

type CacheKey = (u64, String, u64, bool, bool);

fn cache_key(model: &Model) -> CacheKey {
    (
        model.browse.page_gen,
        model.browse.find.clone(),
        model.queue.generation(),
        here(model),
        matches!(model.browse.meta, Meta::Loaded(_)),
    )
}

/// The grid's cache as it should be now: the kept one while it's fresh,
/// else worked out (the view reads this, so a model changed outside
/// `update` still draws right).
pub fn cache(model: &Model) -> std::borrow::Cow<'_, GridCache> {
    let key = cache_key(model);
    if model.browse.cache.key.as_ref() == Some(&key) {
        return std::borrow::Cow::Borrowed(&model.browse.cache);
    }
    let Some(page) = &model.browse.page else {
        return std::borrow::Cow::Owned(GridCache {
            key: Some(key),
            ..GridCache::default()
        });
    };
    let visible = grid::visible(page, &model.browse.find);
    let names = columns(model);
    let types: Vec<String> = names.iter().map(|c| column_type(model, c)).collect();
    let right = names
        .iter()
        .enumerate()
        .map(|(i, _)| {
            page.rows
                .iter()
                .filter_map(|r| r.get(i))
                .find(|v| !matches!(v, Value::Null))
                .is_some_and(grid::right_aligned)
        })
        .collect();
    // Widths over the whole page (and the table's inserts), staged values
    // included, so paging through the `/` filter doesn't move columns.
    let mut all: Vec<GridRow> = Vec::new();
    if let Some(opened) = &model.browse.opened {
        if here(model) {
            all.extend(
                model
                    .queue
                    .inserts(&opened.target)
                    .map(|e| GridRow::Insert(e.id.clone())),
            );
        }
    }
    all.extend((0..page.rows.len()).map(GridRow::Page));
    let cells: Vec<Vec<String>> = all
        .iter()
        .map(|r| {
            names
                .iter()
                .map(|c| match cell_value(model, r, c) {
                    Some(v) => grid::display(&v),
                    None => text::DEFAULT_CELL.to_string(),
                })
                .collect()
        })
        .collect();
    let clean_names: Vec<String> = names.iter().map(|n| grid::clean(n)).collect();
    let clean_types: Vec<String> = types.iter().map(|t| grid::clean(t)).collect();
    std::borrow::Cow::Owned(GridCache {
        key: Some(key),
        widths: grid::column_widths(&clean_names, &clean_types, &cells),
        right,
        visible,
    })
}

/// Rebuilds the grid's cache when what it depends on changed (`update`
/// calls it after every message).
pub fn refresh(model: &mut Model) {
    if let std::borrow::Cow::Owned(fresh) = cache(model) {
        model.browse.cache = fresh;
    }
}

/// The GUI's `editedCellValue` (`cell-type.ts`), after the NULL rule: hex
/// (`\x…`) is bytes where the cell held bytes, or in a binary column on
/// MySQL and MariaDB (whose strings are bytes); hex that doesn't parse, and
/// anything else, is text.
pub fn edited_value(
    original: Option<&Value>,
    column_type: &str,
    engine: &str,
    text: &str,
) -> Value {
    let value = typed_value(text);
    let Value::Text(typed) = &value else {
        return value;
    };
    let holds_bytes = matches!(original, Some(Value::Bytes(_)))
        || (binary_column(column_type) && matches!(engine, "mysql" | "mariadb"));
    if holds_bytes {
        if let Some(bytes) = from_hex(typed) {
            return Value::Bytes(bytes);
        }
    }
    value
}

/// `cellTypeFromColumnType`'s binary rule.
fn binary_column(ty: &str) -> bool {
    let lower = ty.to_lowercase();
    let base = lower.split('(').next().unwrap_or("").trim();
    matches!(
        base,
        "bytea"
            | "blob"
            | "tinyblob"
            | "mediumblob"
            | "longblob"
            | "varbinary"
            | "binary"
            | "image"
    )
}

/// `fromHex` (`values.ts`): `\x` and pairs of hex digits.
fn from_hex(text: &str) -> Option<Vec<u8>> {
    let hex = text.strip_prefix("\\x")?;
    if hex.len() % 2 != 0 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// The engine of the connection panel 1 has.
pub(crate) fn engine(model: &Model) -> String {
    model
        .conn
        .id()
        .and_then(|id| model.library.connection(id))
        .map(|c| c.engine.clone())
        .unwrap_or_default()
}

/// Ties the queue to panel 1's connection before a staging, or says why
/// not.
fn bind(model: &mut Model) -> Result<(), Vec<Effect>> {
    super::commit::idle(model)?;
    let Some(id) = model.conn.id().map(str::to_string) else {
        return Err(vec![Model::log_effect(
            Some(Tag::Error),
            text::NOT_CONNECTED_LOG,
        )]);
    };
    match model.queue.bind(&id) {
        Ok(()) => Ok(()),
        Err(other) => Err(queue_elsewhere(model, &other)),
    }
}

/// **Task 5's hook:** a staging on a connection other than the queue's is
/// refused, the log says where the changes are, and Task 5's question asks
/// to keep or discard them.
pub fn queue_elsewhere(model: &mut Model, other: &str) -> Vec<Effect> {
    let name = model
        .library
        .connection(other)
        .map_or(other, |c| c.name.as_str());
    let mut effects = vec![Model::log_effect(
        Some(Tag::Error),
        text::queue_elsewhere(model.queue.entries().len(), name),
    )];
    effects.extend(super::commit::ask_switch(model, other, None));
    effects
}

/// Whether the main view shows the opened table (its Data or another tab).
pub fn shows_opened(model: &Model) -> bool {
    model.ctx == Panel::Tables
        && match (&model.browse.opened, selected_target(model)) {
            (Some(opened), Some((target, _))) => opened.target == target,
            _ => false,
        }
}

/// Whether the main view shows the grid now.
pub fn shows_grid(model: &Model) -> bool {
    shows_opened(model) && model.main_tab_index() == 0
}

/// The grid's columns: the page's, else the metadata's.
pub fn columns(model: &Model) -> Vec<String> {
    if let Some(page) = &model.browse.page {
        return page.columns.clone();
    }
    match &model.browse.meta {
        Meta::Loaded(meta) => meta.columns.iter().map(|c| c.name.clone()).collect(),
        _ => Vec::new(),
    }
}

/// A column's type: the metadata's, else panel 2's.
pub fn column_type(model: &Model, column: &str) -> String {
    if let Meta::Loaded(meta) = &model.browse.meta {
        if let Some(c) = meta.column(column) {
            return c.ty.clone();
        }
    }
    let Some(opened) = &model.browse.opened else {
        return String::new();
    };
    model
        .schema
        .iter()
        .find(|t| t.schema == opened.target.schema && t.name == opened.target.table)
        .and_then(|t| t.columns.iter().find(|(n, _)| n == column))
        .map(|(_, ty)| ty.clone())
        .unwrap_or_default()
}

/// The grid's rows: the table's staged inserts first, then the page rows
/// the `/` filter keeps.
pub fn rows(model: &Model) -> Vec<GridRow> {
    let browse = &model.browse;
    let Some(opened) = &browse.opened else {
        return Vec::new();
    };
    let mut rows: Vec<GridRow> = Vec::new();
    if here(model) {
        rows.extend(
            model
                .queue
                .inserts(&opened.target)
                .map(|e| GridRow::Insert(e.id.clone())),
        );
    }
    if browse.page.is_some() {
        rows.extend(cache(model).visible.iter().copied().map(GridRow::Page));
    }
    rows
}

/// A page row's key: its primary-key columns, in the metadata's order,
/// with their values as loaded. Empty when the table has none (Core then
/// refuses the edit with `NOT_EDITABLE`); `None` while the metadata isn't
/// loaded.
pub fn row_key(model: &Model, row: usize) -> Option<RowValues> {
    let Meta::Loaded(meta) = &model.browse.meta else {
        return None;
    };
    let page = model.browse.page.as_ref()?;
    meta.primary_key()
        .into_iter()
        .map(|c| page.value(row, c).map(|v| (c.to_string(), v.clone())))
        .collect()
}

/// A key as the command log names it: `id 48109`, `a 1 · b 2`.
pub fn key_text(key: &RowValues) -> String {
    if key.is_empty() {
        return "the row".to_string();
    }
    key.iter()
        .map(|(c, v)| format!("{c} {}", grid::display(v)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Opens the table panel 2 has selected (Enter): its metadata and first
/// page load, and the main view takes the focus. The same table again only
/// focuses it.
pub fn open(model: &mut Model) -> Vec<Effect> {
    let Some((target, kind)) = selected_target(model) else {
        return Vec::new();
    };
    model.focus_panel(Panel::Main);
    let Some(core_id) = model.conn.core_id().map(str::to_string) else {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_LOG)];
    };
    let opened = Opened {
        target,
        kind,
        core_id,
    };
    if model.browse.opened.as_ref() == Some(&opened) {
        return Vec::new();
    }
    let next_op = model.browse.next_op;
    model.browse = Browse {
        next_op,
        meta: Meta::Loading,
        ..Browse::default()
    };
    let effect = Effect::LoadMeta {
        core_id: opened.core_id.clone(),
        target: opened.target.clone(),
    };
    model.browse.opened = Some(opened);
    let mut effects = vec![effect];
    effects.extend(load_page(model, 1));
    effects
}

/// Forgets the opened table (the connection went).
pub fn close(model: &mut Model) {
    let next_op = model.browse.next_op;
    model.browse = Browse {
        next_op,
        ..Browse::default()
    };
}

/// Reads page `page_no` of the opened table with its filter and sort.
pub fn load_page(model: &mut Model, page_no: u32) -> Vec<Effect> {
    let Some(opened) = model.browse.opened.clone() else {
        return Vec::new();
    };
    let browse = &mut model.browse;
    browse.next_op += 1;
    let op = browse.next_op;
    browse.loading = Some(op);
    browse.page_no = page_no.max(1);
    vec![Effect::LoadPage(PageCall {
        op,
        core_id: opened.core_id,
        query: TableQuery {
            target: opened.target,
            filters: browse.filter.iter().cloned().collect(),
            logic: FilterLogic::And,
            sort: browse.sort.iter().cloned().collect(),
        },
        page: browse.page_no,
        page_size: model.page_size,
    })]
}

/// Reads the shown page again (after an apply; `r`).
pub fn reload(model: &mut Model) -> Vec<Effect> {
    let page = model.browse.page_no.max(1);
    load_page(model, page)
}

/// Keeps the cursor within the grid.
fn clamp(model: &mut Model) {
    let rows = rows(model).len();
    let cols = columns(model).len();
    let browse = &mut model.browse;
    browse.row = browse.row.min(rows.saturating_sub(1));
    browse.col = browse.col.min(cols.saturating_sub(1));
}

/// `hjkl` and the arrows (the prototype's clamp).
pub fn move_cell(model: &mut Model, dx: isize, dy: isize) {
    let browse = &mut model.browse;
    browse.row = browse.row.saturating_add_signed(dy);
    browse.col = browse.col.saturating_add_signed(dx);
    clamp(model);
}

/// `g`, `G`.
pub fn first_row(model: &mut Model) {
    model.browse.row = 0;
}

pub fn last_row(model: &mut Model) {
    model.browse.row = rows(model).len().saturating_sub(1);
}

/// The grid row and column under the cursor.
fn cursor(model: &Model) -> Option<(GridRow, String)> {
    let row = rows(model).into_iter().nth(model.browse.row)?;
    let column = columns(model).into_iter().nth(model.browse.col)?;
    Some((row, column))
}

/// The value a cell shows now: staged, else as loaded. `None` for a staged
/// Set default or an insert's column left to its default.
pub fn cell_value(model: &Model, row: &GridRow, column: &str) -> Option<Value> {
    let opened = model.browse.opened.as_ref()?;
    match row {
        GridRow::Insert(id) => match &model.queue.entry(id)?.staging {
            Staging::Insert { values } => values
                .iter()
                .find(|(c, _)| c == column)
                .map(|(_, v)| v.clone()),
            _ => None,
        },
        GridRow::Page(i) => {
            let loaded = model.browse.page.as_ref()?.value(*i, column)?.clone();
            if !here(model) {
                return Some(loaded);
            }
            let Some(key) = row_key(model, *i) else {
                return Some(loaded);
            };
            match model
                .queue
                .cell(&opened.target, &key, column)
                .map(|e| &e.staging)
            {
                Some(Staging::Update { value, .. }) => Some(value.clone()),
                Some(Staging::SetDefault { .. }) => None,
                _ => Some(loaded),
            }
        }
    }
}

fn is_primary_key(model: &Model, column: &str) -> bool {
    matches!(&model.browse.meta, Meta::Loaded(meta)
        if meta.column(column).is_some_and(|c| c.is_primary_key))
}

/// `e`/Enter: edits the cell in place. A primary-key column says so in the
/// log and doesn't edit (prototype `startEdit`); a row staged for delete
/// doesn't edit.
pub fn start_edit(model: &mut Model) -> Vec<Effect> {
    let Some((row, column)) = cursor(model) else {
        return Vec::new();
    };
    if let GridRow::Page(i) = row {
        if is_primary_key(model, &column) {
            return vec![Model::log_effect(
                Some(Tag::ReadOnly),
                text::primary_key(&column),
            )];
        }
        if deleted_here(model, i) {
            return vec![Model::log_effect(Some(Tag::ReadOnly), text::ROW_DELETED)];
        }
    }
    let text = match cell_value(model, &row, &column) {
        Some(Value::Null) => "NULL".to_string(),
        Some(v) => escape_null(&grid::cell_text(&v)),
        None => String::new(),
    };
    model.browse.editing = Some(CellEdit {
        row,
        column,
        start: text.clone(),
        text,
    });
    Vec::new()
}

/// Whether page row `i` is staged for delete (on the queue's connection).
fn deleted_here(model: &Model, i: usize) -> bool {
    match (&model.browse.opened, row_key(model, i)) {
        (Some(opened), Some(key)) => here(model) && model.queue.deleted(&opened.target, &key),
        _ => false,
    }
}

/// Whether `text` is `NULL` behind backslashes (`NULL`, `\NULL`, …).
fn null_word(text: &str) -> bool {
    text.trim_start_matches('\\') == "NULL"
}

/// The text a cell holding `text` starts editing with: one more backslash
/// before a `NULL` word, so the text "NULL" isn't NULL (I1).
pub(crate) fn escape_null(text: &str) -> String {
    if null_word(text) {
        format!("\\{text}")
    } else {
        text.to_string()
    }
}

/// What a typed cell means: `NULL` is NULL; a `NULL` word behind
/// backslashes loses one (`\NULL` is the text "NULL"); anything else is
/// its text (Core casts it per the metadata).
pub fn typed_value(text: &str) -> Value {
    if text == "NULL" {
        Value::Null
    } else if null_word(text) {
        Value::Text(text[1..].to_string())
    } else {
        Value::Text(text.to_string())
    }
}

/// A key the edit, find or form text takes.
pub fn type_char(model: &mut Model, c: char) {
    let browse = &mut model.browse;
    if let Some(edit) = &mut browse.editing {
        edit.text.push(c);
    } else if browse.finding {
        browse.find.push(c);
        browse.row = 0;
    } else if let Some(form) = &mut browse.form {
        if form.field == FormField::Value {
            form.value.push(c);
        }
    }
}

pub fn backspace(model: &mut Model) {
    let browse = &mut model.browse;
    if let Some(edit) = &mut browse.editing {
        edit.text.pop();
    } else if browse.finding {
        browse.find.pop();
        browse.row = 0;
    } else if let Some(form) = &mut browse.form {
        if form.field == FormField::Value {
            form.value.pop();
        }
    }
}

/// Whether a text input takes printable keys now.
pub fn typing(model: &Model) -> bool {
    let browse = &model.browse;
    browse.editing.is_some()
        || browse.finding
        || browse
            .form
            .as_ref()
            .is_some_and(|f| f.field == FormField::Value)
}

/// The plans for `requests`, on the queue's own connection when it's open;
/// with none, the entries stay staged, unplanned (planned at commit).
pub(crate) fn plan(model: &mut Model, requests: Vec<PlanRequest>) -> Vec<Effect> {
    let own = model.queue.connection().is_some() && model.queue.connection() == model.conn.id();
    match model.conn.core_id().filter(|_| own).map(str::to_string) {
        Some(core_id) => requests
            .into_iter()
            .map(|request| {
                Effect::PlanEdit(PlanCall {
                    core_id: core_id.clone(),
                    request,
                })
            })
            .collect(),
        None => {
            model.queue.unplan(&requests);
            Vec::new()
        }
    }
}

/// Turns a staging outcome into its plan or log line.
pub(crate) fn staged(model: &mut Model, outcome: Outcome, unstaged: String) -> Vec<Effect> {
    model.staged = model.queue.counts();
    match outcome {
        Outcome::Staged(request) => plan(model, vec![request]),
        Outcome::Unstaged if !unstaged.is_empty() => {
            vec![Model::log_effect(Some(Tag::Unstaged), unstaged)]
        }
        Outcome::Unstaged | Outcome::Nothing => Vec::new(),
    }
}

/// Enter while editing: stages the value (prototype `applyEdit`).
pub fn apply_edit(model: &mut Model) -> Vec<Effect> {
    let Some(edit) = model.browse.editing.take() else {
        return Vec::new();
    };
    // An unchanged Enter stages nothing (the GUI's rule).
    if edit.text == edit.start {
        return Vec::new();
    }
    let Some(opened) = model.browse.opened.clone() else {
        return Vec::new();
    };
    if let Err(effects) = bind(model) {
        return effects;
    }
    let ty = column_type(model, &edit.column);
    let engine = engine(model);
    match edit.row {
        GridRow::Insert(id) => {
            let value = edited_value(None, &ty, &engine, &edit.text);
            let order = columns(model);
            let label = text::undo_edit(&edit.column, "the new row");
            let outcome =
                model
                    .queue
                    .edit_insert(&id, &edit.column, Some(value), &order, label.clone());
            staged(model, outcome, label)
        }
        GridRow::Page(i) => {
            let Some(key) = row_key(model, i) else {
                return vec![Model::log_effect(Some(Tag::Error), text::META_LOADING)];
            };
            let Some(page) = model.browse.page.as_ref() else {
                return Vec::new();
            };
            let Some(original) = page.value(i, &edit.column).cloned() else {
                return Vec::new();
            };
            let value = edited_value(Some(&original), &ty, &engine, &edit.text);
            let row = page.row_values(i);
            let keyed = key_text(&key);
            let label = text::undo_edit(&edit.column, &keyed);
            let outcome = model.queue.edit_cell(
                &opened.target,
                &key,
                &row,
                &edit.column,
                value,
                &original,
                label.clone(),
            );
            staged(model, outcome, label)
        }
    }
}

pub fn cancel_edit(model: &mut Model) {
    model.browse.editing = None;
}

/// `d`: stages or unstages the row's delete; on a staged insert, drops it.
pub fn toggle_delete(model: &mut Model) -> Vec<Effect> {
    let Some((row, _)) = cursor(model) else {
        return Vec::new();
    };
    let Some(opened) = model.browse.opened.clone() else {
        return Vec::new();
    };
    if let Err(effects) = bind(model) {
        return effects;
    }
    match row {
        GridRow::Insert(id) => {
            let label = text::undo_insert(&opened.target.table);
            let outcome = model.queue.drop_insert(&id, label.clone());
            let effects = staged(model, outcome, label);
            clamp(model);
            effects
        }
        GridRow::Page(i) => {
            let Some(key) = row_key(model, i) else {
                return vec![Model::log_effect(Some(Tag::Error), text::META_LOADING)];
            };
            let row = model
                .browse
                .page
                .as_ref()
                .map(|p| p.row_values(i))
                .unwrap_or_default();
            let label = text::undo_delete(&key_text(&key));
            let outcome = model
                .queue
                .toggle_delete(&opened.target, &key, &row, label.clone());
            staged(model, outcome, label)
        }
    }
}

/// `a`: a blank row at the top, its cells editable; the cursor moves to it.
pub fn insert_row(model: &mut Model) -> Vec<Effect> {
    let Some(opened) = model.browse.opened.clone() else {
        return Vec::new();
    };
    if columns(model).is_empty() {
        return Vec::new();
    }
    if let Err(effects) = bind(model) {
        return effects;
    }
    let label = text::undo_insert(&opened.target.table);
    model.queue.add_insert(&opened.target, label);
    model.staged = model.queue.counts();
    model.browse.row = model.queue.inserts(&opened.target).count() - 1;
    model.browse.col = 0;
    Vec::new()
}

/// `D`: stages Set default on the cell; on an insert, the column goes back
/// to its default.
pub fn set_default(model: &mut Model) -> Vec<Effect> {
    let Some((row, column)) = cursor(model) else {
        return Vec::new();
    };
    let Some(opened) = model.browse.opened.clone() else {
        return Vec::new();
    };
    if let Err(effects) = bind(model) {
        return effects;
    }
    match row {
        GridRow::Insert(id) => {
            let order = columns(model);
            let label = text::undo_default(&column, "the new row");
            let outcome = model
                .queue
                .edit_insert(&id, &column, None, &order, label.clone());
            staged(model, outcome, label)
        }
        GridRow::Page(i) => {
            if is_primary_key(model, &column) {
                return vec![Model::log_effect(
                    Some(Tag::ReadOnly),
                    text::primary_key(&column),
                )];
            }
            let Some(key) = row_key(model, i) else {
                return vec![Model::log_effect(Some(Tag::Error), text::META_LOADING)];
            };
            if model.queue.deleted(&opened.target, &key) {
                return vec![Model::log_effect(Some(Tag::ReadOnly), text::ROW_DELETED)];
            }
            let row = model
                .browse
                .page
                .as_ref()
                .map(|p| p.row_values(i))
                .unwrap_or_default();
            let label = text::undo_default(&column, &key_text(&key));
            let outcome =
                model
                    .queue
                    .set_default(&opened.target, &key, &row, &column, label.clone());
            staged(model, outcome, label)
        }
    }
}

/// `u`: takes back the last staging action (prototype `undo`).
pub fn undo(model: &mut Model) -> Vec<Effect> {
    if let Err(effects) = super::commit::idle(model) {
        return effects;
    }
    let Some((label, requests)) = model.queue.undo() else {
        return Vec::new();
    };
    model.staged = model.queue.counts();
    clamp(model);
    let mut effects = vec![Model::log_effect(Some(Tag::Undo), label)];
    effects.extend(plan(model, requests));
    effects
}

/// `/`: starts typing the filter.
pub fn start_find(model: &mut Model) {
    model.browse.finding = true;
}

/// Enter: keeps it.
pub fn apply_find(model: &mut Model) {
    model.browse.finding = false;
}

/// Esc while typing: clears it (prototype `filtering`).
pub fn clear_find(model: &mut Model) {
    model.browse.finding = false;
    model.browse.find.clear();
    model.browse.row = 0;
}

/// `F`: the form, on the current filter or the cursor's column.
pub fn open_form(model: &mut Model) {
    let cols = columns(model);
    if cols.is_empty() {
        return;
    }
    let form = match &model.browse.filter {
        Some(f) => FilterForm {
            field: FormField::Value,
            column: cols.iter().position(|c| *c == f.column).unwrap_or(0),
            op: FILTER_OPS.iter().position(|o| *o == f.op).unwrap_or(0),
            value: f.value.clone(),
        },
        None => FilterForm {
            field: FormField::Column,
            column: model.browse.col.min(cols.len() - 1),
            op: 0,
            value: String::new(),
        },
    };
    model.browse.form = Some(form);
}

/// Tab: the next field (the value only when the operator reads one).
pub fn form_field(model: &mut Model) {
    if let Some(form) = &mut model.browse.form {
        form.field = match form.field {
            FormField::Column => FormField::Op,
            FormField::Op if op_takes_value(FILTER_OPS[form.op]) => FormField::Value,
            FormField::Op | FormField::Value => FormField::Column,
        };
    }
}

/// The arrows: the previous or next column or operator.
pub fn form_step(model: &mut Model, forward: bool) {
    let cols = columns(model).len();
    if let Some(form) = &mut model.browse.form {
        let step = |i: usize, n: usize| {
            if n == 0 {
                0
            } else if forward {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            }
        };
        match form.field {
            FormField::Column => form.column = step(form.column, cols),
            FormField::Op => form.op = step(form.op, FILTER_OPS.len()),
            FormField::Value => {}
        }
    }
}

/// Enter: the filter goes in the query and the first page loads.
pub fn apply_form(model: &mut Model) -> Vec<Effect> {
    let Some(form) = model.browse.form.take() else {
        return Vec::new();
    };
    let Some(column) = columns(model).into_iter().nth(form.column) else {
        return Vec::new();
    };
    let op = FILTER_OPS[form.op];
    model.browse.filter = Some(Filter {
        column,
        op,
        value: if op_takes_value(op) {
            form.value
        } else {
            String::new()
        },
    });
    model.browse.row = 0;
    load_page(model, 1)
}

pub fn cancel_form(model: &mut Model) {
    model.browse.form = None;
}

/// `s`: sorts by the cursor's column, ascending, then descending, then not.
pub fn cycle_sort(model: &mut Model) -> Vec<Effect> {
    let Some(column) = columns(model).into_iter().nth(model.browse.col) else {
        return Vec::new();
    };
    model.browse.sort = match &model.browse.sort {
        Some(s) if s.column == column && s.direction == SortDirection::Asc => Some(Sort {
            column,
            direction: SortDirection::Desc,
        }),
        Some(s) if s.column == column => None,
        _ => Some(Sort {
            column,
            direction: SortDirection::Asc,
        }),
    };
    model.browse.row = 0;
    load_page(model, 1)
}

/// `n`: the next page, when there is one.
pub fn next_page(model: &mut Model) -> Vec<Effect> {
    match &model.browse.page {
        Some(page) if page.has_next() => {
            let next = page.page + 1;
            model.browse.row = 0;
            load_page(model, next)
        }
        _ => Vec::new(),
    }
}

/// `p`: the previous page.
pub fn prev_page(model: &mut Model) -> Vec<Effect> {
    let page = model.browse.page_no;
    if page <= 1 {
        return Vec::new();
    }
    model.browse.row = 0;
    load_page(model, page - 1)
}

/// Esc in the grid: clears the `/` filter, then drops an empty insert under
/// the cursor, then clears the server filter, else goes back to the panel
/// (the prototype's Esc, plus what Task 4 adds).
pub fn back(model: &mut Model) -> Vec<Effect> {
    if !model.browse.find.is_empty() {
        clear_find(model);
        return Vec::new();
    }
    if let Some((GridRow::Insert(id), _)) = cursor(model) {
        let empty = model
            .queue
            .entry(&id)
            .is_some_and(|e| matches!(&e.staging, Staging::Insert { values } if values.is_empty()));
        if empty {
            let label = text::undo_insert(
                &model
                    .browse
                    .opened
                    .as_ref()
                    .map(|o| o.target.table.clone())
                    .unwrap_or_default(),
            );
            model.queue.drop_insert(&id, label);
            model.staged = model.queue.counts();
            clamp(model);
            return Vec::new();
        }
    }
    if model.browse.filter.take().is_some() {
        model.browse.row = 0;
        return load_page(model, 1);
    }
    let ctx = model.ctx;
    model.focus_panel(ctx);
    Vec::new()
}

/// A page answered. A late one (a newer page was asked for) is dropped.
pub fn on_page(
    model: &mut Model,
    op: u64,
    result: Result<Page, CallError>,
    stamp: Stamp,
) -> Vec<Effect> {
    if model.browse.loading != Some(op) {
        return Vec::new();
    }
    model.browse.loading = None;
    match result {
        Ok(page) => {
            model.log.push(LogLine {
                time: stamp.time,
                tag: None,
                text: page.sql.clone(),
                elapsed: Some(grid::elapsed_text(page.elapsed_ms)),
            });
            model.browse.failed = None;
            model.browse.page_no = page.page;
            model.browse.page = Some(page);
            model.browse.page_gen += 1;
            // A page row's index now names another row.
            if matches!(&model.browse.editing, Some(e) if matches!(e.row, GridRow::Page(_))) {
                model.browse.editing = None;
            }
            clamp(model);
        }
        Err(e) => {
            model.log.push(LogLine {
                time: stamp.time,
                tag: Some(Tag::Error),
                text: text::failed_line("table page", &e.code),
                elapsed: None,
            });
            model.browse.failed = Some(e);
        }
    }
    Vec::new()
}

/// The opened table's metadata answered.
pub fn on_meta(
    model: &mut Model,
    core_id: &str,
    target: &TableTarget,
    result: Result<TableMeta, CallError>,
) -> Vec<Effect> {
    let current = model
        .browse
        .opened
        .as_ref()
        .is_some_and(|o| o.core_id == core_id && o.target == *target);
    if !current {
        return Vec::new();
    }
    match result {
        Ok(meta) => {
            // The columns are kept for completion and the preview too.
            super::query::remember_columns(
                model,
                target,
                meta.columns
                    .iter()
                    .map(|c| (c.name.clone(), c.ty.clone()))
                    .collect(),
            );
            model.browse.meta = Meta::Loaded(meta);
            Vec::new()
        }
        Err(e) => {
            let effect = Model::log_effect(
                Some(Tag::Error),
                text::failed_line("table metadata", &e.code),
            );
            model.browse.meta = Meta::Failed(e);
            vec![effect]
        }
    }
}

/// Core's plan for a staged entry: the log shows Core's SQL; a refusal
/// takes the staging back and says Core's message (`NOT_EDITABLE`: a
/// keyless table, a view, a key that isn't the primary key).
pub fn on_planned(
    model: &mut Model,
    id: &str,
    seq: u64,
    result: Result<PlannedChange, CallError>,
) -> Vec<Effect> {
    let outcome = model.queue.planned(id, seq, result);
    model.staged = model.queue.counts();
    match outcome {
        Planned::Stale => Vec::new(),
        Planned::Kept(entry) => {
            let tag = match entry.staging {
                Staging::Delete { .. } => Tag::StagedDelete,
                _ => Tag::Staged,
            };
            let sql = match &entry.plan {
                super::pending::Plan::Planned(plan) => plan.sql.clone(),
                _ => String::new(),
            };
            vec![Model::log_effect(Some(tag), sql)]
        }
        // The edit itself refused: taken back; Core's message says why.
        Planned::Refused(_, e) => {
            clamp(model);
            vec![Model::log_effect(Some(Tag::ReadOnly), e.message)]
        }
        // Anything else: kept, unplanned, and Core's message logged.
        Planned::Failed(_, e) => vec![Model::log_effect(
            Some(Tag::Error),
            text::plan_failed(&e.code, &e.message),
        )],
    }
}

/// The table's tabs in the main view: a view has Data and Columns.
pub fn tab_count(kind: TableKind, tables_tab: TablesTab) -> usize {
    match (kind, tables_tab) {
        (TableKind::Table, TablesTab::Tables) => text::MAIN_TABS_TABLE.len(),
        _ => text::MAIN_TABS_VIEW.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::app::{update, Msg};
    use crate::state::pending::Plan;
    use crate::testing::fixtures::{browsing, invoices, keyless};
    use crate::testing::keys::{key, press};
    use crossterm::event::KeyCode;
    use seaquel_core::domain::edits::Edit;

    fn keys(model: &mut Model, typed: &str) -> Vec<Effect> {
        typed.chars().flat_map(|c| update(model, key(c))).collect()
    }

    fn enter(model: &mut Model) -> Vec<Effect> {
        update(model, press(KeyCode::Enter))
    }

    fn esc(model: &mut Model) -> Vec<Effect> {
        update(model, press(KeyCode::Esc))
    }

    fn plans(effects: &[Effect]) -> Vec<&PlanCall> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::PlanEdit(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    fn logs(effects: &[Effect]) -> Vec<(Option<Tag>, String)> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Log(entry) => Some((entry.tag, entry.text.clone())),
                _ => None,
            })
            .collect()
    }

    fn pages(effects: &[Effect]) -> Vec<&PageCall> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::LoadPage(call) => Some(call),
                _ => None,
            })
            .collect()
    }

    fn column_index(model: &Model, name: &str) -> usize {
        columns(model).iter().position(|c| c == name).unwrap()
    }

    /// The fixture's cursor on 48109's `total` (the design's row 3, col 5).
    fn on_total(model: &mut Model) {
        model.browse.row = 3;
        model.browse.col = column_index(model, "total");
    }

    #[test]
    fn the_page_size_is_100_unless_given_and_within_core_s_cap() {
        assert_eq!(page_size(None), 100);
        assert_eq!(page_size(Some(25)), 25);
        assert_eq!(page_size(Some(1_000_000)), MAX_PAGE_SIZE);
    }

    #[test]
    fn enter_in_panel_two_opens_the_table() {
        let mut m = crate::testing::fixtures::connected(148, 42);
        m.tables.selected = 2; // public.invoices
        let effects = enter(&mut m);
        assert_eq!(m.focus, Panel::Main);
        assert!(effects.iter().any(|e| matches!(e,
            Effect::LoadMeta { target, .. } if *target == invoices())));
        let page = pages(&effects);
        assert_eq!(page.len(), 1);
        assert_eq!(
            (page[0].page, page[0].page_size, page[0].core_id.as_str()),
            (1, 100, "core-1")
        );
        assert_eq!(page[0].query.target, invoices());
        assert!(page[0].query.filters.is_empty() && page[0].query.sort.is_empty());
        assert!(shows_grid(&m));
        // The same table again only focuses it.
        esc(&mut m);
        assert!(enter(&mut m).is_empty());
        assert_eq!(m.focus, Panel::Main);
    }

    // Prototype: `dy`, `left || right`, `g`, `G` in the main view.
    #[test]
    fn hjkl_and_the_arrows_clamp_and_g_jumps() {
        let mut m = browsing(148, 42);
        assert_eq!((m.browse.row, m.browse.col), (0, 0));
        keys(&mut m, "kh");
        assert_eq!((m.browse.row, m.browse.col), (0, 0));
        keys(&mut m, "jjjlll");
        update(&mut m, press(KeyCode::Right));
        assert_eq!((m.browse.row, m.browse.col), (3, 4));
        for _ in 0..20 {
            keys(&mut m, "l");
        }
        assert_eq!(m.browse.col, 6, "seven columns");
        keys(&mut m, "G");
        assert_eq!(m.browse.row, 14, "fifteen rows");
        keys(&mut m, "j");
        assert_eq!(m.browse.row, 14);
        keys(&mut m, "g");
        assert_eq!(m.browse.row, 0);
    }

    // Prototype `startEdit`, the editing keys and `applyEdit`.
    #[test]
    fn e_edits_typing_and_backspace_enter_stages() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        assert!(keys(&mut m, "e").is_empty());
        let edit = m.browse.editing.clone().unwrap();
        assert_eq!(
            (edit.column.as_str(), edit.text.as_str()),
            ("total", "2975.00")
        );
        assert_eq!(m.bar_context(), crate::state::keymap::BarContext::CellEdit);
        assert_eq!(m.mode().as_deref(), Some("EDIT"));
        // Every printable key is text: `q` and `1` don't quit or focus.
        for _ in 0..7 {
            update(&mut m, press(KeyCode::Backspace));
        }
        keys(&mut m, "3150.00q1");
        update(&mut m, press(KeyCode::Backspace));
        update(&mut m, press(KeyCode::Backspace));
        assert_eq!(m.browse.editing.as_ref().unwrap().text, "3150.00");
        let effects = enter(&mut m);
        assert_eq!(m.browse.editing, None);
        let plan = plans(&effects);
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].core_id, "core-1");
        let Edit::UpdateCell {
            target,
            key,
            column,
            value,
        } = &plan[0].request.edit
        else {
            panic!("an update")
        };
        assert_eq!(*target, invoices());
        assert_eq!(*key, [("id".to_string(), Value::Int(48109))]);
        assert_eq!(column, "total");
        assert_eq!(*value, Value::Text("3150.00".into()));
        assert_eq!(m.staged.updates, 1);
        // The cell shows the staged value.
        let row = rows(&m)[3].clone();
        assert_eq!(
            cell_value(&m, &row, "total"),
            Some(Value::Text("3150.00".into()))
        );
    }

    #[test]
    fn esc_cancels_an_edit() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        keys(&mut m, "e9");
        assert!(esc(&mut m).is_empty());
        assert_eq!(m.browse.editing, None);
        assert!(m.queue.is_empty());
        assert_eq!(m.focus, Panel::Main, "esc left the edit, not the grid");
    }

    #[test]
    fn null_is_typed_as_null_and_backslash_null_is_the_text() {
        assert_eq!(typed_value("NULL"), Value::Null);
        assert_eq!(typed_value("\\NULL"), Value::Text("NULL".into()));
        assert_eq!(typed_value("null"), Value::Text("null".into()));
        assert_eq!(typed_value(""), Value::Text(String::new()));

        let mut m = browsing(148, 42);
        m.browse.row = 0;
        m.browse.col = column_index(&m, "paid_at");
        keys(&mut m, "e");
        assert_eq!(m.browse.editing.as_ref().unwrap().text, "NULL");
        // NULL back to NULL: nothing staged.
        assert!(plans(&enter(&mut m)).is_empty());
        assert!(m.queue.is_empty());
        keys(&mut m, "e");
        for _ in 0..4 {
            update(&mut m, press(KeyCode::Backspace));
        }
        keys(&mut m, "\\NULL");
        let effects = enter(&mut m);
        let Edit::UpdateCell { value, .. } = &plans(&effects)[0].request.edit else {
            panic!()
        };
        assert_eq!(*value, Value::Text("NULL".into()));
    }

    // Prototype `applyEdit`: `if (nv === r[ed.k]) delete edits[key]`.
    #[test]
    fn an_edit_back_to_the_original_unstages() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        keys(&mut m, "e0");
        enter(&mut m);
        assert_eq!(m.staged.updates, 1);
        keys(&mut m, "e");
        update(&mut m, press(KeyCode::Backspace));
        let effects = enter(&mut m);
        assert!(m.queue.is_empty());
        assert_eq!(m.staged.updates, 0);
        assert_eq!(
            logs(&effects),
            [(Some(Tag::Unstaged), "edit total · id 48109".to_string())]
        );
    }

    // Prototype: `if (k === 'id')` logs "id is the primary key".
    #[test]
    fn the_primary_key_doesn_t_edit_and_says_so() {
        let mut m = browsing(148, 42);
        let effects = keys(&mut m, "e");
        assert_eq!(m.browse.editing, None);
        assert_eq!(
            logs(&effects),
            [(Some(Tag::ReadOnly), "id is the primary key".to_string())]
        );
        assert_eq!(
            logs(&keys(&mut m, "D")),
            [(Some(Tag::ReadOnly), "id is the primary key".to_string())]
        );
    }

    // Prototype `toggleDel`; a row staged for delete doesn't edit.
    #[test]
    fn d_toggles_the_row_s_delete() {
        let mut m = browsing(148, 42);
        m.browse.row = 6; // 48106
        let effects = keys(&mut m, "d");
        let Edit::DeleteRow { key, .. } = &plans(&effects)[0].request.edit else {
            panic!("a delete")
        };
        assert_eq!(*key, [("id".to_string(), Value::Int(48106))]);
        assert_eq!(m.staged.deletes, 1);
        m.browse.col = 1;
        keys(&mut m, "e");
        assert_eq!(m.browse.editing, None, "a deleted row doesn't edit");
        let effects = keys(&mut m, "d");
        assert!(plans(&effects).is_empty());
        assert_eq!(
            logs(&effects),
            [(Some(Tag::Unstaged), "delete id 48106".to_string())]
        );
        assert_eq!(m.staged.deletes, 0);
    }

    // Prototype `filtering`: typing filters, Enter keeps, Esc clears.
    #[test]
    fn slash_filters_the_page_and_esc_clears_it() {
        let mut m = browsing(148, 42);
        m.browse.row = 5;
        keys(&mut m, "/");
        assert_eq!(m.mode().as_deref(), Some("FILTER"));
        keys(&mut m, "acme");
        assert_eq!(m.browse.row, 0);
        assert_eq!(rows(&m), [GridRow::Page(0)]);
        enter(&mut m);
        assert!(!m.browse.finding);
        assert_eq!(m.browse.find, "acme", "kept");
        keys(&mut m, "/q");
        assert!(rows(&m).is_empty(), "no rows match");
        update(&mut m, press(KeyCode::Backspace));
        assert_eq!(rows(&m).len(), 1);
        esc(&mut m);
        assert_eq!((m.browse.find.as_str(), m.browse.finding), ("", false));
        assert_eq!(rows(&m).len(), 15);
        // Esc in the grid clears a kept filter before going back.
        keys(&mut m, "/hooli");
        enter(&mut m);
        esc(&mut m);
        assert_eq!((m.browse.find.as_str(), m.focus), ("", Panel::Main));
        esc(&mut m);
        assert_eq!(m.focus, Panel::Tables);
    }

    #[test]
    fn n_and_p_page() {
        let mut m = browsing(148, 42);
        m.browse.page.as_mut().unwrap().total_rows = 250;
        m.browse.page.as_mut().unwrap().total_pages = 3;
        assert!(keys(&mut m, "p").is_empty(), "no page before the first");
        let effects = keys(&mut m, "n");
        let page = pages(&effects);
        assert_eq!(page[0].page, 2);
        let op = page[0].op;
        // A stale answer for an older call is dropped.
        let mut late = m.browse.page.clone().unwrap();
        late.page = 7;
        update(
            &mut m,
            Msg::Page {
                op: op - 1,
                result: Ok(late),
                stamp: Stamp::default(),
            },
        );
        assert_eq!(m.browse.page.as_ref().unwrap().page, 1);
        let mut second = m.browse.page.clone().unwrap();
        second.page = 2;
        second.sql = "SELECT page two".into();
        update(
            &mut m,
            Msg::Page {
                op,
                result: Ok(second),
                stamp: Stamp {
                    time: "12:04:02".into(),
                    elapsed_ms: 18,
                },
            },
        );
        assert_eq!(m.browse.page.as_ref().unwrap().page, 2);
        assert_eq!(
            m.log.last(1).next().unwrap().text,
            "SELECT page two",
            "the command log shows Core's page query"
        );
        assert_eq!(pages(&keys(&mut m, "p"))[0].page, 1);
        // On the last page (3 of 3), n does nothing.
        m.browse.page.as_mut().unwrap().page = 3;
        m.browse.page_no = 3;
        assert!(keys(&mut m, "n").is_empty());
    }

    #[test]
    fn f_builds_a_filter_and_reloads_the_first_page() {
        let mut m = browsing(148, 42);
        m.browse.col = column_index(&m, "status");
        m.browse.page_no = 3;
        keys(&mut m, "F");
        let form = m.browse.form.clone().unwrap();
        assert_eq!((form.field, form.column), (FormField::Column, 2));
        // Column: three on, one back (total), then the operator: `>`.
        update(&mut m, press(KeyCode::Right));
        update(&mut m, press(KeyCode::Down));
        update(&mut m, press(KeyCode::Down));
        update(&mut m, press(KeyCode::Down));
        update(&mut m, press(KeyCode::Up));
        update(&mut m, press(KeyCode::Tab));
        update(&mut m, press(KeyCode::Right));
        update(&mut m, press(KeyCode::Right));
        update(&mut m, press(KeyCode::Tab));
        keys(&mut m, "3000");
        let effects = enter(&mut m);
        assert_eq!(m.browse.form, None);
        let page = pages(&effects);
        assert_eq!(page[0].page, 1);
        assert_eq!(page[0].query.filters.len(), 1);
        let f = &page[0].query.filters[0];
        assert_eq!(
            (f.column.as_str(), f.op, f.value.as_str()),
            ("total", FilterOp::Gt, "3000")
        );
        // Esc clears the server filter and reloads, before going back.
        let page = pages(&esc(&mut m)).len();
        assert_eq!(page, 1);
        assert_eq!(m.browse.filter, None);
        assert_eq!(m.focus, Panel::Main);
        // Esc in the form cancels it.
        keys(&mut m, "F");
        esc(&mut m);
        assert_eq!(m.browse.form, None);
        // IS NULL skips the value.
        keys(&mut m, "F");
        update(&mut m, press(KeyCode::Tab));
        for _ in 0..10 {
            update(&mut m, press(KeyCode::Right));
        }
        update(&mut m, press(KeyCode::Tab));
        assert_eq!(m.browse.form.as_ref().unwrap().field, FormField::Column);
        let f = pages(&enter(&mut m))[0].query.filters[0].clone();
        assert_eq!((f.op, f.value.as_str()), (FilterOp::IsNull, ""));
    }

    #[test]
    fn s_cycles_the_sort() {
        let mut m = browsing(148, 42);
        m.browse.col = column_index(&m, "issued_at");
        let sorts: Vec<_> = (0..3)
            .map(|_| pages(&keys(&mut m, "s"))[0].query.sort.clone())
            .collect();
        assert_eq!(
            sorts,
            [
                vec![Sort {
                    column: "issued_at".into(),
                    direction: SortDirection::Asc
                }],
                vec![Sort {
                    column: "issued_at".into(),
                    direction: SortDirection::Desc
                }],
                vec![],
            ]
        );
    }

    #[test]
    fn a_inserts_a_blank_row_whose_cells_edit_and_esc_drops_it_while_empty() {
        let mut m = browsing(148, 42);
        m.browse.row = 4;
        keys(&mut m, "a");
        assert_eq!((m.browse.row, m.browse.col), (0, 0));
        assert!(matches!(rows(&m)[0], GridRow::Insert(_)));
        assert_eq!(rows(&m).len(), 16);
        assert_eq!(m.staged.inserts, 1);
        esc(&mut m);
        assert_eq!(rows(&m).len(), 15, "empty: dropped");
        assert_eq!(m.focus, Panel::Main);

        keys(&mut m, "al");
        keys(&mut m, "eNew Co");
        let effects = enter(&mut m);
        let Edit::InsertRow { values, .. } = &plans(&effects)[0].request.edit else {
            panic!("an insert")
        };
        assert_eq!(
            *values,
            [("customer".to_string(), Value::Text("New Co".into()))]
        );
        esc(&mut m);
        assert_eq!(m.focus, Panel::Tables, "not empty: kept, and Esc goes back");
        assert_eq!(m.staged.inserts, 1);
    }

    #[test]
    fn capital_d_stages_set_default() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        let effects = keys(&mut m, "D");
        let Edit::SetDefault { key, column, .. } = &plans(&effects)[0].request.edit else {
            panic!("set default")
        };
        assert_eq!(
            (key[0].1.clone(), column.as_str()),
            (Value::Int(48109), "total")
        );
        assert_eq!(m.staged.updates, 1);
        let row = rows(&m)[3].clone();
        assert_eq!(cell_value(&m, &row, "total"), None, "shows DEFAULT");
    }

    // A keyless table: the edit goes to Core with no key, and Core's
    // `NOT_EDITABLE` message is what the TUI says.
    #[test]
    fn a_keyless_table_says_core_s_not_editable_message() {
        let mut m = keyless(148, 42);
        m.browse.col = 1;
        keys(&mut m, "eX");
        let effects = enter(&mut m);
        let call = plans(&effects)[0].clone();
        let Edit::UpdateCell { key, .. } = &call.request.edit else {
            panic!()
        };
        assert!(key.is_empty());
        let message = "public.invoices has no primary key, so its rows can't be edited here.";
        let effects = update(
            &mut m,
            Msg::Planned {
                id: call.request.id.clone(),
                seq: call.request.seq,
                result: Err(CallError::new("NOT_EDITABLE", message)),
            },
        );
        assert_eq!(logs(&effects), [(Some(Tag::ReadOnly), message.to_string())]);
        assert!(m.queue.is_empty());
        assert_eq!(m.staged.total(), 0);
        // `d` and `D` the same way.
        for k in ["d", "D"] {
            let effects = keys(&mut m, k);
            let call = plans(&effects)[0].clone();
            update(
                &mut m,
                Msg::Planned {
                    id: call.request.id,
                    seq: call.request.seq,
                    result: Err(CallError::new("NOT_EDITABLE", message)),
                },
            );
            assert!(m.queue.is_empty(), "{k}");
        }
    }

    #[test]
    fn a_plan_logs_core_s_sql_and_a_late_one_is_dropped() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        let first = plans(&{
            keys(&mut m, "e1");
            enter(&mut m)
        })[0]
            .request
            .clone();
        let second = plans(&{
            keys(&mut m, "e2");
            enter(&mut m)
        })[0]
            .request
            .clone();
        let plan = |sql: &str| {
            Ok(PlannedChange {
                sql: sql.into(),
                params: vec![],
                query_type: seaquel_core::sql::statements::QueryType::Update,
                dml: true,
                summary: None,
            })
        };
        let late = update(
            &mut m,
            Msg::Planned {
                id: first.id.clone(),
                seq: first.seq,
                result: plan("first"),
            },
        );
        assert!(late.is_empty(), "dropped");
        let effects = update(
            &mut m,
            Msg::Planned {
                id: second.id,
                seq: second.seq,
                result: plan("UPDATE \"public\".\"invoices\" SET \"total\" = $1 WHERE \"id\" = $2"),
            },
        );
        assert_eq!(
            logs(&effects),
            [(
                Some(Tag::Staged),
                "UPDATE \"public\".\"invoices\" SET \"total\" = $1 WHERE \"id\" = $2".to_string()
            )]
        );
        assert!(matches!(m.queue.entries()[0].plan, Plan::Planned(_)));
    }

    // Prototype `undo`.
    #[test]
    fn u_undoes_the_last_staging() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        keys(&mut m, "e1");
        enter(&mut m);
        m.browse.row = 6;
        keys(&mut m, "d");
        assert_eq!(m.staged.total(), 2);
        let effects = keys(&mut m, "u");
        assert_eq!(
            logs(&effects),
            [(Some(Tag::Undo), "delete id 48106".to_string())]
        );
        assert_eq!(m.staged.total(), 1);
        keys(&mut m, "u");
        assert_eq!(m.staged.total(), 0);
        assert!(keys(&mut m, "u").is_empty(), "nothing left");
    }

    fn set_cell(model: &mut Model, row: usize, column: &str, value: Value) {
        let i = column_index(model, column);
        model.browse.page.as_mut().unwrap().rows[row][i] = value;
        model.browse.page_gen += 1;
        refresh(model);
    }

    // I1: the text "NULL" starts as `\NULL`, and an unchanged Enter stages
    // nothing (the GUI's rule).
    #[test]
    fn an_unchanged_enter_stages_nothing_and_null_text_round_trips() {
        let mut m = browsing(148, 42);
        set_cell(&mut m, 1, "customer", Value::Text("NULL".into()));
        set_cell(&mut m, 2, "customer", Value::Text("\\NULL".into()));
        m.browse.col = 1;
        for row in [0, 1, 2, 3] {
            m.browse.row = row;
            keys(&mut m, "e");
            assert!(enter(&mut m).is_empty(), "row {row}");
            assert!(m.queue.is_empty(), "row {row}");
        }
        m.browse.row = 1;
        keys(&mut m, "e");
        assert_eq!(m.browse.editing.as_ref().unwrap().text, "\\NULL");
        esc(&mut m);
        m.browse.row = 2;
        keys(&mut m, "e");
        assert_eq!(m.browse.editing.as_ref().unwrap().text, "\\\\NULL");
        esc(&mut m);
        assert_eq!(typed_value("\\\\NULL"), Value::Text("\\NULL".into()));
    }

    // I2: the GUI's `editedCellValue`.
    #[test]
    fn hex_is_bytes_where_the_cell_held_bytes_or_the_column_is_binary_on_mysql() {
        let bytes = Value::Bytes(vec![0xde, 0xad]);
        assert_eq!(
            edited_value(Some(&bytes), "blob", "sqlite", "\\xbeef"),
            Value::Bytes(vec![0xbe, 0xef])
        );
        assert_eq!(
            edited_value(Some(&bytes), "blob", "sqlite", "\\xbee"),
            Value::Text("\\xbee".into()),
            "odd hex: text"
        );
        assert_eq!(
            edited_value(Some(&Value::Text("a".into())), "BLOB", "sqlite", "\\x00"),
            Value::Text("\\x00".into()),
            "SQLite's untyped BLOB holding text stays text"
        );
        for (ty, engine) in [
            ("varbinary(16)", "mysql"),
            ("longblob", "mariadb"),
            ("BINARY(4)", "mysql"),
        ] {
            assert_eq!(
                edited_value(None, ty, engine, "\\x0102"),
                Value::Bytes(vec![1, 2]),
                "{ty} {engine}"
            );
        }
        assert_eq!(
            edited_value(None, "bytea", "postgres", "\\x0102"),
            Value::Text("\\x0102".into())
        );
        assert_eq!(
            edited_value(Some(&bytes), "blob", "sqlite", "NULL"),
            Value::Null
        );
        assert_eq!(
            edited_value(None, "varbinary", "mysql", "\\x"),
            Value::Bytes(vec![]),
            "empty hex is empty bytes"
        );
    }

    #[test]
    fn a_bytes_cell_edits_as_hex_and_stages_bytes() {
        let mut m = browsing(148, 42);
        set_cell(&mut m, 0, "customer", Value::Bytes(vec![0xca, 0xfe]));
        m.browse.col = 1;
        keys(&mut m, "e");
        assert_eq!(m.browse.editing.as_ref().unwrap().text, "\\xcafe");
        update(&mut m, press(KeyCode::Backspace));
        keys(&mut m, "0");
        let effects = enter(&mut m);
        let Edit::UpdateCell { value, .. } = &plans(&effects)[0].request.edit else {
            panic!()
        };
        assert_eq!(*value, Value::Bytes(vec![0xca, 0xf0]));
    }

    /// Connected to another saved connection, on the same table name.
    fn switch_to_other(m: &mut Model) {
        m.conn = crate::state::app::Conn::Connected {
            id: "conn-ask".into(),
            core_id: "core-2".into(),
        };
        m.browse.opened.as_mut().unwrap().core_id = "core-2".into();
        refresh(m);
    }

    // I3: the queue's overlays show only on its own connection.
    #[test]
    fn another_connection_shows_none_of_the_queue_and_refuses_staging() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        keys(&mut m, "e9");
        enter(&mut m);
        assert_eq!(m.queue.connection(), Some("conn-saved"));
        switch_to_other(&mut m);
        let row = rows(&m)[3].clone();
        assert_eq!(
            cell_value(&m, &row, "total"),
            Some(Value::Decimal("2975.00".into())),
            "the loaded value, not the staged one"
        );
        keys(&mut m, "a");
        assert_eq!(
            m.queue.entries().len(),
            1,
            "no insert on another connection"
        );
        // Task 5's question (keep, discard, cancel); Esc keeps them.
        assert!(matches!(
            m.modal,
            Some(crate::state::app::Modal::QueueSwitch(_))
        ));
        esc(&mut m);
        let effects = keys(&mut m, "d");
        assert!(plans(&effects).is_empty());
        assert_eq!(
            logs(&effects),
            [(Some(Tag::Error), text::queue_elsewhere(1, "prod-analytics"))]
        );
        esc(&mut m);
        assert_eq!(m.queue.entries().len(), 1);
        // Back on its own connection, the overlay is back.
        m.conn = crate::state::app::Conn::Connected {
            id: "conn-saved".into(),
            core_id: "core-3".into(),
        };
        refresh(&mut m);
        assert_eq!(
            cell_value(&m, &row, "total"),
            Some(Value::Text("2975.009".into()))
        );
    }

    // I3: undo plans again only on the queue's own connection; with none
    // open the entries stay staged, unplanned.
    #[test]
    fn undo_with_no_table_open_plans_on_the_queue_s_connection_or_leaves_it_unplanned() {
        for connected in [true, false] {
            let mut m = browsing(148, 42);
            on_total(&mut m);
            keys(&mut m, "e1");
            enter(&mut m);
            keys(&mut m, "e2");
            enter(&mut m);
            close(&mut m);
            if !connected {
                m.conn = crate::state::app::Conn::None;
            }
            let effects = keys(&mut m, "u");
            let plan = plans(&effects);
            assert_eq!(m.queue.entries().len(), 1);
            if connected {
                assert_eq!(plan.len(), 1);
                assert_eq!(plan[0].core_id, "core-1");
                assert_eq!(m.queue.entries()[0].plan, Plan::Planning);
            } else {
                assert!(plan.is_empty());
                assert_eq!(
                    m.queue.entries()[0].plan,
                    Plan::Unplanned,
                    "never stuck planning"
                );
            }
        }
    }

    // I4: only a refusal of the edit takes it back; Core's message is
    // always logged.
    #[test]
    fn other_failures_keep_the_entry_unplanned_and_say_core_s_message() {
        for (code, kept) in [
            ("INVALID_ARGUMENT", false),
            ("NOT_EDITABLE", false),
            ("QUERY_ERROR", true),
        ] {
            let mut m = browsing(148, 42);
            on_total(&mut m);
            keys(&mut m, "e1");
            let call = plans(&enter(&mut m))[0].clone();
            let message = format!("{code} said this");
            let effects = update(
                &mut m,
                Msg::Planned {
                    id: call.request.id,
                    seq: call.request.seq,
                    result: Err(CallError::new(code, &message)),
                },
            );
            let logged = logs(&effects);
            assert_eq!(logged.len(), 1, "{code}");
            assert!(logged[0].1.contains(&message), "{code}: {logged:?}");
            assert_eq!(m.queue.entries().len(), usize::from(kept), "{code}");
            if kept {
                assert_eq!(m.queue.entries()[0].plan, Plan::Unplanned);
                assert_eq!(logged[0].0, Some(Tag::Error));
            } else {
                assert_eq!(logged[0].0, Some(Tag::ReadOnly));
            }
        }
    }

    // M6: a row staged for delete says why it doesn't edit; no empty
    // `unstaged` line.
    #[test]
    fn a_deleted_row_says_why_and_an_emptied_insert_logs_nothing_empty() {
        let mut m = browsing(148, 42);
        m.browse.row = 6;
        m.browse.col = 1;
        keys(&mut m, "d");
        let effects = keys(&mut m, "e");
        assert_eq!(
            logs(&effects),
            [(Some(Tag::ReadOnly), text::ROW_DELETED.to_string())]
        );
        assert_eq!(
            logs(&keys(&mut m, "D")),
            [(Some(Tag::ReadOnly), text::ROW_DELETED.to_string())]
        );

        keys(&mut m, "al");
        keys(&mut m, "ex");
        enter(&mut m);
        let effects = keys(&mut m, "D");
        assert!(
            logs(&effects).iter().all(|(_, t)| !t.is_empty()),
            "{:?}",
            logs(&effects)
        );
    }

    // M1: the `/` filter's rows and the column widths are kept with the
    // page, not worked out per frame.
    #[test]
    fn the_grid_s_cache_follows_the_page_the_filter_and_the_queue() {
        let mut m = browsing(148, 42);
        let widths = m.browse.cache.widths.clone();
        assert_eq!(widths.len(), 7);
        assert_eq!(m.browse.cache.visible.len(), 15);
        keys(&mut m, "/acme");
        assert_eq!(m.browse.cache.visible, [0]);
        esc(&mut m);
        assert_eq!(m.browse.cache.visible.len(), 15);
        // A long staged value widens its column (whole page, staged too).
        m.browse.row = 4;
        m.browse.col = 1;
        keys(&mut m, "e");
        keys(&mut m, "-a-much-longer-customer-name");
        enter(&mut m);
        assert!(m.browse.cache.widths[1] > widths[1]);
        assert_eq!(
            m.browse.cache.right,
            [true, false, false, false, false, true, false]
        );
    }

    // A new page under an edit of a page row: the row it named is gone.
    #[test]
    fn a_page_arriving_ends_an_edit_of_a_page_row() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        let op = pages(&keys(&mut m, "r"))[0].op;
        keys(&mut m, "e");
        let page = m.browse.page.clone().unwrap();
        update(
            &mut m,
            Msg::Page {
                op,
                result: Ok(page),
                stamp: Stamp::default(),
            },
        );
        assert_eq!(m.browse.editing, None);
        assert!(m.queue.is_empty());
    }

    #[test]
    fn metadata_for_another_table_or_connection_is_dropped() {
        let mut m = browsing(148, 42);
        let meta = TableMeta {
            columns: vec![],
            indexes: vec![],
            ddl: Ok(String::new()),
        };
        update(
            &mut m,
            Msg::Meta {
                core_id: "core-old".into(),
                target: invoices(),
                result: Ok(meta.clone()),
            },
        );
        assert!(matches!(&m.browse.meta, Meta::Loaded(m) if !m.columns.is_empty()));
        let effects = update(
            &mut m,
            Msg::Meta {
                core_id: "core-1".into(),
                target: invoices(),
                result: Err(CallError::new("QUERY_ERROR", "gone")),
            },
        );
        assert!(matches!(m.browse.meta, Meta::Failed(_)));
        assert_eq!(
            logs(&effects),
            [(
                Some(Tag::Error),
                "table metadata failed: QUERY_ERROR".to_string()
            )]
        );
    }

    #[test]
    fn leaving_the_grid_ends_an_edit_or_a_filter_being_typed() {
        let mut m = browsing(148, 42);
        on_total(&mut m);
        keys(&mut m, "e");
        update(&mut m, crate::testing::keys::click(1, 1));
        assert_eq!(m.browse.editing, None);
        keys(&mut m, "0/");
        update(&mut m, press(KeyCode::Tab));
        assert!(
            m.browse.finding,
            "the filter takes every key while it's typed"
        );
    }
}
