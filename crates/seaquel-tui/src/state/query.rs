//! Query (design 1b) in `update`: tabs of SQL text over the
//! main view, each with its editor, its run and its results. `Q` shows the
//! editor (opening a tab when there's none), `+` opens a new one, and a
//! saved query or history row opens in a tab from panel 3 (`o`, or Enter
//! in the main view over it).
//!
//! **Run sends only what the user typed.** Ctrl+R sends the whole text to
//! Core's `db.run` (`RunTarget::All`); Ctrl+E (or Alt+R) and Normal mode's
//! `R` send it with the UTF-16 cursor (`RunTarget::Current`), and Core picks the
//! statement with the same `seaquel_core::sql` function the TUI uses to
//! know which `{{param}}`s and destructive statements the run holds. The
//! parameters open a form first; the editor's own destructive check asks
//! next (`prod` typed on a production connection) and sends
//! `confirmed`; Core's `CONFIRM_REQUIRED` reopens the question with Core's
//! list. One run per tab: a new one cancels the old, and closing the tab
//! cancels it. Results page with `db.page` from the source the statement
//! started with; "stream all" (`:all`) keeps at most [`ROW_CAP`] rows and
//! cancels the stream there.
//!
//! **Explain** (Ctrl+X; Alt+X or `:analyze` for ANALYZE) is Core's
//! `explain` on the statement at the cursor, as typed. ANALYZE runs the
//! statement, so anything but a SELECT asks first, naming its kind.
//!
//! **History comes only from Core**: `done.history` is added to panel 3
//! (the TUI's own `StorageChanged` is skipped). **Save** (Ctrl+S, `:w`)
//! is `savedQueryUpdate` for a tab from a saved query, else a name and
//! `savedQueryCreate`; `NAME_TAKEN` names the row Core says has the name.

use std::fmt;
use std::time::Instant;

use crossterm::event::KeyCode;
use seaquel_core::domain::edits::TableTarget;
use seaquel_core::domain::run::{PageSource, RunTarget, StatementKind};
use seaquel_core::Value;
use seaquel_types::ExplainResult;

use seaquel_core::sql::offsets;
use seaquel_core::sql::params::{extract_parameters, has_parameters};
use seaquel_core::sql::scan::{split_statements, statement_at, tokens};
use seaquel_core::sql::statements::{destructive_reason, query_type, QueryType};
use seaquel_core::sql::SqlEngine;

use super::app::{Effect, Load, LogEntry, Modal, Model, Panel, SavedTab};
use super::commit::{self, Destructive, HistoryLabel, PROD_LABEL};
use super::completion::{candidates, Completion};
use super::dialogs::{CallError, Notice};
use super::editor::{display_width, Editor, Mode, Normal, HIGHLIGHT_NOW_BYTES};
use super::grid::{self, Page};
use super::log::Tag;
use super::panels::{HistoryItem, SavedItem};
use super::picker::{text_hash, RememberedTab, MAX_REMEMBERED_TEXT};
use super::text;
use crate::view::layout;

/// The most rows "stream all" keeps in the model, Core's own
/// cap for a stream (`max_query_rows`, 100,000 unless
/// `SEAQUEL_MAX_QUERY_ROWS` lowers it).
pub const ROW_CAP: usize = 100_000;

/// What the query view's keys go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    #[default]
    Editor,
    Results,
}

/// The results box's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResultTab {
    #[default]
    Results,
    Explain,
    Messages,
}

impl ResultTab {
    pub const ALL: [ResultTab; 3] = [ResultTab::Results, ResultTab::Explain, ResultTab::Messages];

    pub fn index(self) -> usize {
        ResultTab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// The query tabs.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub tabs: Vec<QueryTab>,
    pub active: usize,
    /// The main view shows the query tabs (not the list panel's item).
    pub shown: bool,
    pub pane: Pane,
    next_tab: u64,
    next_op: u64,
    untitled: u32,
    /// What the state file last saw of the tabs.
    remembered_sig: Option<u64>,
}

impl Query {
    pub fn active(&self) -> Option<&QueryTab> {
        self.tabs.get(self.active)
    }

    pub fn active_mut(&mut self) -> Option<&mut QueryTab> {
        self.tabs.get_mut(self.active)
    }

    pub fn tab(&self, id: u64) -> Option<&QueryTab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: u64) -> Option<&mut QueryTab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    /// Whether any tab's run, page or explain is in flight.
    pub fn running(&self) -> bool {
        self.tabs.iter().any(|t| t.op.is_some() || t.explaining())
    }
}

/// The saved query a tab came from (or was saved as).
#[derive(Clone, PartialEq, Eq)]
pub struct SavedRef {
    pub id: String,
    pub name: String,
    pub shared: bool,
}

impl fmt::Debug for SavedRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SavedRef({})", self.id)
    }
}

/// A run or a page in flight on a tab.
#[derive(Clone, PartialEq)]
pub struct Op {
    pub op: u64,
    pub stream_id: String,
    pub kind: OpKind,
    /// Rows per page of the run (0 streams).
    pub page_size: u32,
    /// What started it, to send again once confirmed.
    pub pending: Option<PendingRun>,
    /// The saved connection its history goes under.
    pub connection_id: Option<String>,
    /// Core's connection it runs on.
    pub core_id: String,
}

impl fmt::Debug for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Op")
            .field("op", &self.op)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Run,
    /// `db.page` for one statement of the last run.
    Page {
        statement: usize,
    },
}

/// How a statement ended (or not yet).
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Running,
    Done {
        elapsed_ms: f64,
        rows_affected: Option<u64>,
    },
    Failed(CallError),
    Cancelled,
}

/// One statement of a run: what Core started, its rows and how it ended.
#[derive(Clone, PartialEq)]
pub struct StatementResult {
    pub index: u32,
    /// As typed.
    pub sql: String,
    /// What `db.page` takes back.
    pub source: Option<PageSource>,
    pub kind: Option<StatementKind>,
    /// Its rows, once it returned columns.
    pub page: Option<Page>,
    /// The page number and size its `statementStart` named.
    pub page_no: u32,
    pub page_size: u32,
    /// Each column's width over the rows held (kept as batches arrive).
    pub widths: Vec<usize>,
    pub status: Status,
    /// "Stream all" stopped at [`ROW_CAP`].
    pub capped: bool,
    /// The tick its `statementStart` arrived in: a capped stream's time.
    pub started: Option<Instant>,
}

impl fmt::Debug for StatementResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StatementResult")
            .field("index", &self.index)
            .field("kind", &self.kind)
            .field("page", &self.page)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// The Explain tab of a tab.
#[derive(Debug, Clone, PartialEq)]
pub enum ExplainView {
    Loading { analyze: bool },
    Loaded(Box<ExplainResult>),
    Failed(CallError),
}

/// One query tab.
#[derive(Clone)]
pub struct QueryTab {
    pub id: u64,
    /// The saved query's name, or `untitled-N`.
    pub title: String,
    pub saved: Option<SavedRef>,
    /// The text as stored (or as opened): `modified` compares with it.
    pub stored: String,
    pub modified: bool,
    checked_gen: Option<u64>,
    pub editor: Editor,
    pub op: Option<Op>,
    pub statements: Vec<StatementResult>,
    /// A run-level failure (`CONNECTION_NOT_FOUND`, …).
    pub run_error: Option<CallError>,
    /// The saved connection the results came from.
    pub results_connection: Option<String>,
    /// The run's `done`: statements planned, and whether they all ran.
    pub finished: Option<(u32, bool)>,
    /// Which statement the Results tab shows.
    pub shown: Option<usize>,
    pub result_tab: ResultTab,
    /// The results grid's cursor.
    pub row: usize,
    pub col: usize,
    pub explain: Option<ExplainView>,
    pub explain_op: Option<u64>,
    /// The parameter values last typed, by name.
    pub params: Vec<(String, String)>,
    pub started: Option<Instant>,
    /// Restored from the state file and waiting for panel 3's list: take
    /// the saved query's text (an unchanged tab) or only its stored text.
    pub awaiting_library: Option<Awaiting>,
    /// The stored text's hash the state file had (kept while waiting).
    restored_hash: Option<String>,
    /// Why the tab's text is empty (too long to keep).
    pub notice: Option<String>,
}

/// What a restored saved tab still needs from the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Awaiting {
    /// The text and the stored text (an unchanged tab).
    Text,
    /// The stored text only (its own text was kept).
    Stored,
}

impl fmt::Debug for QueryTab {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("QueryTab")
            .field("id", &self.id)
            .field("saved", &self.saved)
            .field("modified", &self.modified)
            .field("editor", &self.editor)
            .field("op", &self.op)
            .field("statements", &self.statements.len())
            .finish_non_exhaustive()
    }
}

