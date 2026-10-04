//! The model and `update` (Decision 9; spike S3's split). `update` is pure:
//! no Core, no I/O and no clock (time comes in [`Msg::Tick`]), so every key
//! path is tested without a terminal. It returns [`Effect`]s, which the
//! runtime carries out.
//!
//! The focus model is the prototype's (`seaquel-tui-browse-logic.js.txt`):
//! `1`–`4` focus a panel and `0` the main view; the main view shows the
//! last list panel focused (`ctx`); Tab cycles all five; `h`/`l` in a panel
//! cycle the four panels; `[`/`]` switch the focused panel's tab, or in the
//! main view the table's tabs.

use std::collections::BTreeSet;
use std::fmt;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::layout::Rect;

use seaquel_core::domain::edits::{PlannedChange, TableTarget};

use super::ask::{self, Ask, GenerateCall};
use super::browse::{self, Browse, PageCall, PlanCall, TableMeta};
use super::commit::{self, Applied, ApplyCall, CommitDialog, Committing, QueueSwitch, ValueEdit};
use super::connect;
use super::dialogs::{CallError, Notice, PasswordPrompt, Problem, TrustPrompt};
use super::grid::Page;
use super::install;
use super::keymap::{self, Action, BarContext, Key};
use super::log::{CommandLog, LogLine, Tag};
use super::panels::{HistoryItem, Library, Row, SavedItem, TableItem};
use super::pending::Queue;
use super::picker::{Picker, Remembered};
use super::query::{
    self, CellView, ExplainCall, PageRunCall, Pane, ParamsForm, Query, RunCall, RunConfirm,
    RunKind, RunMsg, SaveAs, SaveQueryCall,
};
use super::secrets::{SecretKind, Typed};
use super::text;
use crate::view::layout;
use crate::view::theme::Theme;

/// How long a keychain call may be pending before the wait box shows: a
/// store that answers at once (a saved password the OS already allows)
/// never flashes it.
pub const KEYCHAIN_BOX_AFTER: Duration = Duration::from_millis(250);

/// How long a keychain call may stay pending before the connect gives up
/// (`SecretWait`'s default limit: a dialog nobody answers).
pub const KEYCHAIN_LIMIT: Duration = Duration::from_secs(300);

/// How long after a change the state file is written (Q4 A).
pub const STATE_FILE_DELAY: Duration = Duration::from_millis(500);

/// A focusable box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Connection,
    Tables,
    Saved,
    Pending,
    Main,
}

impl Panel {
    /// Tab order (the prototype's `P`).
    pub const ORDER: [Panel; 5] = [
        Panel::Connection,
        Panel::Tables,
        Panel::Saved,
        Panel::Pending,
        Panel::Main,
    ];

    /// The number its title shows and its key focuses (`0` the main view).
    pub fn number(self) -> u8 {
        match self {
            Panel::Connection => 1,
            Panel::Tables => 2,
            Panel::Saved => 3,
            Panel::Pending => 4,
            Panel::Main => 0,
        }
    }
}

/// Panel 2's tabs. Functions and Enums are deferred (Q9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablesTab {
    Tables,
    Views,
}

/// Panel 3's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedTab {
    Saved,
    History,
}

/// A list's selection and length. The rows live in the model
/// (`schema`, `saved_items`, …); [`Model::refresh_lists`] keeps `len` in
/// step with them; `commit::sync` keeps panel 4's in step with the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct List {
    pub selected: usize,
    pub len: usize,
}

impl List {
    /// Moves the selection by `delta`, within the list (the prototype's
    /// `clamp`).
    fn step(&mut self, delta: isize) {
        if self.len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = self.selected.saturating_add_signed(delta).min(self.len - 1);
    }
}

/// Staged changes by kind (`queue.counts()`, kept by `commit::sync`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Staged {
    pub inserts: usize,
    pub updates: usize,
    pub deletes: usize,
}

impl Staged {
    pub fn total(&self) -> usize {
        self.inserts + self.updates + self.deletes
    }
}

/// A dialog over everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Modal {
    /// `?`, scrolled by `scroll` lines.
    Help {
        scroll: usize,
    },
    /// "Quit?"
    ConfirmQuit,
    /// A project, then a connection.
    Picker(Picker),
    /// A password the connection doesn't save.
    Password(PasswordPrompt),
    /// An unknown SSH host key.
    Trust(TrustPrompt),
    /// A failed connect.
    Problem(Problem),
    Notice(Notice),
    /// `c`: commit the staged changes (Task 5).
    Commit(CommitDialog),
    /// `D` in panel 4: "Discard N staged changes?"
    ConfirmDiscard,
    /// `c` on a queue a commit may have partly applied (it lost its
    /// connection): "Commit again?"
    ConfirmRecommit,
    /// Changes are staged on another connection: keep, discard or stay.
    QueueSwitch(QueueSwitch),
    /// `e` in panel 4: a staged value being edited.
    EditValue(ValueEdit),
    /// A run's `{{param}}` values (Task 6).
    Params(ParamsForm),
    /// A destructive run or an EXPLAIN ANALYZE of a write: run it?
    RunConfirm(RunConfirm),
    /// A new saved query's name.
    SaveAs(SaveAs),
    /// A result cell full size.
    Cell(CellView),
    /// Ask AI over the query tab (Task 7).
    Ask(Ask),
    /// DuckDB support isn't installed: its download (the DuckDB helper
    /// plan, Task 7).
    InstallDuckdb(install::InstallDialog),
}

/// Panel 1's connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conn {
    /// Nothing chosen yet.
    None,
    /// Core is connecting (`attempt` tells a late answer from the current).
    Connecting(Attempt),
    Connected {
        id: String,
        /// Core's connection id.
        core_id: String,
    },
    /// The last connect failed or was given up.
    Failed { id: String },
    /// Core closed it.
    Closed { id: String },
}

impl Conn {
    /// The saved connection panel 1 is about, if any.
    pub fn id(&self) -> Option<&str> {
        match self {
            Conn::None => None,
            Conn::Connecting(a) => Some(&a.pending.connection_id),
            Conn::Connected { id, .. } | Conn::Failed { id } | Conn::Closed { id } => Some(id),
        }
    }

    /// Core's id while connected.
    pub fn core_id(&self) -> Option<&str> {
        match self {
            Conn::Connected { core_id, .. } => Some(core_id),
            _ => None,
        }
    }
}

/// A connect in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    pub attempt: u64,
    pub pending: super::dialogs::Pending,
}

/// What a list holds now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Load {
    #[default]
    Idle,
    Loading,
    Loaded,
    Failed,
}