/// A run waiting for its parameters or a confirmation. Holds the text as it
/// was when Run was pressed, so what's confirmed is what runs.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingRun {
    pub tab: u64,
    pub text: String,
    pub target: RunTarget,
    /// `:all`: page size 0, at most [`ROW_CAP`] rows kept.
    pub stream_all: bool,
    /// The form's values as typed (`NULL` is NULL).
    pub params: Option<Vec<(String, String)>>,
}

impl fmt::Debug for PendingRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingRun")
            .field("tab", &self.tab)
            .field("target", &self.target)
            .field("stream_all", &self.stream_all)
            .field("params", &self.params.as_ref().map(Vec::len))
            .finish_non_exhaustive()
    }
}

/// The `{{param}}` form.
#[derive(Clone, PartialEq, Eq)]
pub struct ParamsForm {
    pub names: Vec<String>,
    pub values: Vec<String>,
    pub field: usize,
    pub pending: PendingRun,
}

impl fmt::Debug for ParamsForm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParamsForm")
            .field("names", &self.names.len())
            .field("field", &self.field)
            .finish_non_exhaustive()
    }
}

/// What a run confirmation asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmKind {
    /// Destructive statements: the editor's check, or Core's list (and its
    /// total) after `CONFIRM_REQUIRED`.
    Destructive {
        list: Vec<Destructive>,
        total: u32,
        from_core: bool,
    },
    /// EXPLAIN ANALYZE of a statement that isn't a SELECT: its kind.
    Analyze { verb: String },
}

/// What runs once confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirmed {
    Run(PendingRun),
    Analyze(ExplainCall),
}

/// "Run it?" (and on a `prod` connection, `prod` typed).
#[derive(Clone, PartialEq, Eq)]
pub struct RunConfirm {
    pub kind: ConfirmKind,
    pub typed: String,
    pub then: Confirmed,
}

impl fmt::Debug for RunConfirm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunConfirm")
            .field("kind", &self.kind)
            .field("typed_len", &self.typed.len())
            .finish_non_exhaustive()
    }
}

/// Ctrl+S on a tab that isn't saved yet: its name.
#[derive(Clone, PartialEq, Eq)]
pub struct SaveAs {
    pub tab: u64,
    pub name: String,
    /// Why the last try was refused (`NAME_TAKEN`, worded).
    pub error: Option<String>,
    /// Ask AI's SQL to save instead of the tab's text.
    pub text: Option<SqlText>,
    /// The Ask AI popup to go back to.
    pub back: Option<Box<super::ask::Ask>>,
}

impl fmt::Debug for SaveAs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveAs")
            .field("tab", &self.tab)
            .field("error", &self.error.is_some())
            .finish_non_exhaustive()
    }
}

/// A cell opened full size (Enter in the results).
#[derive(Clone, PartialEq, Eq)]
pub struct CellView {
    pub column: String,
    /// Wrapped by the view; JSON pretty-printed.
    pub text: String,
    pub scroll: usize,
}

impl fmt::Debug for CellView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CellView")
            .field("bytes", &self.text.len())
            .finish_non_exhaustive()
    }
}

/// A run for the runtime: `db.run` with the TUI's origin.
#[derive(Clone, PartialEq)]
pub struct RunCall {
    pub tab: u64,
    pub op: u64,
    pub stream_id: String,
    pub core_id: String,
    pub text: String,
    pub target: RunTarget,
    pub params: Option<Vec<(String, Value)>>,
    pub page_size: u32,
    pub confirmed: bool,
    /// Record the run under this saved connection.
    pub history: Option<HistoryCall>,
}

impl fmt::Debug for RunCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RunCall")
            .field("tab", &self.tab)
            .field("op", &self.op)
            .field("target", &self.target)
            .field("text_len", &self.text.len())
            .field("params", &self.params.as_ref().map(Vec::len))
            .field("page_size", &self.page_size)
            .field("confirmed", &self.confirmed)
            .finish_non_exhaustive()
    }
}

/// `db.run`'s history context, as the model knows the saved connection.
#[derive(Clone, PartialEq, Eq)]
pub struct HistoryCall {
    pub connection_id: String,
    pub connection_name: String,
    pub labels: Vec<HistoryLabel>,
}

impl fmt::Debug for HistoryCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "HistoryCall({})", self.connection_id)
    }
}

/// `db.page` for one statement of a tab's last run.
#[derive(Clone, PartialEq)]
pub struct PageRunCall {
    pub tab: u64,
    pub op: u64,
    pub stream_id: String,
    pub core_id: String,
    pub source: PageSource,
    pub page: u32,
    pub page_size: u32,
}

impl fmt::Debug for PageRunCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PageRunCall")
            .field("tab", &self.tab)
            .field("op", &self.op)
            .field("page", &self.page)
            .finish_non_exhaustive()
    }
}

/// Core's `explain` of the statement at the cursor, as typed.
#[derive(Clone, PartialEq, Eq)]
pub struct ExplainCall {
    pub tab: u64,
    pub op: u64,
    pub core_id: String,
    pub sql: String,
    pub analyze: bool,
}

impl fmt::Debug for ExplainCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExplainCall")
            .field("tab", &self.tab)
            .field("op", &self.op)
            .field("analyze", &self.analyze)
            .finish_non_exhaustive()
    }
}

/// `savedQueryCreate` or `savedQueryUpdate` of a tab's text.
#[derive(Clone, PartialEq, Eq)]
pub struct SaveQueryCall {
    pub tab: u64,
    pub save: SaveKind,
    pub text: String,
    /// Ask AI's SQL, not the tab's text: the tab stays as it is.
    pub detached: bool,
}

impl fmt::Debug for SaveQueryCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SaveQueryCall")
            .field("tab", &self.tab)
            .field("save", &self.save)
            .field("detached", &self.detached)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub enum SaveKind {
    Create { project_id: String, name: String },
    Update { id: String },
}

impl fmt::Debug for SaveKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveKind::Create { project_id, .. } => write!(f, "Create({project_id})"),
            SaveKind::Update { id } => write!(f, "Update({id})"),
        }
    }
}

/// A run's events as `update` takes them (`RunEvent`, with the history
/// row in the model's type).
#[derive(Clone)]
pub enum RunMsg {
    Start {
        index: u32,
        sql: String,
        source: PageSource,
        kind: StatementKind,
        page: u32,
        page_size: u32,
    },
    Batch {
        columns: Option<Vec<String>>,
        rows: Vec<Vec<Value>>,
    },
    Done {
        index: u32,
        elapsed_ms: f64,
        total_rows: u64,
        total_pages: u32,
        count_estimated: bool,
        rows_affected: Option<u64>,
    },
    Failed {
        index: u32,
        error: CallError,
        elapsed_ms: f64,
        /// A planned failure has no `Start`: its SQL comes here.
        sql: Option<String>,
    },
    Finished {
        statements: u32,
        succeeded: bool,
        history: Option<HistoryItem>,
    },
    Refused {
        error: CallError,
        /// `CONFIRM_REQUIRED`: Core's list and its total.
        destructive: Option<(Vec<Destructive>, u32)>,
    },
    /// The stream ended with neither `done` nor `error`: cancelled.
    Ended,
}

impl fmt::Debug for RunMsg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunMsg::Start { index, kind, .. } => write!(f, "Start({index}, {kind:?})"),
            RunMsg::Batch { rows, .. } => write!(f, "Batch({} rows)", rows.len()),
            RunMsg::Done { index, .. } => write!(f, "Done({index})"),
            RunMsg::Failed { index, error, .. } => write!(f, "Failed({index}, {error:?})"),
            RunMsg::Finished { statements, .. } => write!(f, "Finished({statements})"),
            RunMsg::Refused { error, .. } => write!(f, "Refused({error:?})"),
            RunMsg::Ended => f.write_str("Ended"),
        }
    }
}

/// SQL text in a message or effect: `Debug` shows its size only (`Msg` and
/// `Effect` derive `Debug`).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct SqlText(pub String);

impl fmt::Debug for SqlText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SqlText({} bytes)", self.0.len())
    }
}

impl From<String> for SqlText {
    fn from(s: String) -> SqlText {
        SqlText(s)
    }
}

impl From<&str> for SqlText {
    fn from(s: &str) -> SqlText {
        SqlText(s.to_string())
    }
}

/// Which statements Run means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    All,
    Current,
}

// ── Tabs ──

impl QueryTab {
    fn new(id: u64, title: String, text: &str, saved: Option<SavedRef>) -> QueryTab {
        let mut editor = Editor::new(text);
        editor.scroll_into_view(1, 1);
        QueryTab {
            id,
            title,
            saved,
            stored: if text.is_empty() {
                String::new()
            } else {
                text.to_string()
            },
            modified: false,
            checked_gen: None,
            editor,
            op: None,
            statements: Vec::new(),
            run_error: None,
            results_connection: None,
            finished: None,
            shown: None,
            result_tab: ResultTab::Results,
            row: 0,
            col: 0,
            explain: None,
            explain_op: None,
            params: Vec::new(),
            started: None,
            awaiting_library: None,
            restored_hash: None,
            notice: None,
        }
    }