/// The whole state the view draws.
#[derive(Debug, Clone)]
pub struct Model {
    pub focus: Panel,
    /// The list panel the main view shows (never `Connection` or `Main`).
    pub ctx: Panel,
    pub tables_tab: TablesTab,
    pub saved_tab: SavedTab,
    /// The main view's tab, for a table.
    pub main_tab: usize,
    pub tables: List,
    pub views: List,
    pub saved: List,
    pub history: List,
    pub pending: List,
    pub staged: Staged,
    pub modal: Option<Modal>,
    /// The terminal's size, columns × rows.
    pub size: (u16, u16),
    /// The last tick's time.
    pub now: Option<Instant>,
    pub log: CommandLog,
    pub theme: Theme,
    /// The saved projects and connections.
    pub library: Library,
    /// The project panel 3 shows.
    pub project: Option<String>,
    pub conn: Conn,
    /// The secrets typed for the connected connection (Decision 16):
    /// dropped when it disconnects or closes.
    pub typed: Typed,
    /// Panel 2: the connected database's tables, views and materialized
    /// views, as Core listed them. Their columns are filled in as they're
    /// read (`table_metadata`): `schema_tables` lists none (probe F1).
    pub schema: Vec<TableItem>,
    /// The column reads asked for, by `(schema, table)`, on the connected
    /// connection; cleared with `schema`.
    pub column_loads: std::collections::BTreeMap<(String, String), Load>,
    pub schema_load: Load,
    /// Folded schemas (panel 2) and folders (panel 3).
    pub folded_schemas: BTreeSet<String>,
    pub folded_folders: BTreeSet<String>,
    /// Panel 3: the project's saved queries and the connection's history.
    pub saved_items: Vec<SavedItem>,
    pub history_items: Vec<HistoryItem>,
    /// Since when a keychain call is pending (`SecretWait`), if one is.
    pub keychain: Option<Instant>,
    /// Esc hid the wait box during a save (which goes on).
    pub keychain_hidden: bool,
    /// The pending keychain call belongs to a connect that was given up:
    /// ignored until it ends (`Keychain { pending: false }`).
    pub keychain_stale: bool,
    /// A password being saved for this connection.
    pub saving: Option<String>,
    /// A connect found no secret store in this session (Core's
    /// `SECRET_STORE_UNAVAILABLE`, probe F4): from then on the prompts ask
    /// for what the store would hold, and "Save password" is off.
    pub store_unavailable: bool,
    /// The platform's secret store, which the keychain dialogs name
    /// (`Store::here()`; fixed in tests, so snapshots don't vary by OS).
    pub store: text::Store,
    /// The kitty keyboard flags are pushed: Esc arrives as a key of its
    /// own and never merges with the next (set by the runtime).
    pub kitty_keys: bool,
    /// What the state file keeps.
    pub remembered: Remembered,
    /// The state file changed at this tick (written [`STATE_FILE_DELAY`]
    /// later).
    pub remember_dirty: Option<Option<Instant>>,
    pub next_attempt: u64,
    /// The DuckDB helper's lookups and downloads (Task 7 of the DuckDB
    /// helper plan).
    pub next_install: u64,
    /// The opened table: its page, metadata, cursor and filters (Task 4).
    pub browse: Browse,
    /// The staged changes (Decision 12); `staged` counts them.
    pub queue: Queue,
    /// Rows per page (`--page-size`, default 100).
    pub page_size: u32,
    /// The main view's tab over panel 4 (Diff, SQL).
    pub pending_tab: usize,
    /// An apply in flight (Task 5): staging waits for it.
    pub committing: Option<Committing>,
    pub next_apply: u64,
    /// The query tabs (Task 6).
    pub query: Query,
}

impl Model {
    pub fn new(size: (u16, u16), theme: Theme) -> Model {
        Model {
            focus: Panel::Tables,
            ctx: Panel::Tables,
            tables_tab: TablesTab::Tables,
            saved_tab: SavedTab::Saved,
            main_tab: 0,
            tables: List::default(),
            views: List::default(),
            saved: List::default(),
            history: List::default(),
            pending: List::default(),
            staged: Staged::default(),
            modal: None,
            size,
            now: None,
            log: CommandLog::default(),
            theme,
            library: Library::default(),
            project: None,
            conn: Conn::None,
            typed: Typed::default(),
            schema: Vec::new(),
            column_loads: Default::default(),
            schema_load: Load::Idle,
            folded_schemas: BTreeSet::new(),
            folded_folders: BTreeSet::new(),
            saved_items: Vec::new(),
            history_items: Vec::new(),
            keychain: None,
            keychain_hidden: false,
            keychain_stale: false,
            saving: None,
            store_unavailable: false,
            store: text::Store::here(),
            kitty_keys: false,
            remembered: Remembered::default(),
            remember_dirty: None,
            next_attempt: 1,
            next_install: 1,
            browse: Browse::default(),
            queue: Queue::default(),
            page_size: browse::DEFAULT_PAGE_SIZE,
            pending_tab: 0,
            committing: None,
            next_apply: 0,
            query: Query::default(),
        }
    }

    /// Whether a query tab's run (or page) is in flight.
    pub fn running(&self) -> bool {
        self.query.running()
    }

    /// Whether the keychain wait box shows: a call pending for at least
    /// [`KEYCHAIN_BOX_AFTER`] while a connect or a save waits on it.
    pub fn keychain_box(&self) -> bool {
        let Some(since) = self.keychain.filter(|_| !self.keychain_stale) else {
            return false;
        };
        let waiting = matches!(self.conn, Conn::Connecting(_))
            || (self.saving.is_some() && !self.keychain_hidden)
            || ask::waiting(self);
        waiting
            && self
                .now
                .is_some_and(|now| now.saturating_duration_since(since) >= KEYCHAIN_BOX_AFTER)
    }

    /// Which key bar (and keymap chain) applies now.
    pub fn bar_context(&self) -> BarContext {
        if self.keychain_box() {
            return BarContext::Keychain;
        }
        match &self.modal {
            Some(Modal::Help { .. }) => return BarContext::Help,
            Some(Modal::ConfirmQuit) => return BarContext::ConfirmQuit,
            Some(Modal::Picker(_)) => return BarContext::Picker,
            Some(Modal::Password(p)) if !p.can_save => return BarContext::PasswordNoSave,
            Some(Modal::Password(_)) => return BarContext::Password,
            Some(Modal::Trust(_)) => return BarContext::Trust,
            Some(Modal::Problem(p)) if p.reconnect.is_some() => {
                return BarContext::ProblemReconnect
            }
            Some(Modal::Problem(p)) => {
                return if p.retry.is_some() {
                    BarContext::ProblemRetry
                } else {
                    BarContext::Problem
                }
            }
            Some(Modal::Notice(_)) => return BarContext::Notice,
            Some(Modal::Commit(_)) => {
                return if commit::prod(self) {
                    BarContext::CommitProd
                } else {
                    BarContext::Commit
                }
            }
            Some(Modal::ConfirmDiscard) => return BarContext::ConfirmDiscard,
            Some(Modal::ConfirmRecommit) => return BarContext::ConfirmRecommit,
            Some(Modal::QueueSwitch(_)) => return BarContext::QueueSwitch,
            Some(Modal::EditValue(_)) => return BarContext::EditValue,
            Some(Modal::Params(_)) => return BarContext::Params,
            Some(Modal::RunConfirm(_)) => return BarContext::RunConfirm,
            Some(Modal::SaveAs(_)) => return BarContext::SaveAs,
            Some(Modal::Cell(_)) => return BarContext::Cell,
            Some(Modal::Ask(a)) => return a.bar_context(),
            Some(Modal::InstallDuckdb(d)) => return d.bar_context(),
            None => {}
        }
        if self.focus == Panel::Main && self.query.shown {
            if let Some(tab) = self.query.active() {
                if self.query.pane == Pane::Results {
                    return BarContext::Results;
                }
                let editor = &tab.editor;
                return if editor.command.is_some() {
                    BarContext::QueryCommand
                } else if editor.mode == super::editor::Mode::Normal {
                    BarContext::QueryNormal
                } else if editor.completion.is_some() {
                    BarContext::Completion
                } else {
                    BarContext::QueryInsert
                };
            }
        }
        match self.focus {
            Panel::Connection => BarContext::Connection,
            Panel::Tables => BarContext::Tables,
            Panel::Saved => BarContext::Saved,
            Panel::Pending => BarContext::Pending,
            Panel::Main if browse::shows_grid(self) => {
                let b = &self.browse;
                if b.editing.is_some() {
                    BarContext::CellEdit
                } else if b.finding {
                    BarContext::Find
                } else if b.form.is_some() {
                    BarContext::FilterForm
                } else {
                    BarContext::Grid
                }
            }
            Panel::Main => BarContext::Main,
        }
    }

    /// The mode the key bar's right side names, when one is on: the
    /// editor's with its line and column (`INSERT · Ln 7, Col 13`).
    pub fn mode(&self) -> Option<String> {
        let editor = |mode: &str| {
            let tab = self.query.active()?;
            let (row, col) = tab.editor.cursor();
            Some(text::editor_mode(mode, row + 1, col + 1))
        };
        match self.bar_context() {
            BarContext::CellEdit | BarContext::EditValue => Some(text::MODE_EDIT.to_string()),
            BarContext::Find | BarContext::FilterForm => Some(text::MODE_FILTER.to_string()),
            BarContext::QueryInsert | BarContext::Completion => editor(text::MODE_INSERT),
            BarContext::QueryNormal | BarContext::QueryCommand => editor(text::MODE_NORMAL),
            _ => None,
        }
    }

    /// Whether the main view shows a table or view (its tabs).
    fn main_shows_a_table(&self) -> bool {
        self.ctx == Panel::Tables
    }

    /// The main view's tabs for what it shows now.
    pub fn main_tabs(&self) -> &'static [&'static str] {
        match (self.ctx, self.tables_tab) {
            (Panel::Saved, _) => text::MAIN_TABS_SQL,
            (Panel::Pending, _) => text::MAIN_TABS_PENDING,
            (_, TablesTab::Views) => text::MAIN_TABS_VIEW,
            (_, TablesTab::Tables) => text::MAIN_TABS_TABLE,
        }
    }

    /// The main view's active tab index into [`Model::main_tabs`].
    pub fn main_tab_index(&self) -> usize {
        if self.main_shows_a_table() {
            self.main_tab
        } else if self.ctx == Panel::Pending {
            self.pending_tab
        } else {
            0
        }
    }

    /// The list a panel shows now.
    pub fn list(&self, panel: Panel) -> Option<&List> {
        match panel {
            Panel::Tables => Some(match self.tables_tab {
                TablesTab::Tables => &self.tables,
                TablesTab::Views => &self.views,
            }),
            Panel::Saved => Some(match self.saved_tab {
                SavedTab::Saved => &self.saved,
                SavedTab::History => &self.history,
            }),
            Panel::Pending => Some(&self.pending),
            Panel::Connection | Panel::Main => None,
        }
    }

    pub(crate) fn list_mut(&mut self, panel: Panel) -> Option<&mut List> {
        match panel {
            Panel::Tables => Some(match self.tables_tab {
                TablesTab::Tables => &mut self.tables,
                TablesTab::Views => &mut self.views,
            }),
            Panel::Saved => Some(match self.saved_tab {
                SavedTab::Saved => &mut self.saved,
                SavedTab::History => &mut self.history,
            }),
            Panel::Pending => Some(&mut self.pending),
            Panel::Connection | Panel::Main => None,
        }
    }

    /// Panel 2's rows for its tab.
    pub fn table_rows(&self) -> Vec<Row> {
        super::panels::table_rows(
            &self.schema,
            self.tables_tab == TablesTab::Views,
            &self.folded_schemas,
        )
    }

    /// Panel 3's saved rows.
    pub fn saved_rows(&self) -> Vec<Row> {
        super::panels::saved_rows(&self.saved_items, &self.folded_folders)
    }

    /// The selected row of panel 2.
    pub fn selected_table_row(&self) -> Option<Row> {
        let list = self.list(Panel::Tables)?;
        self.table_rows().into_iter().nth(list.selected)
    }

    /// The selected row of panel 3's Saved tab.
    pub fn selected_saved_row(&self) -> Option<Row> {
        self.saved_rows().into_iter().nth(self.saved.selected)
    }

    /// Sets each list's length from what it holds, keeping the selection
    /// within it.
    pub fn refresh_lists(&mut self) {
        let tables = super::panels::table_rows(&self.schema, false, &self.folded_schemas).len();
        let views = super::panels::table_rows(&self.schema, true, &self.folded_schemas).len();
        let saved = self.saved_rows().len();
        let history = self.history_items.len();
        for (list, len) in [
            (&mut self.tables, tables),
            (&mut self.views, views),
            (&mut self.saved, saved),
            (&mut self.history, history),
        ] {
            list.len = len;
            list.selected = list.selected.min(len.saturating_sub(1));
        }
    }

    /// Something the state file keeps changed.
    pub fn remember(&mut self) {
        if self.remember_dirty.is_none() {
            self.remember_dirty = Some(self.now);
        }
    }

    /// A line in the command log, stamped by the runtime.
    pub fn log_effect(tag: Option<Tag>, text: impl Into<String>) -> Effect {
        Effect::Log(LogEntry {
            tag,
            text: text.into(),
            elapsed: None,
        })
    }

    /// Focuses `panel` (the prototype's `focus`).
    pub fn focus_panel(&mut self, panel: Panel) {
        self.focus(panel);
    }

    /// The prototype's `focus(p)`: the main view keeps showing the last
    /// list panel, and an edit or a filter being typed ends.
    fn focus(&mut self, panel: Panel) {
        if panel != self.focus {
            self.browse.editing = None;
            self.browse.finding = false;
            self.browse.form = None;
        }
        self.focus = panel;
        if !matches!(panel, Panel::Main | Panel::Connection) {
            self.ctx = panel;
            self.query.shown = false;
        }
    }

    /// Moves `panel`'s list selection by `delta`, as `j`/`k` do there (the
    /// wheel, which needn't move the focus).
    pub(crate) fn step_list(&mut self, panel: Panel, delta: isize) {
        if let Some(list) = self.list_mut(panel) {
            let before = list.selected;
            list.step(delta);
            if panel == Panel::Tables && list.selected != before {
                self.main_tab = 0;
            }
        }
    }

    /// Scrolls the open help by a line.
    pub(crate) fn scroll_help(&mut self, down: bool) {
        let max = self.help_max_scroll();
        if let Some(Modal::Help { scroll }) = &mut self.modal {
            *scroll = if down {
                (*scroll + 1).min(max)
            } else {
                scroll.saturating_sub(1)
            };
        }
    }

    /// The prototype's `cycleTab`.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        if self.focus == Panel::Main && self.query.shown {
            query::cycle(self, forward);
            return;
        }
        let target = if self.focus == Panel::Main {
            if self.main_shows_a_table() {
                let n = self.main_tabs().len();
                self.main_tab = (self.main_tab + if forward { 1 } else { n - 1 }) % n;
                return;
            }
            self.ctx
        } else {
            self.focus
        };
        match target {
            Panel::Tables => {
                self.tables_tab = match self.tables_tab {
                    TablesTab::Tables => TablesTab::Views,
                    TablesTab::Views => TablesTab::Tables,
                };
                self.main_tab = 0;
                if let Some(list) = self.list_mut(Panel::Tables) {
                    list.selected = 0;
                }
                self.remembered.tables_tab =
                    (self.tables_tab == TablesTab::Views).then(|| "views".to_string());
                self.remember();
            }
            Panel::Saved => {
                self.saved_tab = match self.saved_tab {
                    SavedTab::Saved => SavedTab::History,
                    SavedTab::History => SavedTab::Saved,
                };
                if let Some(list) = self.list_mut(Panel::Saved) {
                    list.selected = 0;
                }
                self.remembered.saved_tab =
                    (self.saved_tab == SavedTab::History).then(|| "history".to_string());
                self.remember();
            }
            // Panel 4 (or the main view over it): Diff and SQL.
            Panel::Pending => self.pending_tab = (self.pending_tab + 1) % 2,
            Panel::Connection | Panel::Main => {}
        }
    }

    /// Asks before quitting when something would be lost.
    fn quit(&mut self) -> Vec<Effect> {
        if self.staged.total() > 0 || self.running() || self.committing.is_some() {
            self.modal = Some(Modal::ConfirmQuit);
            Vec::new()
        } else {
            vec![Effect::Quit]
        }
    }

    /// How far the help can scroll at the current size.
    fn help_max_scroll(&self) -> usize {
        let area = layout::help_area(Rect::new(0, 0, self.size.0, self.size.1));
        let visible = usize::from(area.height.saturating_sub(2));
        keymap::help_line_count().saturating_sub(visible)
    }
}