    /// The statement the Results tab shows.
    pub fn shown_statement(&self) -> Option<&StatementResult> {
        self.statements.get(self.shown?)
    }

    /// Whether an explain is in flight.
    pub fn explaining(&self) -> bool {
        matches!(self.explain, Some(ExplainView::Loading { .. }))
    }
}

/// A tab with `text`, made active, the query view shown and the editor
/// focused.
fn add_tab(model: &mut Model, title: Option<String>, text: &str, saved: Option<SavedRef>) {
    let q = &mut model.query;
    q.next_tab += 1;
    let title = title.unwrap_or_else(|| {
        q.untitled += 1;
        text::untitled(q.untitled)
    });
    let mut tab = QueryTab::new(q.next_tab, title, text, saved);
    if tab.saved.is_none() {
        // A history row (or a new tab) isn't stored anywhere yet.
        tab.stored = String::new();
    }
    q.tabs.push(tab);
    q.active = q.tabs.len() - 1;
    focus_editor(model);
}

pub(crate) fn focus_editor(model: &mut Model) {
    model.focus_panel(Panel::Main);
    model.query.shown = true;
    model.query.pane = Pane::Editor;
}

/// `Q`: the query view, with a new tab when there's none.
pub fn show(model: &mut Model) -> Vec<Effect> {
    if model.query.tabs.is_empty() {
        add_tab(model, None, "", None);
    } else {
        focus_editor(model);
    }
    Vec::new()
}

/// `+`: a new, empty tab.
pub fn new_tab(model: &mut Model) -> Vec<Effect> {
    add_tab(model, None, "", None);
    Vec::new()
}

/// `o` in panel 3, or Enter in the main view over it: the selected saved
/// query (its own tab, reused when open) or history row (a new tab).
pub fn open_selected(model: &mut Model) -> Vec<Effect> {
    if model.ctx != Panel::Saved || (model.focus == Panel::Main && model.query.shown) {
        return Vec::new();
    }
    match model.saved_tab {
        SavedTab::Saved => {
            let Some(super::panels::Row::Item(i)) = model.selected_saved_row() else {
                return Vec::new();
            };
            let Some(item) = model.saved_items.get(i).cloned() else {
                return Vec::new();
            };
            if let Some(i) = model
                .query
                .tabs
                .iter()
                .position(|t| t.saved.as_ref().is_some_and(|s| s.id == item.id))
            {
                model.query.active = i;
                focus_editor(model);
                return Vec::new();
            }
            let saved = SavedRef {
                id: item.id.clone(),
                name: item.name.clone(),
                shared: item.shared,
            };
            add_tab(model, Some(item.name.clone()), &item.sql, Some(saved));
        }
        SavedTab::History => {
            let Some(item) = model.history_items.get(model.history.selected).cloned() else {
                return Vec::new();
            };
            add_tab(model, None, &item.sql, None);
        }
    }
    Vec::new()
}

/// `[`/`]` over the query view: the next tab (editor) or result tab.
pub fn cycle(model: &mut Model, forward: bool) {
    let q = &mut model.query;
    match q.pane {
        Pane::Editor => {
            let n = q.tabs.len();
            if n > 0 {
                q.active = (q.active + if forward { 1 } else { n - 1 }) % n;
            }
        }
        Pane::Results => {
            if let Some(tab) = q.active_mut() {
                let i = tab.result_tab.index();
                let n = ResultTab::ALL.len();
                tab.result_tab = ResultTab::ALL[(i + if forward { 1 } else { n - 1 }) % n];
            }
        }
    }
}

/// Ctrl+W: the editor or the results.
pub fn toggle_pane(model: &mut Model) {
    let q = &mut model.query;
    q.pane = match q.pane {
        Pane::Editor => Pane::Results,
        Pane::Results => Pane::Editor,
    };
    if let Some(tab) = q.active_mut() {
        tab.editor.completion = None;
    }
}

/// Closes the active tab (`:q`, `:q!`), cancelling what it runs.
fn close_tab(model: &mut Model, force: bool) -> Vec<Effect> {
    let Some(tab) = model.query.active() else {
        return Vec::new();
    };
    if tab.modified && !force {
        return vec![Model::log_effect(Some(Tag::Error), text::TAB_MODIFIED)];
    }
    let mut tab = model.query.tabs.remove(model.query.active);
    let effects = cancel_tab(&mut tab);
    let q = &mut model.query;
    q.active = q.active.min(q.tabs.len().saturating_sub(1));
    if q.tabs.is_empty() {
        q.shown = false;
        model.focus_panel(model.ctx);
    }
    effects
}

// ── The editor ──

/// The active tab's editor, when the editor has the keys.
fn editor_mut(model: &mut Model) -> Option<&mut Editor> {
    if model.focus != Panel::Main || !model.query.shown || model.query.pane != Pane::Editor {
        return None;
    }
    model.query.active_mut().map(|t| &mut t.editor)
}

/// Whether the editor takes printable keys as text now.
pub fn typing(model: &Model) -> bool {
    model.modal.is_none()
        && model.focus == Panel::Main
        && model.query.shown
        && model.query.pane == Pane::Editor
        && model
            .query
            .active()
            .is_some_and(|t| t.editor.mode == Mode::Insert || t.editor.command.is_some())
}

/// How long the `:` line may grow.
const COMMAND_MAX: usize = 64;

/// A printable key in Insert mode (or the `:` line).
pub fn type_char(model: &mut Model, c: char) -> Vec<Effect> {
    let Some(editor) = editor_mut(model) else {
        return Vec::new();
    };
    if let Some(command) = &mut editor.command {
        if command.chars().count() < COMMAND_MAX {
            command.push(c);
        }
        return Vec::new();
    }
    editor.type_char(c);
    let open = editor.completion.is_some();
    if open {
        refresh_completion(model, true).1
    } else if c == '.' {
        refresh_completion(model, false).1
    } else {
        Vec::new()
    }
}

/// A bracketed paste: in Insert mode its text goes in exactly as received
/// (one undo step), and any popup closes.
pub fn paste(model: &mut Model, text: &str) -> Vec<Effect> {
    if !typing(model) {
        return Vec::new();
    }
    let Some(editor) = editor_mut(model) else {
        return Vec::new();
    };
    if let Some(command) = &mut editor.command {
        // The `:` line takes the first line of it.
        command.push_str(text.lines().next().unwrap_or(""));
        command.truncate(command.chars().take(COMMAND_MAX).map(char::len_utf8).sum());
        return Vec::new();
    }
    editor.completion = None;
    editor.insert_str(text);
    Vec::new()
}

/// An editing key in Insert mode, or Backspace on the `:` line; `false`
/// when it isn't one.
pub fn edit_key(model: &mut Model, code: KeyCode) -> bool {
    let Some(editor) = editor_mut(model) else {
        return false;
    };
    if let Some(command) = &mut editor.command {
        if code == KeyCode::Backspace {
            command.pop();
            return true;
        }
        return false;
    }
    let handled = editor.insert_key(code);
    if handled && editor.completion.is_some() {
        refresh_completion(model, true);
    }
    handled
}

/// Esc in Insert mode.
pub fn normal_mode(model: &mut Model) {
    if let Some(editor) = editor_mut(model) {
        editor.mode = Mode::Normal;
        editor.completion = None;
        editor.pending = None;
    }
}

/// A Normal-mode command.
pub fn normal(model: &mut Model, command: Normal) {
    if let Some(editor) = editor_mut(model) {
        editor.normal(command);
        editor.completion = None;
    }
}

/// `:`: the command line.
pub fn start_command(model: &mut Model) {
    if let Some(editor) = editor_mut(model) {
        editor.pending = None;
        editor.command = Some(String::new());
    }
}

/// Enter on the command line: `w`, `explain`, `analyze`, `all`, `q`, `q!`.
pub fn run_command(model: &mut Model) -> Vec<Effect> {
    let Some(command) = editor_mut(model).and_then(|e| e.command.take()) else {
        return Vec::new();
    };
    match command.trim() {
        "" => Vec::new(),
        "w" => save(model),
        "explain" => explain(model, false),
        "analyze" => explain(model, true),
        "all" => run(model, RunKind::All, true),
        "q" => close_tab(model, false),
        "q!" => close_tab(model, true),
        "ask" => super::ask::open(model),
        other => vec![Model::log_effect(
            Some(Tag::Error),
            text::unknown_command(other),
        )],
    }
}

pub fn cancel_command(model: &mut Model) {
    if let Some(editor) = editor_mut(model) {
        editor.command = None;
    }
}