/// When a Core call answered, for its command-log line: the wall-clock
/// time (`HH:MM:SS`, the runtime's clock) and how long it took.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Stamp {
    pub time: String,
    pub elapsed_ms: u64,
}

/// A stored change another writer made (the runtime drops the TUI's own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Changed {
    /// Connections, projects or labels.
    Library,
    SavedQueries,
    History,
    /// Another process wrote the file (Decision 6): no kind or ids.
    External,
}

/// What reaches `update`.
#[derive(Debug, Clone)]
pub enum Msg {
    Key(KeyEvent),
    Mouse(MouseEvent),
    /// Bracketed paste: the text as the terminal sent it.
    Paste(String),
    Resize(u16, u16),
    Tick(Instant),
    /// The projects and connections.
    Library(Result<Library, CallError>),
    /// A project's saved queries.
    Saved {
        project_id: String,
        result: Result<Vec<SavedItem>, CallError>,
    },
    /// A connection's history, newest first.
    History {
        connection_id: String,
        result: Result<Vec<HistoryItem>, CallError>,
    },
    /// The connected database's tables.
    Schema {
        core_id: String,
        result: Result<Vec<TableItem>, CallError>,
        stamp: Stamp,
    },
    /// A connect answered.
    Connected {
        attempt: u64,
        result: Result<String, CallError>,
        stamp: Stamp,
    },
    /// "Save password" answered.
    PasswordSaved {
        connection_id: String,
        kinds: Vec<SecretKind>,
        result: Result<(), CallError>,
    },
    /// A keychain call started or the last one ended (`SecretWait`).
    Keychain {
        pending: bool,
        at: Instant,
    },
    Changed(Changed),
    /// Core closed one of the workspace's connections.
    Closed {
        core_id: String,
        code: String,
    },
    /// A command-log line the runtime stamped.
    Log(LogLine),
    /// A table page answered (`op` tells a late one from the current).
    Page {
        op: u64,
        result: Result<Page, CallError>,
        stamp: Stamp,
    },
    /// A table's columns, read for completion (probe F1).
    Columns {
        core_id: String,
        target: TableTarget,
        result: Result<Vec<(String, String)>, CallError>,
    },
    /// A table's metadata (and DDL) answered.
    Meta {
        core_id: String,
        target: TableTarget,
        result: Result<TableMeta, CallError>,
    },
    /// Core planned (or refused) a staged entry as of `seq`.
    Planned {
        id: String,
        seq: u64,
        result: Result<PlannedChange, CallError>,
    },
    /// Core applied (or refused) a commit.
    Applied {
        op: u64,
        result: Result<Applied, CallError>,
        stamp: Stamp,
    },
    /// One event of a query tab's run or page (`op` tells a late one).
    Run {
        tab: u64,
        op: u64,
        event: RunMsg,
    },
    /// Core explained a statement.
    Explained {
        tab: u64,
        op: u64,
        result: Result<Box<seaquel_types::ExplainResult>, CallError>,
    },
    /// A tab's text was saved (or refused; `taken_by` for `NAME_TAKEN`).
    QuerySaved {
        tab: u64,
        /// The text that was sent.
        text: query::SqlText,
        result: Result<SavedItem, CallError>,
        taken_by: Option<String>,
        /// Ask AI's SQL, not the tab's text: the tab stays as it is.
        detached: bool,
    },
    /// `$EDITOR` closed: the new text, or why it couldn't run.
    Edited {
        tab: u64,
        result: Result<query::SqlText, String>,
    },
    /// Ask AI's `ai_generate` answered (`op` tells a late one).
    Generated {
        op: u64,
        result: Result<query::SqlText, CallError>,
        elapsed_ms: u64,
    },
    /// A project's dashboards' names, for Ask AI's `@`.
    Mentions {
        project_id: String,
        result: Result<ask::Names, CallError>,
    },
    /// The DuckDB helper's download size looked up (`op` tells a late one).
    DuckdbOffer {
        op: u64,
        result: Result<install::Offer, CallError>,
    },
    /// Compressed bytes of the helper's download received so far.
    InstallProgress {
        op: u64,
        bytes: u64,
        total: u64,
    },
    /// The helper's download ended.
    Installed {
        op: u64,
        result: Result<(), CallError>,
    },
}

/// A command-log line `update` asks for; the runtime stamps the time. Its
/// text can name a connection, so `Debug` doesn't show it.
#[derive(Clone, PartialEq, Eq)]
pub struct LogEntry {
    pub tag: Option<Tag>,
    pub text: String,
    pub elapsed: Option<String>,
}

impl fmt::Debug for LogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LogEntry({:?})", self.tag)
    }
}

/// A connect for the runtime to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectCall {
    pub attempt: u64,
    pub connection_id: String,
    /// Supplied to Core; they win over the store.
    pub secrets: Typed,
    /// The host-key fingerprint the user trusted.
    pub trust: Option<String>,
}

/// A "Save password" for the runtime: `connectionUpdate` with the save
/// flags of `kinds` and their secrets (Decision 23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveCall {
    pub connection_id: String,
    pub kinds: Vec<SecretKind>,
    pub secrets: Typed,
}

/// What `update` asks the runtime to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Quit,
    /// Ctrl+Z: give the terminal back and stop (Unix).
    Suspend,
    /// Cancel a query tab's run (or page) and its explain: drop the task and
    /// cancel its stream in Core.
    CancelRun {
        tab: u64,
        stream_id: Option<String>,
    },
    /// The model changed in a way only a tick knows about.
    Redraw,
    LoadLibrary,
    LoadSaved {
        project_id: String,
    },
    LoadHistory {
        connection_id: String,
    },
    LoadSchema {
        core_id: String,
    },
    Connect(ConnectCall),
    /// Drop a connect in flight (the keychain wait given up).
    CancelConnect {
        attempt: u64,
    },
    Disconnect {
        core_id: String,
    },
    SavePassword(SaveCall),
    /// Write the state file.
    SaveState(Remembered),
    Log(LogEntry),
    /// Read a page of a table (one at a time: a new one drops the last).
    LoadPage(PageCall),
    /// Read a table's columns (`table_metadata`) for completion.
    LoadColumns {
        core_id: String,
        target: TableTarget,
    },
    /// Read a table's metadata and DDL.
    LoadMeta {
        core_id: String,
        target: TableTarget,
    },
    /// Plan a staged entry (`plan_edits`).
    PlanEdit(PlanCall),
    /// Commit the staged changes (`apply_changes`).
    Apply(ApplyCall),
    /// Run a query tab's text (`db.run`).
    Run(RunCall),
    /// Another page of a statement (`db.page`).
    PageRun(PageRunCall),
    /// Explain a statement (a new one replaces the tab's last).
    Explain(ExplainCall),
    /// Drop a tab's explain in flight.
    CancelExplain {
        tab: u64,
    },
    /// Save a tab's text as a saved query.
    SaveQuery(SaveQueryCall),
    /// Hand the text to `$EDITOR` (the loop gives it the terminal).
    ExternalEditor {
        tab: u64,
        text: query::SqlText,
    },
    /// Ask AI: `ai_generate` (a new one replaces the last).
    Generate(GenerateCall),
    /// Drop Ask AI's request in flight.
    CancelGenerate,
    /// A project's dashboards, for `@`.
    LoadMentions {
        project_id: String,
    },
    /// Look up the DuckDB helper's download (`duckdb_helper_status` and
    /// `duckdb_helper_asset`).
    CheckDuckdb {
        op: u64,
    },
    /// Download and install the DuckDB helper (`duckdb_helper_install`).
    InstallDuckdb {
        op: u64,
    },
    /// Drop the lookup or download `op` (the future goes, and with it the
    /// partial file).
    CancelInstall {
        op: u64,
    },
}

/// Applies `msg` to `model`.
pub fn update(model: &mut Model, msg: Msg) -> Vec<Effect> {
    if let Msg::Key(event) = &msg {
        if let Some((esc, then)) = esc_then_key(model, event) {
            let mut effects = update(model, Msg::Key(esc));
            effects.extend(update(model, Msg::Key(then)));
            return effects;
        }
    }
    // Read before the message is handled: its handler may end the op.
    let lost = connect::lost_by(model, &msg);
    let mut effects = apply(model, msg);
    if let Some(error) = lost {
        effects.extend(connect::lost(model, error));
    }
    commit::sync(model);
    browse::refresh(model);
    query::sync(model);
    effects
}

/// Probe F2: Esc and the next key in one read. A legacy terminal sends
/// Esc as a bare `ESC`, so `ESC :` (Esc typed quickly, then `:`; or merged
/// by tmux within its `escape-time`, or by SSH) reads as Alt+`:`. When the
/// context has no binding for that Alt character, it's Esc followed by the
/// character, as vim reads it in a terminal; an Alt binding that exists
/// (Alt+R, Alt+X) keeps its meaning. Only characters are split (review
/// M1): Alt+Left, Alt+Backspace or Alt+Enter come as one sequence, not as
/// Esc and a key. And with the kitty flags pushed Esc is `CSI 27 u`, never
/// merged, so nothing is split. Ctrl+Alt (AltGr) is left alone.
fn esc_then_key(model: &Model, event: &KeyEvent) -> Option<(KeyEvent, KeyEvent)> {
    use crossterm::event::KeyModifiers;
    if model.kitty_keys {
        return None;
    }
    let key = Key::from_event(event)?;
    if !key.alt || key.ctrl || !matches!(key.code, KeyCode::Char(_)) {
        return None;
    }
    if keymap::lookup(model.bar_context().chain(), key).is_some() {
        return None;
    }
    let esc = KeyEvent {
        code: KeyCode::Esc,
        modifiers: KeyModifiers::NONE,
        kind: event.kind,
        state: event.state,
    };
    let then = KeyEvent {
        modifiers: event.modifiers - KeyModifiers::ALT,
        ..*event
    };
    Some((esc, then))
}

fn apply(model: &mut Model, msg: Msg) -> Vec<Effect> {
    match msg {
        Msg::Key(event) => match Key::from_event(&event) {
            Some(key) => on_key(model, key),
            None => Vec::new(),
        },
        Msg::Mouse(event) => super::mouse::on_mouse(model, event),
        Msg::Paste(text) if ask::typing(model) => {
            ask::paste(model, &text);
            Vec::new()
        }
        Msg::Paste(text) => query::paste(model, &text),
        Msg::Resize(width, height) => {
            model.size = (width, height);
            Vec::new()
        }
        Msg::Tick(now) => {
            model.now = Some(now);
            on_tick(model, now)
        }
        Msg::Log(line) => {
            model.log.push(line);
            Vec::new()
        }
        Msg::Page { op, result, stamp } => browse::on_page(model, op, result, stamp),
        Msg::Meta {
            core_id,
            target,
            result,
        } => browse::on_meta(model, &core_id, &target, result),
        Msg::Columns {
            core_id,
            target,
            result,
        } => query::on_columns(model, &core_id, &target, result),
        Msg::Planned { id, seq, result } => browse::on_planned(model, &id, seq, result),
        Msg::Applied { op, result, stamp } => commit::on_applied(model, op, result, stamp),
        Msg::Run { tab, op, event } => query::on_run(model, tab, op, event),
        Msg::Explained { tab, op, result } => query::on_explained(model, tab, op, result),
        Msg::QuerySaved {
            tab,
            text,
            result,
            taken_by,
            detached,
        } => query::on_saved(model, tab, text, result, taken_by, detached),
        Msg::Edited { tab, result } => query::on_edited(model, tab, result),
        Msg::Generated {
            op,
            result,
            elapsed_ms,
        } => ask::on_generated(model, op, result, elapsed_ms),
        Msg::Mentions { project_id, result } => {
            ask::on_mentions(model, &project_id, result);
            Vec::new()
        }
        Msg::DuckdbOffer { op, result } => install::on_offer(model, op, result),
        Msg::InstallProgress { op, bytes, total } => {
            install::on_progress(model, op, bytes, total);
            Vec::new()
        }
        Msg::Installed { op, result } => install::on_installed(model, op, result),
        other => connect::on_msg(model, other),
    }
}

/// The state file's delay, and the keychain wait's limit.
fn on_tick(model: &mut Model, now: Instant) -> Vec<Effect> {
    let mut effects = Vec::new();
    if let Some(since) = model.remember_dirty {
        match since {
            Some(since) if now.saturating_duration_since(since) < STATE_FILE_DELAY => {}
            Some(_) => {
                model.remember_dirty = None;
                query::remember(model);
                effects.push(Effect::SaveState(model.remembered.clone()));
            }
            // Changed before the first tick: the delay starts now.
            None => model.remember_dirty = Some(Some(now)),
        }
    }
    if let Some(since) = model.keychain.filter(|_| !model.keychain_stale) {
        if now.saturating_duration_since(since) >= KEYCHAIN_LIMIT {
            effects.extend(connect::give_up_keychain(model));
        }
    }
    query::on_tick(model);
    effects
}