// ── Completion ──

/// The engine panel 1's connection reads SQL with, if it's a known one.
pub(crate) fn engine(model: &Model) -> Option<SqlEngine> {
    let id = model.conn.id()?;
    model.library.connection(id)?.engine.parse().ok()
}

/// The engine highlighting and completion read with: panel 1's, else
/// Postgres's quoting.
pub fn editor_engine(model: &Model) -> SqlEngine {
    engine(model).unwrap_or(SqlEngine::Postgres)
}

/// Opens (or refreshes, or closes) the popup for the cursor; whether it's
/// open, and a read of an `alias.`'s columns when they aren't known yet
/// (`schema_tables` lists none), which reopens it on arrival.
fn refresh_completion(model: &mut Model, explicit: bool) -> (bool, Vec<Effect>) {
    let engine = editor_engine(model);
    let schema = (model.schema_load == Load::Loaded).then_some(model.schema.as_slice());
    let Some(tab) = model.query.active() else {
        return (false, Vec::new());
    };
    let text = tab.editor.text();
    let cursor = tab.editor.cursor_byte();
    let mut effects = Vec::new();
    if let (Some(schema), Some(core_id)) = (schema, model.conn.core_id()) {
        if let Some(i) = super::completion::alias_target(&text, cursor, engine, schema) {
            let table = &schema[i];
            let key = (table.schema.clone(), table.name.clone());
            if table.columns.is_empty() && !model.column_loads.contains_key(&key) {
                effects.push(Effect::LoadColumns {
                    core_id: core_id.to_string(),
                    target: TableTarget {
                        schema: key.0.clone(),
                        table: key.1.clone(),
                    },
                });
                model.column_loads.insert(key, Load::Loading);
            }
        }
    }
    let schema = (model.schema_load == Load::Loaded).then_some(model.schema.as_slice());
    let found = candidates(&text, cursor, engine, schema, explicit);
    let popup = found.map(|(start, items)| {
        let row = text[..start].matches('\n').count();
        let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
        Completion {
            items,
            selected: 0,
            row,
            col: text[line_start..start].chars().count(),
            prefix_chars: text[start..cursor].chars().count(),
        }
    });
    let opened = popup.is_some();
    if let Some(tab) = model.query.active_mut() {
        tab.editor.completion = popup;
    }
    (opened, effects)
}

/// Tab or Ctrl+Space: the popup (or, with nothing to offer, Tab indents;
/// not while an `alias.`'s columns are being read).
pub fn complete(model: &mut Model, indent: bool) -> Vec<Effect> {
    if editor_mut(model).is_none() {
        return Vec::new();
    }
    let (opened, effects) = refresh_completion(model, true);
    let reading = alias_reading(model);
    if !opened && indent && !reading {
        if let Some(editor) = editor_mut(model) {
            editor.indent();
        }
    }
    effects
}

/// Whether the cursor is after an `alias.` whose columns are being read.
fn alias_reading(model: &Model) -> bool {
    let Some(tab) = model.query.active() else {
        return false;
    };
    let text = tab.editor.text();
    super::completion::alias_target(
        &text,
        tab.editor.cursor_byte(),
        editor_engine(model),
        &model.schema,
    )
    .and_then(|i| {
        let t = &model.schema[i];
        model.column_loads.get(&(t.schema.clone(), t.name.clone()))
    })
    .is_some_and(|load| *load == Load::Loading)
}

/// Keeps a table's columns on its panel 2 entry (for completion and the
/// preview), wherever they were read.
pub fn remember_columns(model: &mut Model, target: &TableTarget, columns: Vec<(String, String)>) {
    if let Some(t) = model
        .schema
        .iter_mut()
        .find(|t| t.schema == target.schema && t.name == target.table)
    {
        t.columns = columns;
    }
    model
        .column_loads
        .insert((target.schema.clone(), target.table.clone()), Load::Loaded);
}

/// An `alias.`'s columns arrived: kept, and the popup opens if
/// the cursor is still after that alias. Another connection's, a late
/// answer after a reconnect, is dropped; a failure isn't asked again for
/// this connection (`r` in panel 2 reads the list, not the columns).
pub fn on_columns(
    model: &mut Model,
    core_id: &str,
    target: &TableTarget,
    result: Result<Vec<(String, String)>, CallError>,
) -> Vec<Effect> {
    if model.conn.core_id() != Some(core_id) {
        return Vec::new();
    }
    match result {
        Ok(columns) => remember_columns(model, target, columns),
        Err(e) => {
            model
                .column_loads
                .insert((target.schema.clone(), target.table.clone()), Load::Failed);
            return vec![Model::log_effect(
                Some(Tag::Error),
                text::failed_line("table columns", &e.code),
            )];
        }
    }
    let after_it = model.query.active().is_some_and(|tab| {
        tab.editor.mode == super::editor::Mode::Insert
            && tab.editor.command.is_none()
            && super::completion::alias_target(
                &tab.editor.text(),
                tab.editor.cursor_byte(),
                editor_engine(model),
                &model.schema,
            )
            .is_some_and(|i| {
                model.schema[i].schema == target.schema && model.schema[i].name == target.table
            })
    });
    if after_it && model.focus == Panel::Main && model.query.shown {
        return refresh_completion(model, true).1;
    }
    Vec::new()
}

pub fn completion_step(model: &mut Model, forward: bool) {
    if let Some(popup) = editor_mut(model).and_then(|e| e.completion.as_mut()) {
        let n = popup.items.len().max(1);
        popup.selected = (popup.selected + if forward { 1 } else { n - 1 }) % n;
    }
}

pub fn accept_completion(model: &mut Model) {
    let Some(editor) = editor_mut(model) else {
        return;
    };
    let Some(popup) = editor.completion.take() else {
        return;
    };
    let Some(item) = popup.items.get(popup.selected) else {
        return;
    };
    editor.delete_before(popup.prefix_chars);
    editor.insert_str(&item.label);
}

pub fn close_completion(model: &mut Model) {
    if let Some(editor) = editor_mut(model) {
        editor.completion = None;
    }
}

// ── Running ──

/// A typed parameter value: `NULL` is NULL, `\NULL` the text, anything else
/// text (Core casts it, as the GUI's dialog sends text).
fn param_value(text: &str) -> Value {
    match text {
        "NULL" => Value::Null,
        "\\NULL" => Value::Text("NULL".into()),
        other => Value::Text(other.to_string()),
    }
}

/// The statements a pending run holds, as typed: every statement, or the
/// one at the cursor (`seaquel_core::sql`'s choice, as Core's).
fn statements(pending: &PendingRun, engine: SqlEngine) -> Vec<String> {
    let text = &pending.text;
    match pending.target {
        RunTarget::All => split_statements(text, engine)
            .into_iter()
            .map(|s| text[s.text].to_string())
            .collect(),
        RunTarget::Current { cursor } => {
            let byte = offsets::utf16_to_byte(text, cursor as usize);
            statement_at(text, byte, engine)
                .map(|s| vec![text[s.text].to_string()])
                .unwrap_or_default()
        }
    }
}

/// Ctrl+R (all), Ctrl+E, Alt+R and `R` (the statement at the cursor), `:all`.
pub fn run(model: &mut Model, kind: RunKind, stream_all: bool) -> Vec<Effect> {
    if model.conn.core_id().is_none() {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_RUN)];
    }
    let Some(engine) = engine(model) else {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_RUN)];
    };
    let Some(tab) = model.query.active() else {
        return Vec::new();
    };
    let text = tab.editor.text();
    if text.trim().is_empty() {
        return vec![Model::log_effect(Some(Tag::Error), text::NOTHING_TO_RUN)];
    }
    let target = match kind {
        RunKind::All => RunTarget::All,
        RunKind::Current => RunTarget::Current {
            cursor: tab.editor.cursor_utf16() as u64,
        },
    };
    let pending = PendingRun {
        tab: tab.id,
        text,
        target,
        stream_all,
        params: None,
    };
    let mut names: Vec<String> = Vec::new();
    for statement in statements(&pending, engine) {
        for name in extract_parameters(&statement) {
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    if !names.is_empty() {
        let values = names
            .iter()
            .map(|n| {
                tab.params
                    .iter()
                    .find(|(k, _)| k == n)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            })
            .collect();
        model.modal = Some(Modal::Params(ParamsForm {
            names,
            values,
            field: 0,
            pending,
        }));
        return Vec::new();
    }
    check_and_go(model, pending, engine)
}

/// The editor's own destructive check (on the text before substitution, as
/// Core's): it asks first, then the run goes with `confirmed`.
fn check_and_go(model: &mut Model, pending: PendingRun, engine: SqlEngine) -> Vec<Effect> {
    let list: Vec<Destructive> = statements(&pending, engine)
        .into_iter()
        .filter_map(|sql| {
            destructive_reason(&sql, engine).map(|r| Destructive {
                sql,
                reason: text::destructive_reason(r).to_string(),
            })
        })
        .collect();
    if list.is_empty() {
        return go(model, pending, false);
    }
    let total = list.len() as u32;
    model.modal = Some(Modal::RunConfirm(RunConfirm {
        kind: ConfirmKind::Destructive {
            list,
            total,
            from_core: false,
        },
        typed: String::new(),
        then: Confirmed::Run(pending),
    }));
    Vec::new()
}

pub(crate) fn next_op(model: &mut Model) -> u64 {
    model.query.next_op += 1;
    model.query.next_op
}

/// Sends the run: one per tab, so the tab's last is cancelled first.
fn go(model: &mut Model, pending: PendingRun, confirmed: bool) -> Vec<Effect> {
    let Some(core_id) = model.conn.core_id().map(str::to_string) else {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_RUN)];
    };
    let history = model
        .conn
        .id()
        .and_then(|id| model.library.connection(id))
        .map(|c| HistoryCall {
            connection_id: c.id.clone(),
            connection_name: c.name.clone(),
            labels: commit::history_labels(model, c),
        });
    let op = next_op(model);
    let page_size = if pending.stream_all {
        0
    } else {
        model.page_size
    };
    let now = model.now;
    let Some(tab) = model.query.tab_mut(pending.tab) else {
        return Vec::new();
    };
    let mut effects = Vec::new();
    if let Some(old) = tab.op.take() {
        effects.push(Effect::CancelRun {
            tab: tab.id,
            stream_id: Some(old.stream_id),
        });
    }
    let stream_id = format!("tui-run-{}-{op}", tab.id);
    let params = pending.params.as_ref().map(|values| {
        values
            .iter()
            .map(|(name, value)| (name.clone(), param_value(value)))
            .collect()
    });
    let call = RunCall {
        tab: tab.id,
        op,
        stream_id: stream_id.clone(),
        core_id,
        text: pending.text.clone(),
        target: pending.target,
        params,
        page_size,
        confirmed,
        history: history.clone(),
    };
    tab.op = Some(Op {
        op,
        stream_id,
        kind: OpKind::Run,
        page_size,
        pending: Some(pending),
        connection_id: history.as_ref().map(|h| h.connection_id.clone()),
        core_id: call.core_id.clone(),
    });
    tab.statements.clear();
    tab.run_error = None;
    tab.results_connection = history.as_ref().map(|h| h.connection_id.clone());
    tab.finished = None;
    tab.shown = None;
    tab.row = 0;
    tab.col = 0;
    tab.started = now;
    if tab.result_tab == ResultTab::Explain {
        tab.result_tab = ResultTab::Results;
    }
    effects.push(Effect::Run(call));
    effects
}

/// Cancels a tab's run (and explain): its task is dropped and its stream
/// cancelled in Core; what was running is marked cancelled.
fn cancel_tab(tab: &mut QueryTab) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(op) = tab.op.take() {
        effects.push(Effect::CancelRun {
            tab: tab.id,
            stream_id: Some(op.stream_id),
        });
        for s in &mut tab.statements {
            if s.status == Status::Running {
                s.status = Status::Cancelled;
            }
        }
        tab.started = None;
    }
    if tab.explaining() {
        effects.push(Effect::CancelExplain { tab: tab.id });
        tab.explain = Some(ExplainView::Failed(CallError::new(
            "CANCELLED",
            text::EXPLAIN_STOPPED,
        )));
        tab.explain_op = None;
    }
    effects
}

/// Ctrl+C while something runs: the active tab's run, else every tab's.
pub fn cancel(model: &mut Model) -> Vec<Effect> {
    let active_runs = model
        .query
        .active()
        .is_some_and(|t| t.op.is_some() || t.explaining());
    let mut effects = Vec::new();
    for (i, tab) in model.query.tabs.iter_mut().enumerate() {
        if !active_runs || i == model.query.active {
            effects.extend(cancel_tab(tab));
        }
    }
    if !effects.is_empty() {
        effects.push(Model::log_effect(None, text::CANCELLED));
    }
    effects
}

/// The statement a run's event is about.
fn target(tab: &mut QueryTab, kind: OpKind, index: Option<u32>) -> Option<&mut StatementResult> {
    match kind {
        OpKind::Page { statement } => tab.statements.get_mut(statement),
        OpKind::Run => match index {
            Some(index) => tab.statements.iter_mut().rev().find(|s| s.index == index),
            None => tab.statements.last_mut(),
        },
    }
}

/// A log line for a statement: its SQL on one line, and its time.
fn statement_line(sql: &str, elapsed_ms: f64) -> Effect {
    Effect::Log(LogEntry {
        tag: None,
        text: grid::clean(&sql.split_whitespace().collect::<Vec<_>>().join(" ")),
        elapsed: Some(grid::elapsed_text(elapsed_ms)),
    })
}

/// A run's event.
pub fn on_run(model: &mut Model, tab: u64, op: u64, event: RunMsg) -> Vec<Effect> {
    let model_now = model.now;
    let Some(t) = model.query.tab_mut(tab) else {
        return Vec::new();
    };
    let Some(current) = t.op.as_ref().filter(|o| o.op == op) else {
        return Vec::new();
    };
    let (kind, page_size) = (current.kind, current.page_size);
    let mut effects = Vec::new();
    match event {
        RunMsg::Start {
            index,
            sql,
            source,
            kind: statement_kind,
            page,
            page_size: size,
        } => match kind {
            OpKind::Run => t.statements.push(StatementResult {
                index,
                sql,
                source: Some(source),
                kind: Some(statement_kind),
                page: None,
                page_no: page,
                page_size: size,
                widths: Vec::new(),
                status: Status::Running,
                capped: false,
                started: model_now,
            }),
            OpKind::Page { statement } => {
                if let Some(s) = t.statements.get_mut(statement) {
                    s.page = None;
                    s.widths.clear();
                    s.page_no = page;
                    s.page_size = size;
                    s.status = Status::Running;
                    s.started = model_now;
                }
                t.row = 0;
            }
        },
        RunMsg::Batch { columns, rows } => {
            // A run's statement with rows shows as soon as they come.
            if kind == OpKind::Run && columns.is_some() && !t.statements.is_empty() {
                t.shown = Some(t.statements.len() - 1);
            }
            let Some(s) = target(t, kind, None) else {
                return Vec::new();
            };
            if let Some(columns) = columns {
                if s.page.is_none() {
                    s.widths = columns
                        .iter()
                        .map(|c| {
                            display_width(&grid::clean(c))
                                .clamp(grid::MIN_COLUMN_WIDTH, grid::MAX_COLUMN_WIDTH)
                        })
                        .collect();
                    s.page = Some(Page {
                        sql: s.sql.clone(),
                        columns,
                        rows: Vec::new(),
                        page: s.page_no,
                        page_size: s.page_size,
                        total_rows: 0,
                        total_pages: 1,
                        count_estimated: false,
                        elapsed_ms: 0.0,
                    });
                }
            }
            let Some(page) = s.page.as_mut() else {
                return Vec::new();
            };
            for row in &rows {
                for (i, cell) in row.iter().enumerate() {
                    if let Some(w) = s.widths.get_mut(i) {
                        let cw = display_width(&grid::display(cell)).min(grid::MAX_COLUMN_WIDTH);
                        *w = (*w).max(cw);
                    }
                }
            }
            page.rows.extend(rows);
            // A streamed statement (`:all`, or one with its own LIMIT) keeps
            // at most ROW_CAP rows, then stops the stream.
            // Core's own cap (`max_query_rows`) is the same number and fails
            // the statement one row past it, so the stream stops on reaching
            // the cap (a result of exactly that many rows says it stopped).
            let streamed = page_size == 0 || s.kind == Some(StatementKind::Stream);
            if streamed && page.rows.len() >= ROW_CAP {
                page.rows.truncate(ROW_CAP);
                page.total_rows = ROW_CAP as u64;
                s.capped = true;
                // The time up to the stop, to the tick.
                let elapsed_ms = match (s.started, model_now) {
                    (Some(start), Some(now)) => {
                        now.saturating_duration_since(start).as_micros() as f64 / 1000.0
                    }
                    _ => 0.0,
                };
                s.status = Status::Done {
                    elapsed_ms,
                    rows_affected: None,
                };
                let statements = t.statements.len() as u32;
                if let Some(op) = t.op.take() {
                    effects.push(Effect::CancelRun {
                        tab: t.id,
                        stream_id: Some(op.stream_id),
                    });
                }
                t.finished = Some((statements, true));
                t.started = None;
                t.shown = Some(t.statements.len() - 1);
                effects.push(Model::log_effect(
                    None,
                    format!("{}; {}", text::row_cap(ROW_CAP), text::NO_HISTORY_ROW),
                ));
            }
        }
        RunMsg::Done {
            index,
            elapsed_ms,
            total_rows,
            total_pages,
            count_estimated,
            rows_affected,
        } => {
            let Some(s) = target(t, kind, Some(index)) else {
                return Vec::new();
            };
            s.status = Status::Done {
                elapsed_ms,
                rows_affected,
            };
            if let Some(page) = &mut s.page {
                page.total_rows = total_rows;
                page.total_pages = total_pages;
                page.count_estimated = count_estimated;
                page.elapsed_ms = elapsed_ms;
            }
            effects.push(statement_line(&s.sql, elapsed_ms));
        }
        RunMsg::Failed {
            index,
            error,
            elapsed_ms,
            sql,
        } => {
            let exists = target(t, kind, Some(index)).is_some();
            if !exists {
                t.statements.push(StatementResult {
                    index,
                    sql: sql.unwrap_or_default(),
                    source: None,
                    kind: None,
                    page: None,
                    page_no: 1,
                    page_size,
                    widths: Vec::new(),
                    status: Status::Running,
                    capped: false,
                    started: None,
                });
            }
            if let Some(s) = target(t, kind, Some(index)) {
                let _ = elapsed_ms;
                effects.push(Model::log_effect(
                    Some(Tag::Error),
                    text::statement_failed(&error.code),
                ));
                s.status = Status::Failed(error);
            }
        }
        RunMsg::Finished {
            statements,
            succeeded,
            history,
        } => {
            let op = t.op.take();
            t.started = None;
            if kind == OpKind::Run {
                t.finished = Some((statements, succeeded));
                t.shown = t.statements.iter().rposition(|s| s.page.is_some());
                t.result_tab = if t.shown.is_some() {
                    ResultTab::Results
                } else {
                    ResultTab::Messages
                };
                if statements == 0 {
                    effects.push(Model::log_effect(None, text::NOTHING_TO_RUN));
                }
            }
            if let (Some(row), Some(connection)) = (history, op.and_then(|o| o.connection_id)) {
                commit::add_history(model, &connection, vec![row]);
            }
        }
        RunMsg::Refused { error, destructive } => {
            let op = t.op.take();
            t.started = None;
            let pending = op.and_then(|o| o.pending);
            match (error.code.as_str(), destructive, pending) {
                ("CONFIRM_REQUIRED", Some((list, total)), Some(pending)) => {
                    model.modal = Some(Modal::RunConfirm(RunConfirm {
                        kind: ConfirmKind::Destructive {
                            list,
                            total,
                            from_core: true,
                        },
                        typed: String::new(),
                        then: Confirmed::Run(pending),
                    }));
                }
                _ => {
                    effects.push(Model::log_effect(
                        Some(Tag::Error),
                        text::failed_line("run", &error.code),
                    ));
                    t.run_error = Some(error);
                    t.result_tab = ResultTab::Messages;
                }
            }
        }
        RunMsg::Ended => {
            effects.extend(cancel_tab(t));
            effects.retain(|e| !matches!(e, Effect::CancelRun { .. }));
            effects.push(Model::log_effect(None, text::CANCELLED));
        }
    }
    effects
}