fn on_key(model: &mut Model, key: Key) -> Vec<Effect> {
    // A prompt's text takes every printable key (Decision 9).
    if !model.keychain_box() {
        if let Some(Modal::Password(prompt)) = &mut model.modal {
            match key.code {
                // AltGr arrives as Ctrl+Alt (Windows, some terminals).
                KeyCode::Char(c) if key.ctrl == key.alt => {
                    prompt.input.push(c);
                    return Vec::new();
                }
                KeyCode::Backspace => {
                    prompt.input.pop();
                    return Vec::new();
                }
                _ => {}
            }
        }
        // A staged value being edited, or `prod` being typed.
        if commit::typing(model) {
            match key.code {
                KeyCode::Char(c) if key.ctrl == key.alt => {
                    commit::type_char(model, c);
                    return Vec::new();
                }
                KeyCode::Backspace => {
                    commit::backspace(model);
                    return Vec::new();
                }
                _ => {}
            }
        }
        // Ask AI's request (`@` goes to the keymap: it may open the list).
        if ask::typing(model) {
            match key.code {
                KeyCode::Char(c) if key.ctrl == key.alt && c != '@' => {
                    ask::type_char(model, c);
                    return Vec::new();
                }
                KeyCode::Backspace => {
                    ask::backspace(model);
                    return Vec::new();
                }
                _ => {}
            }
        }
        // The query's dialogs: parameter values, a name, `prod`.
        if query::dialog_typing(model) {
            match key.code {
                KeyCode::Char(c) if key.ctrl == key.alt => {
                    query::dialog_char(model, c);
                    return Vec::new();
                }
                KeyCode::Backspace => {
                    query::dialog_backspace(model);
                    return Vec::new();
                }
                _ => {}
            }
        }
        // The editor in Insert mode, or its `:` line: every printable key
        // is text (Decision 9), `c` and `q` included.
        if query::typing(model) {
            if let KeyCode::Char(c) = key.code {
                if key.ctrl == key.alt {
                    return query::type_char(model, c);
                }
            }
        }
        // A cell being edited, the `/` filter or the form's value.
        if model.modal.is_none() && model.focus == Panel::Main && browse::typing(model) {
            match key.code {
                KeyCode::Char(c) if key.ctrl == key.alt => {
                    browse::type_char(model, c);
                    return Vec::new();
                }
                KeyCode::Backspace => {
                    browse::backspace(model);
                    return Vec::new();
                }
                _ => {}
            }
        }
    }
    let Some(binding) = keymap::lookup(model.bar_context().chain(), key) else {
        // Enter, Backspace, the arrows, … in Insert mode (and Backspace on
        // the `:` line) edit the text.
        if !model.keychain_box() && query::typing(model) && !key.ctrl && !key.alt {
            query::edit_key(model, key.code);
        }
        return Vec::new();
    };
    let forward = key.code != KeyCode::Char('[');
    match binding.action {
        Action::Focus => {
            if let KeyCode::Char(c @ '1'..='4') = key.code {
                model.focus(Panel::ORDER[usize::from(c as u8 - b'1')]);
            }
        }
        Action::FocusMain => model.focus(Panel::Main),
        Action::NextPanel | Action::PrevPanel => {
            let n = Panel::ORDER.len();
            let i = Panel::ORDER
                .iter()
                .position(|p| *p == model.focus)
                .unwrap_or(0);
            let step = if binding.action == Action::NextPanel {
                1
            } else {
                n - 1
            };
            model.focus(Panel::ORDER[(i + step) % n]);
        }
        Action::PanelLeft | Action::PanelRight => {
            let i = Panel::ORDER
                .iter()
                .position(|p| *p == model.focus)
                .unwrap_or(0);
            let step = if binding.action == Action::PanelRight {
                1
            } else {
                3
            };
            model.focus(Panel::ORDER[(i + step) % 4]);
        }
        Action::CycleTab => model.cycle_tab(forward),
        Action::Help => model.modal = Some(Modal::Help { scroll: 0 }),
        Action::Quit => return model.quit(),
        Action::Interrupt => {
            if model.running() {
                return query::cancel(model);
            }
            model.modal = Some(Modal::ConfirmQuit);
        }
        Action::Suspend => return vec![Effect::Suspend],
        Action::Up | Action::Down if matches!(model.modal, Some(Modal::Picker(_))) => {
            connect::picker_step(model, binding.action == Action::Down);
        }
        Action::Up | Action::Down => {
            let delta = if binding.action == Action::Down {
                1
            } else {
                -1
            };
            // Another table starts on Data (the prototype's `mainTab: 0`).
            model.step_list(model.focus, delta);
        }
        Action::Open if matches!(model.modal, Some(Modal::Picker(_))) => {
            return connect::picker_choose(model);
        }
        Action::Open => return connect::open(model),
        Action::Back if matches!(model.modal, Some(Modal::Picker(_))) => {
            connect::picker_back(model);
        }
        Action::Back => model.focus(model.ctx),
        Action::Close | Action::Cancel => {
            // A name for Ask AI's SQL goes back to the answer.
            model.modal = match model.modal.take() {
                Some(Modal::SaveAs(query::SaveAs {
                    back: Some(back), ..
                })) => Some(Modal::Ask(*back)),
                _ => None,
            };
        }
        Action::Pick => return connect::open_picker(model),
        Action::Reload => return connect::reload(model),
        Action::Submit => return connect::submit_password(model),
        Action::ToggleSave => {
            if let Some(Modal::Password(prompt)) = &mut model.modal {
                prompt.save = prompt.can_save && !prompt.save;
            }
        }
        Action::Trust => return connect::trust(model),
        Action::Retry => connect::retry(model),
        Action::GiveUp if ask::waiting(model) => return ask::give_up_keychain(model),
        Action::GiveUp => return connect::give_up_keychain(model),
        Action::Reconnect => return connect::reconnect(model),
        Action::Download => return install::download(model),
        Action::InstallRetry => return install::retry(model),
        Action::StopInstall => return install::stop(model),
        Action::ScrollUp | Action::ScrollDown => {
            model.scroll_help(binding.action == Action::ScrollDown);
        }
        Action::Confirm => return vec![Effect::Quit],
        Action::Undo => return browse::undo(model),
        Action::Commit => return commit::open(model),
        Action::Unstage => return commit::unstage(model),
        Action::EditValue => return commit::start_value_edit(model),
        Action::ApplyValue => return commit::apply_value(model),
        Action::DiscardAll => return commit::ask_discard(model),
        Action::ConfirmDiscard => return commit::discard(model),
        Action::ConfirmRecommit => return commit::recommit(model),
        Action::Execute => return commit::execute(model),
        Action::PreviewSql => commit::toggle_preview(model),
        Action::KeepQueue => return commit::keep_queue(model),
        Action::DiscardQueue => return commit::discard_queue(model),
        Action::CellUp => browse::move_cell(model, 0, -1),
        Action::CellDown => browse::move_cell(model, 0, 1),
        Action::CellLeft => browse::move_cell(model, -1, 0),
        Action::CellRight => browse::move_cell(model, 1, 0),
        Action::FirstRow => browse::first_row(model),
        Action::LastRow => browse::last_row(model),
        Action::EditCell => return browse::start_edit(model),
        Action::StageDelete => return browse::toggle_delete(model),
        Action::InsertRow => return browse::insert_row(model),
        Action::SetDefault => return browse::set_default(model),
        Action::Find => browse::start_find(model),
        Action::FilterForm => browse::open_form(model),
        Action::Sort => return browse::cycle_sort(model),
        Action::NextPage => return browse::next_page(model),
        Action::PrevPage => return browse::prev_page(model),
        Action::ReloadPage => return browse::reload(model),
        Action::GridBack => return browse::back(model),
        Action::ApplyEdit => return browse::apply_edit(model),
        Action::CancelEdit => browse::cancel_edit(model),
        Action::ApplyFind => browse::apply_find(model),
        Action::ClearFind => browse::clear_find(model),
        Action::FormField => browse::form_field(model),
        Action::FormPrev => browse::form_step(model, false),
        Action::FormNext => browse::form_step(model, true),
        Action::ApplyForm => return browse::apply_form(model),
        Action::CancelForm => browse::cancel_form(model),
        Action::NewQuery => return query::new_tab(model),
        Action::ShowQuery => return query::show(model),
        Action::OpenInQuery => return query::open_selected(model),
        Action::RunAll => return query::run(model, RunKind::All, false),
        Action::RunCurrent => return query::run(model, RunKind::Current, false),
        Action::Explain => return query::explain(model, false),
        Action::Analyze => return query::explain(model, true),
        Action::SaveQuery => return query::save(model),
        Action::ExternalEditor => return query::external_editor(model),
        Action::TogglePane => query::toggle_pane(model),
        Action::NormalMode => query::normal_mode(model),
        Action::Ed(command) => query::normal(model, command),
        Action::StartCommand => query::start_command(model),
        Action::RunCommand => return query::run_command(model),
        Action::CancelCommand => query::cancel_command(model),
        Action::Complete => return query::complete(model, key.code == KeyCode::Tab),
        Action::CompleteUp => query::completion_step(model, false),
        Action::CompleteDown => query::completion_step(model, true),
        Action::AcceptCompletion => query::accept_completion(model),
        Action::CloseCompletion => query::close_completion(model),
        Action::ResultUp => query::results_move(model, 0, -1),
        Action::ResultDown => query::results_move(model, 0, 1),
        Action::ResultLeft => query::results_move(model, -1, 0),
        Action::ResultRight => query::results_move(model, 1, 0),
        Action::ResultFirst => query::results_edge(model, false),
        Action::ResultLast => query::results_edge(model, true),
        Action::ResultNextPage => return query::page(model, true),
        Action::ResultPrevPage => return query::page(model, false),
        Action::OpenCell => query::open_cell(model),
        Action::PrevStatement => query::statement_step(model, false),
        Action::NextStatement => query::statement_step(model, true),
        Action::ResultsBack => return query::results_back(model),
        Action::ParamsNext => query::params_field(model, true),
        Action::ParamsPrev => query::params_field(model, false),
        Action::ParamsSubmit => return query::params_submit(model),
        Action::ConfirmRun => return query::confirm_run(model),
        Action::SaveAsSubmit => return query::save_as_submit(model),
        Action::CellScrollUp => query::cell_scroll(model, false),
        Action::CellScrollDown => query::cell_scroll(model, true),
        Action::Ask => return ask::open(model),
        Action::AskSend => return ask::send(model),
        Action::AskMention => ask::mention(model),
        Action::MentionAccept => ask::mention_accept(model),
        Action::MentionUp => ask::mention_step(model, false),
        Action::MentionDown => ask::mention_step(model, true),
        Action::MentionClose => ask::mention_close(model),
        Action::AskStop => return ask::stop(model),
        Action::AskInsert => return ask::insert(model),
        Action::AskRun => return ask::insert_and_run(model),
        Action::AskRefine => ask::refine(model),
        Action::AskSave => return ask::save(model),
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::fixtures::model;
    use crate::testing::keys::{click, ctrl, key, press};

    fn keys(model: &mut Model, typed: &str) -> Vec<Effect> {
        typed.chars().flat_map(|c| update(model, key(c))).collect()
    }

    // Prototype `onKey`: `k >= '0' && k <= '4'` and `focus(p)`.
    #[test]
    fn numbers_focus_panels_and_the_main_view_keeps_its_context() {
        let mut m = model();
        keys(&mut m, "3");
        assert_eq!((m.focus, m.ctx), (Panel::Saved, Panel::Saved));
        keys(&mut m, "1");
        assert_eq!((m.focus, m.ctx), (Panel::Connection, Panel::Saved));
        keys(&mut m, "0");
        assert_eq!((m.focus, m.ctx), (Panel::Main, Panel::Saved));
        keys(&mut m, "4");
        assert_eq!((m.focus, m.ctx), (Panel::Pending, Panel::Pending));
        keys(&mut m, "2");
        assert_eq!((m.focus, m.ctx), (Panel::Tables, Panel::Tables));
    }

    // Prototype: `k === 'Tab'` cycles `P` both ways.
    #[test]
    fn tab_and_shift_tab_cycle_all_five() {
        let mut m = model();
        let mut seen = vec![m.focus];
        for _ in 0..5 {
            update(&mut m, press(KeyCode::Tab));
            seen.push(m.focus);
        }
        assert_eq!(
            seen,
            [
                Panel::Tables,
                Panel::Saved,
                Panel::Pending,
                Panel::Main,
                Panel::Connection,
                Panel::Tables
            ]
        );
        update(&mut m, press(KeyCode::BackTab));
        assert_eq!(m.focus, Panel::Connection);
        update(&mut m, press(KeyCode::BackTab));
        assert_eq!(m.focus, Panel::Main);
        assert_eq!(
            m.ctx,
            Panel::Tables,
            "the main view keeps the last list panel"
        );
    }

    // Prototype: `left || right` outside the main view: `P[(i + (right ? 1 : 3)) % 4]`.
    #[test]
    fn h_and_l_cycle_the_four_panels() {
        let mut m = model();
        keys(&mut m, "l");
        assert_eq!(m.focus, Panel::Saved);
        update(&mut m, press(KeyCode::Right));
        assert_eq!(m.focus, Panel::Pending);
        keys(&mut m, "l");
        assert_eq!(m.focus, Panel::Connection);
        keys(&mut m, "h");
        assert_eq!(m.focus, Panel::Pending);
        keys(&mut m, "0h");
        assert_eq!(m.focus, Panel::Main, "h does nothing in the main view yet");
    }

    // Prototype `cycleTab`.
    #[test]
    fn brackets_switch_the_tab_of_the_focused_panel_or_the_table() {
        let mut m = model();
        m.views.selected = 1;
        keys(&mut m, "]");
        assert_eq!(m.tables_tab, TablesTab::Views);
        assert_eq!(m.views.selected, 0, "switching resets the selection");
        keys(&mut m, "[");
        assert_eq!(m.tables_tab, TablesTab::Tables);

        // In the main view over a table: the table's five tabs, both ways.
        keys(&mut m, "0]]");
        assert_eq!(m.main_tab, 2);
        keys(&mut m, "[[[");
        assert_eq!(m.main_tab, 4);
        assert_eq!(m.main_tabs()[m.main_tab_index()], "DDL");

        // Over Views, the main view's brackets switch the view's two tabs
        // (Data, Columns).
        m.tables_tab = TablesTab::Views;
        m.main_tab = 0;
        keys(&mut m, "]");
        assert_eq!(m.main_tabs()[m.main_tab_index()], "Columns");
        keys(&mut m, "]");
        assert_eq!(m.main_tab_index(), 0);
        m.tables_tab = TablesTab::Tables;

        keys(&mut m, "3]");
        assert_eq!(m.saved_tab, SavedTab::History);
        assert_eq!(m.saved.selected, 0);
        keys(&mut m, "0[");
        assert_eq!(
            m.saved_tab,
            SavedTab::Saved,
            "the main view over Saved switches it"
        );

        let before = m.clone();
        keys(&mut m, "4]");
        keys(&mut m, "1]");
        assert_eq!(
            (m.tables_tab, m.saved_tab, m.main_tab),
            (before.tables_tab, before.saved_tab, before.main_tab)
        );
    }

    // Prototype: `s.modal === 'help'`.
    #[test]
    fn question_mark_opens_help_and_esc_question_q_enter_close_it() {
        for close in [
            press(KeyCode::Esc),
            key('?'),
            key('q'),
            press(KeyCode::Enter),
        ] {
            let mut m = model();
            keys(&mut m, "?");
            assert_eq!(m.modal, Some(Modal::Help { scroll: 0 }));
            // Nothing else gets through while it's open.
            assert!(keys(&mut m, "3[").is_empty());
            assert_eq!(m.focus, Panel::Tables);
            assert!(update(&mut m, close).is_empty());
            assert_eq!(m.modal, None);
        }
    }

    #[test]
    fn help_scrolls_within_its_lines() {
        let mut m = model();
        m.size = (80, 24);
        keys(&mut m, "?kk");
        assert_eq!(m.modal, Some(Modal::Help { scroll: 0 }));
        for _ in 0..200 {
            keys(&mut m, "j");
        }
        let Some(Modal::Help { scroll }) = m.modal else {
            panic!("help is open")
        };
        let visible = layout::help_area(Rect::new(0, 0, 80, 24)).height as usize - 2;
        assert_eq!(scroll, keymap::help_line_count() - visible);
    }

    #[test]
    fn q_quits_at_once_when_nothing_is_staged_or_running() {
        let mut m = model();
        assert_eq!(keys(&mut m, "q"), [Effect::Quit]);
    }

    #[test]
    fn q_asks_when_something_is_staged_or_running() {
        for (staged, running) in [(true, false), (false, true)] {
            // `staged` counts the queue (Task 5) and `running` the query tabs
            // (Task 6), so stage and run for real.
            let mut m = if staged {
                crate::testing::fixtures::staged(148, 42, false)
            } else {
                crate::testing::fixtures::running(148, 42)
            };
            assert_eq!(m.running(), running);
            update(&mut m, press(KeyCode::Esc));
            m.focus_panel(Panel::Tables);
            assert!(keys(&mut m, "q").is_empty());
            assert_eq!(m.modal, Some(Modal::ConfirmQuit));
            assert!(update(&mut m, press(KeyCode::Esc)).is_empty());
            assert_eq!(m.modal, None);
            keys(&mut m, "q");
            assert_eq!(keys(&mut m, "y"), [Effect::Quit]);
        }
    }

    #[test]
    fn ctrl_c_asks_to_quit_or_cancels_the_run() {
        let mut m = model();
        assert!(update(&mut m, ctrl('c')).is_empty());
        assert_eq!(m.modal, Some(Modal::ConfirmQuit));
        assert_eq!(update(&mut m, ctrl('c')), [Effect::Quit], "twice quits");

        let mut m = crate::testing::fixtures::running(148, 42);
        let effects = update(&mut m, ctrl('c'));
        assert!(
            matches!(
                effects.as_slice(),
                [
                    Effect::CancelRun {
                        stream_id: Some(_),
                        ..
                    },
                    Effect::Log(_)
                ]
            ),
            "{effects:?}"
        );
        assert_eq!(m.modal, None);
        assert!(!m.running());
    }

    #[test]
    fn ctrl_z_suspends() {
        let mut m = model();
        assert_eq!(update(&mut m, ctrl('z')), [Effect::Suspend]);
    }

    // Prototype: `f === 'main'` and `k === 'Escape'`; Enter in a panel.
    #[test]
    fn enter_opens_in_the_main_view_and_esc_goes_back() {
        let mut m = model();
        keys(&mut m, "3");
        update(&mut m, press(KeyCode::Enter));
        assert_eq!(m.focus, Panel::Main);
        update(&mut m, press(KeyCode::Esc));
        assert_eq!(m.focus, Panel::Saved);
    }

    // Prototype: `dy` in tables (`ti`, `mainTab: 0`) and saved (`si`).
    #[test]
    fn j_and_k_move_within_the_list() {
        let mut m = model();
        m.tables.len = 3;
        m.main_tab = 2;
        keys(&mut m, "jjjj");
        assert_eq!(m.tables.selected, 2);
        assert_eq!(m.main_tab, 0, "another table starts on Data");
        update(&mut m, press(KeyCode::Up));
        assert_eq!(m.tables.selected, 1);
        keys(&mut m, "kkk");
        assert_eq!(m.tables.selected, 0);

        m.tables_tab = TablesTab::Views;
        keys(&mut m, "j");
        assert_eq!(m.views.selected, 0, "an empty list stays put");

        keys(&mut m, "3]");
        m.history.len = 2;
        keys(&mut m, "j");
        assert_eq!((m.history.selected, m.saved.selected), (1, 0));
    }

    #[test]
    fn a_click_focuses_the_panel_under_it() {
        let mut m = model();
        let areas = layout::areas(Rect::new(0, 0, m.size.0, m.size.1), m.ctx).unwrap();
        update(&mut m, click(areas.pending.x + 2, areas.pending.y + 1));
        assert_eq!((m.focus, m.ctx), (Panel::Pending, Panel::Pending));
        update(&mut m, click(areas.main.x + 5, areas.main.y + 5));
        assert_eq!(m.focus, Panel::Main);
        update(&mut m, click(areas.keybar.x, areas.keybar.y));
        assert_eq!(m.focus, Panel::Main, "the key bar isn't a panel");
    }

    #[test]
    fn a_resize_and_a_tick_only_record() {
        let mut m = model();
        assert!(update(&mut m, Msg::Resize(100, 30)).is_empty());
        assert_eq!(m.size, (100, 30));
        let now = Instant::now();
        assert!(update(&mut m, Msg::Tick(now)).is_empty());
        assert_eq!(m.now, Some(now));
    }

    #[test]
    fn the_bar_follows_the_focus_and_the_dialog() {
        let mut m = model();
        assert_eq!(m.bar_context(), BarContext::Tables);
        keys(&mut m, "1");
        assert_eq!(m.bar_context(), BarContext::Connection);
        keys(&mut m, "4");
        assert_eq!(m.bar_context(), BarContext::Pending);
        keys(&mut m, "?");
        assert_eq!(m.bar_context(), BarContext::Help);
        m.modal = Some(Modal::ConfirmQuit);
        assert_eq!(m.bar_context(), BarContext::ConfirmQuit);
    }
}