// ── Dialogs ──

/// Whether a field of this module's dialogs takes printable keys now.
pub fn dialog_typing(model: &Model) -> bool {
    match &model.modal {
        Some(Modal::Params(_)) | Some(Modal::SaveAs(_)) => true,
        Some(Modal::RunConfirm(_)) => prod(model),
        _ => false,
    }
}

/// How long a typed field may grow.
const FIELD_MAX: usize = 4096;

pub fn dialog_char(model: &mut Model, c: char) {
    let field = match &mut model.modal {
        Some(Modal::Params(form)) => form.values.get_mut(form.field),
        Some(Modal::SaveAs(save)) => Some(&mut save.name),
        Some(Modal::RunConfirm(confirm)) => Some(&mut confirm.typed),
        _ => None,
    };
    if let Some(field) = field {
        if field.chars().count() < FIELD_MAX {
            field.push(c);
        }
    }
}

pub fn dialog_backspace(model: &mut Model) {
    match &mut model.modal {
        Some(Modal::Params(form)) => {
            if let Some(v) = form.values.get_mut(form.field) {
                v.pop();
            }
        }
        Some(Modal::SaveAs(save)) => {
            save.name.pop();
        }
        Some(Modal::RunConfirm(confirm)) => {
            confirm.typed.pop();
        }
        _ => {}
    }
}

/// Tab and Shift+Tab in the parameter form.
pub fn params_field(model: &mut Model, forward: bool) {
    if let Some(Modal::Params(form)) = &mut model.modal {
        let n = form.names.len().max(1);
        form.field = (form.field + if forward { 1 } else { n - 1 }) % n;
    }
}

/// Enter in the parameter form.
pub fn params_submit(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::Params(form)) = model.modal.take() else {
        return Vec::new();
    };
    let Some(engine) = engine(model) else {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_RUN)];
    };
    let values: Vec<(String, String)> = form.names.into_iter().zip(form.values).collect();
    if let Some(tab) = model.query.tab_mut(form.pending.tab) {
        for (name, value) in &values {
            match tab.params.iter_mut().find(|(k, _)| k == name) {
                Some(slot) => slot.1 = value.clone(),
                None => tab.params.push((name.clone(), value.clone())),
            }
        }
    }
    let pending = PendingRun {
        params: Some(values),
        ..form.pending
    };
    check_and_go(model, pending, engine)
}

/// Enter in the run confirmation.
pub fn confirm_run(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::RunConfirm(confirm)) = &model.modal else {
        return Vec::new();
    };
    if prod(model) && confirm.typed != PROD_LABEL {
        return Vec::new();
    }
    let Some(Modal::RunConfirm(confirm)) = model.modal.take() else {
        return Vec::new();
    };
    match confirm.then {
        Confirmed::Run(pending) => go(model, pending, true),
        Confirmed::Analyze(call) => start_explain(model, call),
    }
}

/// Whether the run confirmation asks for `prod` (panel 1's connection
/// carries the predefined label).
pub fn prod(model: &Model) -> bool {
    model
        .conn
        .id()
        .and_then(|id| model.library.connection(id))
        .is_some_and(|c| c.label_ids.iter().any(|l| l == PROD_LABEL))
}

// ── The results ──

fn shown_page(model: &Model) -> Option<&Page> {
    model.query.active()?.shown_statement()?.page.as_ref()
}

pub fn results_move(model: &mut Model, dx: isize, dy: isize) {
    let Some((rows, cols)) = shown_page(model).map(|p| (p.rows.len(), p.columns.len())) else {
        return;
    };
    if let Some(tab) = model.query.active_mut() {
        tab.row = tab
            .row
            .saturating_add_signed(dy)
            .min(rows.saturating_sub(1));
        tab.col = tab
            .col
            .saturating_add_signed(dx)
            .min(cols.saturating_sub(1));
    }
}

pub fn results_edge(model: &mut Model, last: bool) {
    let rows = shown_page(model).map_or(0, |p| p.rows.len());
    if let Some(tab) = model.query.active_mut() {
        tab.row = if last { rows.saturating_sub(1) } else { 0 };
    }
}

/// `n`/`p`: `db.page` for the shown statement.
pub fn page(model: &mut Model, forward: bool) -> Vec<Effect> {
    // Only on the saved connection the results came from, through its Core
    // id as it is now (a reconnect is followed).
    let from = model
        .query
        .active()
        .and_then(|t| t.results_connection.clone());
    let core_id = match (model.conn.core_id(), &from) {
        (Some(core_id), Some(from)) if model.conn.id() == Some(from.as_str()) => {
            core_id.to_string()
        }
        (_, Some(from)) => {
            let name = model
                .library
                .connection(from)
                .map_or(from.as_str(), |c| c.name.as_str());
            return vec![Model::log_effect(
                Some(Tag::Error),
                text::results_elsewhere(name),
            )];
        }
        _ => return Vec::new(),
    };
    let op = model.query.next_op + 1;
    let Some(tab) = model.query.active_mut() else {
        return Vec::new();
    };
    if tab.op.is_some() {
        return Vec::new();
    }
    let Some(index) = tab.shown else {
        return Vec::new();
    };
    let s = &tab.statements[index];
    let (Some(source), Some(StatementKind::Page), Some(page)) = (&s.source, s.kind, &s.page) else {
        return Vec::new();
    };
    let next = if forward {
        if !page.has_next() {
            return Vec::new();
        }
        page.page + 1
    } else {
        if page.page <= 1 {
            return Vec::new();
        }
        page.page - 1
    };
    let stream_id = format!("tui-page-{}-{op}", tab.id);
    let call = PageRunCall {
        tab: tab.id,
        op,
        stream_id: stream_id.clone(),
        core_id,
        source: source.clone(),
        page: next,
        page_size: page.page_size,
    };
    tab.op = Some(Op {
        op,
        stream_id,
        kind: OpKind::Page { statement: index },
        page_size: page.page_size,
        pending: None,
        connection_id: None,
        core_id: call.core_id.clone(),
    });
    model.query.next_op = op;
    vec![Effect::PageRun(call)]
}

/// `(`/`)`: the previous or next statement with rows.
pub fn statement_step(model: &mut Model, forward: bool) {
    let Some(tab) = model.query.active_mut() else {
        return;
    };
    let with_rows: Vec<usize> = (0..tab.statements.len())
        .filter(|&i| tab.statements[i].page.is_some())
        .collect();
    let Some(pos) = tab
        .shown
        .and_then(|s| with_rows.iter().position(|&i| i == s))
    else {
        return;
    };
    let next = if forward {
        with_rows.get(pos + 1)
    } else {
        pos.checked_sub(1).and_then(|p| with_rows.get(p))
    };
    if let Some(&next) = next {
        tab.shown = Some(next);
        tab.row = 0;
        tab.col = 0;
    }
}

/// Enter: the cell full size.
pub fn open_cell(model: &mut Model) {
    let Some(tab) = model.query.active() else {
        return;
    };
    let Some(page) = tab.shown_statement().and_then(|s| s.page.as_ref()) else {
        return;
    };
    let (Some(cell), Some(column)) = (
        page.rows.get(tab.row).and_then(|r| r.get(tab.col)),
        page.columns.get(tab.col),
    ) else {
        return;
    };
    let text = match cell {
        Value::Null => "NULL".to_string(),
        Value::Json(json) => serde_json::to_string_pretty(json).unwrap_or_default(),
        other => grid::cell_text(other),
    };
    model.modal = Some(Modal::Cell(CellView {
        column: column.clone(),
        text,
        scroll: 0,
    }));
}

/// Esc in the results: cancel what runs, else back to the editor.
pub fn results_back(model: &mut Model) -> Vec<Effect> {
    let busy = model
        .query
        .active()
        .is_some_and(|t| t.op.is_some() || t.explaining());
    if busy {
        return cancel(model);
    }
    model.query.pane = Pane::Editor;
    Vec::new()
}

pub fn cell_scroll(model: &mut Model, down: bool) {
    if let Some(Modal::Cell(cell)) = &mut model.modal {
        let lines = cell.text.lines().count();
        cell.scroll = if down {
            (cell.scroll + 1).min(lines.saturating_sub(1))
        } else {
            cell.scroll.saturating_sub(1)
        };
    }
}

// ── Explain ──

/// Ctrl+X (plain) or Alt+X (ANALYZE) on the statement at the cursor.
pub fn explain(model: &mut Model, analyze: bool) -> Vec<Effect> {
    let Some(core_id) = model.conn.core_id().map(str::to_string) else {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_RUN)];
    };
    let Some(engine) = engine(model) else {
        return vec![Model::log_effect(Some(Tag::Error), text::NOT_CONNECTED_RUN)];
    };
    let Some(tab) = model.query.active() else {
        return Vec::new();
    };
    let text = tab.editor.text();
    let Some(statement) = statement_at(&text, tab.editor.cursor_byte(), engine) else {
        return vec![Model::log_effect(
            Some(Tag::Error),
            text::NOTHING_TO_EXPLAIN,
        )];
    };
    let sql = text[statement.text].to_string();
    if has_parameters(&sql) {
        return vec![Model::log_effect(Some(Tag::Error), text::EXPLAIN_PARAMS)];
    }
    let tab_id = tab.id;
    let op = next_op(model);
    let call = ExplainCall {
        tab: tab_id,
        op,
        core_id,
        sql,
        analyze,
    };
    if analyze && !plain_select(&call.sql, engine) {
        let verb = tokens(&call.sql, engine)
            .first()
            .map(|t| t.text(&call.sql).to_ascii_uppercase())
            .unwrap_or_default();
        model.modal = Some(Modal::RunConfirm(RunConfirm {
            kind: ConfirmKind::Analyze { verb },
            typed: String::new(),
            then: Confirmed::Analyze(call),
        }));
        return Vec::new();
    }
    start_explain(model, call)
}

/// Whether EXPLAIN ANALYZE may run `sql` without asking: a
/// SELECT by `query_type`, no destructive reason, and none of `INTO`,
/// `FOR UPDATE`/`FOR SHARE` (`FOR NO KEY UPDATE`, `FOR KEY SHARE`),
/// `nextval`, `setval`, or a data-changing verb (a CTE's `DELETE`) among
/// its tokens. Words in strings, comments and quoted names aren't tokens
/// of that kind, so they don't count.
pub fn plain_select(sql: &str, engine: SqlEngine) -> bool {
    if query_type(sql, engine) != QueryType::Select || destructive_reason(sql, engine).is_some() {
        return false;
    }
    let words: Vec<String> = tokens(sql, engine)
        .iter()
        .filter(|t| t.kind == seaquel_core::sql::scan::TokenKind::Word)
        .map(|t| t.text(sql).to_ascii_uppercase())
        .collect();
    !words.iter().enumerate().any(|(i, w)| match w.as_str() {
        "INTO" | "NEXTVAL" | "SETVAL" | "INSERT" | "UPDATE" | "DELETE" | "MERGE" => true,
        "FOR" => words
            .get(i + 1)
            .is_some_and(|n| matches!(n.as_str(), "UPDATE" | "SHARE" | "NO" | "KEY")),
        _ => false,
    })
}

fn start_explain(model: &mut Model, call: ExplainCall) -> Vec<Effect> {
    let Some(tab) = model.query.tab_mut(call.tab) else {
        return Vec::new();
    };
    tab.explain = Some(ExplainView::Loading {
        analyze: call.analyze,
    });
    tab.explain_op = Some(call.op);
    tab.result_tab = ResultTab::Explain;
    vec![Effect::Explain(call)]
}

pub fn on_explained(
    model: &mut Model,
    tab: u64,
    op: u64,
    result: Result<Box<ExplainResult>, CallError>,
) -> Vec<Effect> {
    let Some(t) = model.query.tab_mut(tab) else {
        return Vec::new();
    };
    if t.explain_op != Some(op) {
        return Vec::new();
    }
    t.explain_op = None;
    match result {
        Ok(plan) => {
            t.explain = Some(ExplainView::Loaded(plan));
            Vec::new()
        }
        Err(e) => {
            let line = Model::log_effect(Some(Tag::Error), text::failed_line("explain", &e.code));
            t.explain = Some(ExplainView::Failed(e));
            vec![line]
        }
    }
}

// ── Saving ──

/// Ctrl+S and `:w`.
pub fn save(model: &mut Model) -> Vec<Effect> {
    let Some(tab) = model.query.active() else {
        return Vec::new();
    };
    if let Some(saved) = &tab.saved {
        return vec![Effect::SaveQuery(SaveQueryCall {
            tab: tab.id,
            save: SaveKind::Update {
                id: saved.id.clone(),
            },
            text: tab.editor.text(),
            detached: false,
        })];
    }
    if model.project.is_none() {
        return vec![Model::log_effect(Some(Tag::Error), text::NEEDS_PROJECT)];
    }
    model.modal = Some(Modal::SaveAs(SaveAs {
        tab: tab.id,
        name: String::new(),
        error: None,
        text: None,
        back: None,
    }));
    Vec::new()
}

/// Enter in the name prompt.
pub fn save_as_submit(model: &mut Model) -> Vec<Effect> {
    let Some(Modal::SaveAs(save)) = &mut model.modal else {
        return Vec::new();
    };
    let name = save.name.trim().to_string();
    if name.is_empty() {
        save.error = Some(text::NAME_NEEDED.to_string());
        return Vec::new();
    }
    let tab = save.tab;
    // Ask AI's SQL (its own text), else the tab's.
    let detached = save.text.is_some();
    let own = save.text.as_ref().map(|t| t.0.clone());
    let back = save.back.take();
    let text = own.or_else(|| model.query.tab(tab).map(|t| t.editor.text()));
    model.modal = back.map(|ask| Modal::Ask(*ask));
    let (Some(project_id), Some(text)) = (model.project.clone(), text) else {
        return Vec::new();
    };
    vec![Effect::SaveQuery(SaveQueryCall {
        tab,
        save: SaveKind::Create { project_id, name },
        text,
        detached,
    })]
}

pub fn on_saved(
    model: &mut Model,
    tab: u64,
    SqlText(text): SqlText,
    result: Result<SavedItem, CallError>,
    taken_by: Option<String>,
    detached: bool,
) -> Vec<Effect> {
    if detached {
        return super::ask::on_saved(model, tab, SqlText(text), result, taken_by);
    }
    match result {
        Ok(item) => {
            let mut effects = vec![Model::log_effect(None, text::saved_line(&item.name))];
            if item.shared {
                effects.push(Model::log_effect(None, text::SAVED_SHARED));
            }
            if let Some(t) = model.query.tab_mut(tab) {
                t.title = item.name.clone();
                t.saved = Some(SavedRef {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    shared: item.shared,
                });
                t.stored = text;
                t.checked_gen = None;
            }
            if model.project.is_some() {
                match model.saved_items.iter_mut().find(|s| s.id == item.id) {
                    Some(row) => *row = item,
                    None => model.saved_items.push(item),
                }
                model.refresh_lists();
            }
            effects
        }
        Err(e) if e.code == "NAME_TAKEN" => {
            let holder = taken_by
                .as_deref()
                .and_then(|id| model.saved_items.iter().find(|s| s.id == id))
                .map(|s| s.name.as_str());
            let message = text::name_taken(holder);
            if model.query.tab(tab).is_some_and(|t| t.saved.is_none()) {
                model.modal = Some(Modal::SaveAs(SaveAs {
                    tab,
                    name: String::new(),
                    error: Some(message),
                    text: None,
                    back: None,
                }));
                Vec::new()
            } else {
                model.modal = Some(Modal::Notice(Notice(message)));
                Vec::new()
            }
        }
        Err(e) => {
            model.modal = Some(Modal::Notice(Notice(text::apply_failed(
                &e.code, &e.message,
            ))));
            vec![Model::log_effect(
                Some(Tag::Error),
                text::failed_line("save", &e.code),
            )]
        }
    }
}

// ── $EDITOR ──

/// Ctrl+O: the tab's text in `$VISUAL`/`$EDITOR`.
pub fn external_editor(model: &mut Model) -> Vec<Effect> {
    let Some(tab) = model.query.active() else {
        return Vec::new();
    };
    vec![Effect::ExternalEditor {
        tab: tab.id,
        text: tab.editor.text().into(),
    }]
}

pub fn on_edited(model: &mut Model, tab: u64, result: Result<SqlText, String>) -> Vec<Effect> {
    match result {
        Ok(SqlText(new)) => {
            if let Some(t) = model.query.tab_mut(tab) {
                if t.editor.text() != new {
                    t.editor.set_text(&new);
                }
            }
            Vec::new()
        }
        Err(why) => {
            model.modal = Some(Modal::Notice(Notice(text::editor_failed(&why))));
            vec![Model::log_effect(
                Some(Tag::Error),
                text::failed_line("$EDITOR", "EDITOR_FAILED"),
            )]
        }
    }
}

// ── The state file ──

/// The open tabs into [`Model::remembered`], for the state file.
pub fn remember(model: &mut Model) {
    model.remembered.query_tabs = model.query.tabs.iter().map(remembered).collect();
    model.remembered.query_active = model.query.active;
}

/// One tab as the state file keeps it.
fn remembered(t: &QueryTab) -> RememberedTab {
    let saved_id = t.saved.as_ref().map(|s| s.id.clone());
    // Still waiting for the library: as it was read.
    match t.awaiting_library {
        Some(Awaiting::Text) => {
            return RememberedTab {
                saved_id,
                text: None,
                stored_hash: t.restored_hash.clone(),
                omitted: false,
            }
        }
        Some(Awaiting::Stored) => {
            return RememberedTab {
                saved_id,
                text: Some(t.editor.text()),
                stored_hash: t.restored_hash.clone(),
                omitted: false,
            }
        }
        None => {}
    }
    let stored_hash = saved_id.as_ref().map(|_| text_hash(&t.stored));
    if saved_id.is_some() && !t.modified {
        return RememberedTab {
            saved_id,
            text: None,
            stored_hash,
            omitted: false,
        };
    }
    let bytes: usize = t.editor.lines().iter().map(|l| l.len() + 1).sum();
    let empty_with_notice = t.notice.is_some() && bytes <= 1;
    if bytes > MAX_REMEMBERED_TEXT + 1 || empty_with_notice {
        return RememberedTab {
            saved_id,
            text: None,
            stored_hash,
            omitted: true,
        };
    }
    RememberedTab {
        saved_id,
        text: Some(t.editor.text()),
        stored_hash,
        omitted: false,
    }
}

/// The remembered tabs, opened again (the query view not shown).
pub fn restore(model: &mut Model, tabs: &[RememberedTab], active: usize) {
    for remembered in tabs {
        let q = &mut model.query;
        q.next_tab += 1;
        let (title, saved) = match &remembered.saved_id {
            // The name (and text) come from the library once it's read
            // (`sync`).
            Some(id) => (
                text::SAVED_TAB.to_string(),
                Some(SavedRef {
                    id: id.clone(),
                    name: String::new(),
                    shared: false,
                }),
            ),
            None => {
                q.untitled += 1;
                (text::untitled(q.untitled), None)
            }
        };
        let is_saved = saved.is_some();
        let text = remembered.text.as_deref().unwrap_or("");
        let mut tab = QueryTab::new(q.next_tab, title, text, saved);
        tab.stored = String::new();
        tab.restored_hash = remembered.stored_hash.clone();
        if remembered.omitted {
            tab.notice = Some(text::TAB_NOT_KEPT.to_string());
        } else if is_saved {
            tab.awaiting_library = Some(if remembered.text.is_some() {
                Awaiting::Stored
            } else {
                Awaiting::Text
            });
        }
        q.tabs.push(tab);
    }
    let q = &mut model.query;
    q.active = active.min(q.tabs.len().saturating_sub(1));
}

// ── After every message ──

/// The editor's text rows and columns at the current size.
pub fn editor_size(model: &Model) -> Option<(usize, usize)> {
    let area = ratatui::layout::Rect::new(0, 0, model.size.0, model.size.1);
    let areas = layout::areas(area, model.ctx)?;
    let (editor, _) = layout::query_areas(areas.main);
    let lines = model.query.active()?.editor.lines().len();
    let text = layout::editor_text_area(editor, lines);
    Some((usize::from(text.height), usize::from(text.width)))
}

/// Keeps each tab's `modified` flag, the active editor's highlighting (for
/// a text under `HIGHLIGHT_NOW_BYTES`) and its scroll in step (`update`
/// calls it after every message).
pub fn sync(model: &mut Model) {
    // A saved query's tab takes its name from the library (a restored tab
    // knows only the id).
    for tab in &mut model.query.tabs {
        if let Some(saved) = tab.saved.as_mut().filter(|s| s.name.is_empty()) {
            if let Some(item) = model.saved_items.iter().find(|i| i.id == saved.id) {
                saved.name = item.name.clone();
                saved.shared = item.shared;
                tab.title = item.name.clone();
                match tab.awaiting_library.take() {
                    Some(Awaiting::Text) => {
                        tab.editor = Editor::new(&item.sql);
                        tab.stored = item.sql.clone();
                    }
                    Some(Awaiting::Stored) => tab.stored = item.sql.clone(),
                    None => {}
                }
                tab.restored_hash = None;
                tab.checked_gen = None;
            }
        }
    }
    // The state file is written again when a tab opens, closes, changes or
    // becomes active.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    {
        use std::hash::{Hash, Hasher};
        model.query.active.hash(&mut hasher);
        for tab in &model.query.tabs {
            (tab.id, tab.editor.gen(), tab.saved.as_ref().map(|s| &s.id)).hash(&mut hasher);
        }
        let sig = hasher.finish();
        if model.query.remembered_sig != Some(sig) {
            let first = model.query.remembered_sig.is_none();
            model.query.remembered_sig = Some(sig);
            if !first || !model.query.tabs.is_empty() {
                model.remember();
            }
        }
    }
    for tab in &mut model.query.tabs {
        let gen = tab.editor.gen();
        if tab.checked_gen != Some(gen) {
            tab.modified = tab.awaiting_library.is_none() && !tab.editor.text_is(&tab.stored);
            tab.checked_gen = Some(gen);
        }
    }
    if !model.query.shown {
        return;
    }
    let engine = editor_engine(model);
    let size = editor_size(model);
    let Some(tab) = model.query.active_mut() else {
        return;
    };
    let bytes: usize = tab.editor.lines().iter().map(|l| l.len() + 1).sum();
    if bytes <= HIGHLIGHT_NOW_BYTES {
        tab.editor.refresh_highlight(engine);
    }
    if let Some((height, width)) = size {
        tab.editor.scroll_into_view(height, width);
    }
}

/// A tick: highlighting for a large text, once it has changed.
pub fn on_tick(model: &mut Model) {
    if !model.query.shown {
        return;
    }
    let engine = editor_engine(model);
    if let Some(tab) = model.query.active_mut() {
        tab.editor.refresh_highlight(engine);
    }
}

#[cfg(test)]
mod tests;
