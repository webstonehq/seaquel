//! The one keymap table (Decision 9). Every binding has a context, its keys,
//! a help line and, where the key bar shows it, a bar label. `update` looks
//! keys up here, and the key bar ([`bar`]) and `?` help ([`help`]) are
//! generated from it, so neither can drift from what the keys do.
//!
//! The bar and help strings live here, beside their keys, rather than in
//! `text.rs`: the table is the one place to look for them.
//!
//! **Lookup** goes through a chain of contexts, most specific first
//! ([`BarContext::chain`]): a binding in a panel's own context shadows a
//! global one on the same key (Saved's `[ ]` is "Saved/History").
//!
//! The prototype (`seaquel-tui-browse-logic.js.txt`, `onKey` and the key
//! bar in `renderVals`) is the source; bindings for what later tasks build
//! (edit, stage, commit, run) arrive with them.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

/// Where a binding applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Context {
    /// Anywhere no dialog is open.
    Global,
    /// Panel 1.
    Connection,
    /// The list panels 2, 3 and 4.
    Panel,
    /// Panel 2 (Tables - Views), on top of [`Context::Panel`].
    Tables,
    /// Panel 3 (Saved - History), on top of [`Context::Panel`].
    Saved,
    /// Panel 4 (Pending Changes), on top of [`Context::Panel`].
    Pending,
    /// The main view (`0`).
    Main,
    /// The main view's data grid (Task 4), on top of [`Context::Main`].
    Grid,
    /// A cell being edited (its text takes every printable key).
    CellEdit,
    /// The `/` filter being typed.
    Find,
    /// The `F` filter form.
    FilterForm,
    /// The `?` help.
    Help,
    /// "Quit?"
    ConfirmQuit,
    /// The picker: a project, then a connection.
    Picker,
    /// A password prompt (its text takes every printable key).
    Password,
    /// An unknown SSH host key.
    Trust,
    /// A failed connect.
    Problem,
    /// A failed connect a typed password could fix, on top of
    /// [`Context::Problem`].
    ProblemRetry,
    /// A message to acknowledge.
    Notice,
    /// The keychain wait box.
    Keychain,
    /// A DuckDB helper that didn't start or stopped: connect again, on top
    /// of [`Context::Problem`].
    ProblemReconnect,
    /// DuckDB support isn't installed: looking up its download.
    InstallChecking,
    /// "Download now?"
    InstallAsk,
    /// The download's progress.
    InstallDownloading,
    /// The download or its lookup failed.
    InstallFailed,
    /// It failed in a way a retry can't fix (`NOT_SUPPORTED`).
    InstallFailedFinal,
    /// The commit dialog (Task 5).
    Commit,
    /// The commit dialog on a `prod` connection: printable keys are the
    /// confirm text, on top of [`Context::Commit`].
    CommitProd,
    /// "Discard N staged changes?"
    ConfirmDiscard,
    /// "Commit again?" after a commit that lost its connection.
    ConfirmRecommit,
    /// Changes are staged on another connection.
    QueueSwitch,
    /// A staged value being edited (its text takes every printable key).
    EditValue,
    /// The query view (Task 6), under the editor's and results' contexts.
    Query,
    /// The editor in Insert mode (printable keys are its text).
    Insert,
    /// The editor in Normal mode.
    Normal,
    /// The editor's `:` line (printable keys are its text).
    Command,
    /// The completion popup, over Insert mode.
    Completion,
    /// The results box.
    Results,
    /// The `{{param}}` form (printable keys are its values).
    Params,
    /// "Run it?" (on a `prod` connection, printable keys are the confirm
    /// text).
    RunConfirm,
    /// A saved query's name (printable keys are its text).
    SaveAs,
    /// A cell opened full size.
    Cell,
    /// Ask AI's request being typed (printable keys are its text).
    AskPrompt,
    /// The `@` list over the request, on top of [`Context::AskPrompt`].
    AskMention,
    /// A request out.
    AskWaiting,
    /// The generated SQL shown.
    AskAnswer,
    /// The generated SQL already inserted (Ctrl+R didn't run it).
    AskDone,
}

impl Context {
    /// The help's section title; `None` for the dialogs, which say their
    /// keys on their own bar.
    pub fn help_title(self) -> Option<&'static str> {
        match self {
            Context::Global => Some("Global"),
            Context::Connection => Some("Connection"),
            Context::Panel => Some("Panels"),
            Context::Tables => Some("Tables and views"),
            Context::Saved => Some("Saved and History"),
            Context::Pending => Some("Pending changes"),
            Context::Main => Some("Main view"),
            Context::Grid => Some("Data grid"),
            Context::CellEdit => Some("Editing a cell"),
            Context::Find | Context::FilterForm => Some("Filtering"),
            Context::Query => Some("Query"),
            Context::Insert => Some("Query editor: Insert mode"),
            Context::Normal => Some("Query editor: Normal mode"),
            Context::Completion => Some("Completion"),
            Context::Results => Some("Results"),
            Context::Help
            | Context::ConfirmQuit
            | Context::Picker
            | Context::Password
            | Context::Trust
            | Context::Problem
            | Context::ProblemRetry
            | Context::Notice
            | Context::Keychain
            | Context::ProblemReconnect
            | Context::InstallChecking
            | Context::InstallAsk
            | Context::InstallDownloading
            | Context::InstallFailed
            | Context::InstallFailedFinal
            | Context::Commit
            | Context::CommitProd
            | Context::ConfirmDiscard
            | Context::ConfirmRecommit
            | Context::QueueSwitch
            | Context::EditValue
            | Context::Command
            | Context::Params
            | Context::RunConfirm
            | Context::SaveAs
            | Context::Cell
            | Context::AskPrompt
            | Context::AskMention
            | Context::AskWaiting
            | Context::AskAnswer
            | Context::AskDone => None,
        }
    }
}

/// What a key does. Some actions read the key itself (`Focus` the digit,
/// `CycleTab` the bracket).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Focus,
    FocusMain,
    NextPanel,
    PrevPanel,
    CycleTab,
    Help,
    Quit,
    Interrupt,
    Suspend,
    Up,
    Down,
    PanelLeft,
    PanelRight,
    Open,
    Back,
    Close,
    ScrollUp,
    ScrollDown,
    Confirm,
    Cancel,
    /// Panel 1's Enter: the picker.
    Pick,
    /// `r`: read the panel's list again.
    Reload,
    /// A prompt's Enter.
    Submit,
    /// The prompt's "Save password" box.
    ToggleSave,
    /// Trust the host key and connect.
    Trust,
    /// Ask for the password and connect again.
    Retry,
    /// Stop waiting for the keychain.
    GiveUp,
    /// Connect again after the DuckDB helper didn't start or stopped.
    Reconnect,
    /// Download DuckDB support (the DuckDB helper plan, Task 7).
    Download,
    /// Look up or download DuckDB support again.
    InstallRetry,
    /// Leave the install dialog (a download in flight stops) for the
    /// picker.
    StopInstall,
    /// `u`: take back the last staging action.
    Undo,
    // The data grid (Task 4).
    CellUp,
    CellDown,
    CellLeft,
    CellRight,
    FirstRow,
    LastRow,
    EditCell,
    StageDelete,
    InsertRow,
    SetDefault,
    Find,
    FilterForm,
    Sort,
    NextPage,
    PrevPage,
    ReloadPage,
    /// Esc in the grid: clear the `/` filter, an empty insert, the server
    /// filter, else back to the panel.
    GridBack,
    ApplyEdit,
    CancelEdit,
    ApplyFind,
    ClearFind,
    FormField,
    FormPrev,
    FormNext,
    ApplyForm,
    CancelForm,
    // Pending Changes and commit (Task 5).
    /// `c`: the commit dialog.
    Commit,
    /// Panel 4's `space`.
    Unstage,
    /// Panel 4's `e`.
    EditValue,
    /// Enter in the value edit.
    ApplyValue,
    /// Panel 4's `D`.
    DiscardAll,
    /// `y` in the discard question.
    ConfirmDiscard,
    /// `y` in the "commit again?" question.
    ConfirmRecommit,
    /// Enter in the commit dialog.
    Execute,
    /// `p` in the commit dialog (Tab on a `prod` connection).
    PreviewSql,
    /// Keep the changes staged on another connection.
    KeepQueue,
    /// Discard them.
    DiscardQueue,
    // Query (Task 6).
    /// `+`: a new query tab.
    NewQuery,
    /// `Q`: the query editor.
    ShowQuery,
    /// A saved query or history row in a query tab.
    OpenInQuery,
    RunAll,
    /// The statement at the cursor.
    RunCurrent,
    Explain,
    Analyze,
    SaveQuery,
    ExternalEditor,
    /// Ctrl+W: the editor or the results.
    TogglePane,
    /// Esc in Insert mode.
    NormalMode,
    /// A Normal-mode command.
    Ed(super::editor::Normal),
    StartCommand,
    RunCommand,
    CancelCommand,
    Complete,
    CompleteUp,
    CompleteDown,
    AcceptCompletion,
    CloseCompletion,
    ResultUp,
    ResultDown,
    ResultLeft,
    ResultRight,
    ResultFirst,
    ResultLast,
    ResultNextPage,
    ResultPrevPage,
    OpenCell,
    PrevStatement,
    NextStatement,
    /// Esc in the results: cancel, else back to the editor.
    ResultsBack,
    ParamsNext,
    ParamsPrev,
    ParamsSubmit,
    ConfirmRun,
    SaveAsSubmit,
    CellScrollUp,
    CellScrollDown,
    // Ask AI (Task 7).
    /// Ctrl+K: the popup.
    Ask,
    /// Enter: send the request.
    AskSend,
    /// `@`: typed, and at a word's start the list opens.
    AskMention,
    MentionAccept,
    MentionUp,
    MentionDown,
    MentionClose,
    /// Esc while waiting: drop the request.
    AskStop,
    /// Enter on the answer.
    AskInsert,
    /// Ctrl+R on the answer: insert and run it if read-only.
    AskRun,
    /// Tab on the answer.
    AskRefine,
    /// Ctrl+S on the answer.
    AskSave,
}

/// A key as the keymap compares it: Shift is part of a character (`G`,
/// `?`) and of Shift+Tab (`BackTab`), so it's dropped there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Key {
    pub const fn code(code: KeyCode) -> Key {
        Key {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    pub const fn char(c: char) -> Key {
        Key::code(KeyCode::Char(c))
    }

    pub const fn ctrl(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    pub const fn alt(c: char) -> Key {
        Key {
            code: KeyCode::Char(c),
            ctrl: false,
            alt: true,
            shift: false,
        }
    }

    /// Ctrl and a key that isn't a character (Ctrl+Enter, where the kitty
    /// protocol reports it).
    pub const fn ctrl_code(code: KeyCode) -> Key {
        Key {
            code,
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    /// The key a press (or repeat) means; `None` for a release (the kitty
    /// protocol can report them).
    pub fn from_event(event: &KeyEvent) -> Option<Key> {
        if !matches!(event.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
            return None;
        }
        let shift_is_the_key = matches!(event.code, KeyCode::Char(_) | KeyCode::BackTab);
        Some(Key {
            code: event.code,
            ctrl: event.modifiers.contains(KeyModifiers::CONTROL),
            alt: event.modifiers.contains(KeyModifiers::ALT),
            shift: !shift_is_the_key && event.modifiers.contains(KeyModifiers::SHIFT),
        })
    }
}

/// One binding.
#[derive(Debug)]
pub struct Binding {
    pub context: Context,
    pub keys: &'static [Key],
    pub action: Action,
    /// The keys as the help shows them.
    pub help_keys: &'static str,
    /// The help line.
    pub help: &'static str,
    /// The key bar's label and key text, when a bar shows it.
    pub bar: Option<(&'static str, &'static str)>,
}

const fn b(
    context: Context,
    keys: &'static [Key],
    action: Action,
    help_keys: &'static str,
    help: &'static str,
    bar: Option<(&'static str, &'static str)>,
) -> Binding {
    Binding {
        context,
        keys,
        action,
        help_keys,
        help,
        bar,
    }
}

use super::editor::Normal;
use Action as A;
use Context as C;

const UP: &[Key] = &[Key::char('k'), Key::code(KeyCode::Up)];
const DOWN: &[Key] = &[Key::char('j'), Key::code(KeyCode::Down)];
const LEFT: &[Key] = &[Key::char('h'), Key::code(KeyCode::Left)];
const RIGHT: &[Key] = &[Key::char('l'), Key::code(KeyCode::Right)];
const ENTER: &[Key] = &[Key::code(KeyCode::Enter)];
const ESC: &[Key] = &[Key::code(KeyCode::Esc)];
const BRACKETS: &[Key] = &[Key::char('['), Key::char(']')];

/// The table. Order matters for the help: bindings are listed in it.
pub static BINDINGS: &[Binding] = &[
    // Global (the prototype's `onKey` before the per-panel branches).
    b(
        C::Global,
        &[
            Key::char('1'),
            Key::char('2'),
            Key::char('3'),
            Key::char('4'),
        ],
        A::Focus,
        "1 2 3 4",
        "focus a panel",
        Some(("Focus panel", "1-4")),
    ),
    b(
        C::Global,
        &[Key::char('0')],
        A::FocusMain,
        "0",
        "focus the main view",
        Some(("Main", "0")),
    ),
    b(
        C::Global,
        &[Key::code(KeyCode::Tab)],
        A::NextPanel,
        "tab",
        "next panel",
        Some(("Next panel", "tab")),
    ),
    b(
        C::Global,
        &[Key::code(KeyCode::BackTab)],
        A::PrevPanel,
        "shift+tab",
        "previous panel",
        None,
    ),
    b(
        C::Global,
        BRACKETS,
        A::CycleTab,
        "[  ]",
        "previous / next tab",
        Some(("Tabs", "[ ]")),
    ),
    b(
        C::Global,
        &[Key::char('?')],
        A::Help,
        "?",
        "this help",
        Some(("Keybindings", "?")),
    ),
    b(
        C::Global,
        &[Key::char('u')],
        A::Undo,
        "u",
        "undo the last staged change",
        Some(("Undo", "u")),
    ),
    b(
        C::Global,
        &[Key::char('c')],
        A::Commit,
        "c",
        "commit the staged changes",
        Some(("Commit", "c")),
    ),
    b(
        C::Global,
        &[Key::char('Q')],
        A::ShowQuery,
        "Q",
        "the query editor (a new tab if none)",
        Some(("Query", "Q")),
    ),
    b(
        C::Global,
        &[Key::char('+')],
        A::NewQuery,
        "+",
        "a new query tab",
        Some(("New query", "+")),
    ),
    b(
        C::Global,
        &[Key::char('q')],
        A::Quit,
        "q",
        "quit (asks if anything would be lost)",
        None,
    ),
    b(
        C::Global,
        &[Key::ctrl('c')],
        A::Interrupt,
        "ctrl+c",
        "cancel a running statement, or quit",
        None,
    ),
    b(
        C::Global,
        &[Key::ctrl('z')],
        A::Suspend,
        "ctrl+z",
        "suspend (fg resumes; Unix)",
        None,
    ),
    // The list panels (2, 3, 4).
    b(
        C::Panel,
        DOWN,
        A::Down,
        "j k  arrows",
        "select",
        Some(("Select", "j/k")),
    ),
    b(C::Panel, UP, A::Up, "", "", None),
    b(
        C::Panel,
        LEFT,
        A::PanelLeft,
        "h l  arrows",
        "previous / next panel",
        None,
    ),
    b(C::Panel, RIGHT, A::PanelRight, "", "", None),
    b(
        C::Panel,
        ENTER,
        A::Open,
        "enter",
        "open in the main view; fold a schema or folder",
        Some(("Open", "enter")),
    ),
    b(
        C::Tables,
        &[Key::char('r')],
        A::Reload,
        "r",
        "read the tables again",
        Some(("Reload", "r")),
    ),
    b(
        C::Saved,
        BRACKETS,
        A::CycleTab,
        "[  ]",
        "saved queries / history",
        Some(("Saved/History", "[ ]")),
    ),
    b(
        C::Saved,
        &[Key::char('r')],
        A::Reload,
        "r",
        "read saved queries and history again",
        Some(("Reload", "r")),
    ),
    b(
        C::Saved,
        &[Key::char('o')],
        A::OpenInQuery,
        "o",
        "open it in a query tab",
        Some(("Open in a tab", "o")),
    ),
    b(
        C::Pending,
        ENTER,
        A::Open,
        "enter",
        "view the diff in the main view",
        Some(("View diff", "enter")),
    ),
    b(
        C::Pending,
        &[Key::char(' '), Key::char('d')],
        A::Unstage,
        "space  d",
        "unstage the selected change",
        Some(("Unstage", "space")),
    ),
    b(
        C::Pending,
        &[Key::char('e')],
        A::EditValue,
        "e",
        "edit the staged value",
        Some(("Edit value", "e")),
    ),
    b(
        C::Pending,
        &[Key::char('D')],
        A::DiscardAll,
        "D",
        "discard every staged change (asks)",
        Some(("Discard all", "D")),
    ),
    // Panel 1.
    b(
        C::Connection,
        ENTER,
        A::Pick,
        "enter",
        "pick a project and a connection",
        Some(("Connect", "enter")),
    ),
    b(
        C::Connection,
        LEFT,
        A::PanelLeft,
        "h l  arrows",
        "previous / next panel",
        None,
    ),
    b(C::Connection, RIGHT, A::PanelRight, "", "", None),
    // The main view.
    b(
        C::Main,
        ESC,
        A::Back,
        "esc",
        "back to the panel",
        Some(("Back", "esc")),
    ),
    b(
        C::Main,
        ENTER,
        A::OpenInQuery,
        "enter",
        "open it in a query tab (over panel 3)",
        Some(("Open in a tab", "enter")),
    ),
    // The data grid (the prototype's `f === 'main'` branch and its bar).
    b(
        C::Grid,
        DOWN,
        A::CellDown,
        "h j k l  arrows",
        "move the cell cursor",
        Some(("Move", "h j k l")),
    ),
    b(C::Grid, UP, A::CellUp, "", "", None),
    b(C::Grid, LEFT, A::CellLeft, "", "", None),
    b(C::Grid, RIGHT, A::CellRight, "", "", None),
    b(
        C::Grid,
        &[Key::char('g')],
        A::FirstRow,
        "g  G",
        "first / last row of the page",
        None,
    ),
    b(C::Grid, &[Key::char('G')], A::LastRow, "", "", None),
    b(
        C::Grid,
        &[Key::char('e'), Key::code(KeyCode::Enter)],
        A::EditCell,
        "e  enter",
        "edit the cell (NULL sets NULL; \\NULL: text)",
        Some(("Edit", "e")),
    ),
    b(
        C::Grid,
        &[Key::char('d')],
        A::StageDelete,
        "d",
        "stage / unstage the row's delete",
        Some(("Stage delete", "d")),
    ),
    b(
        C::Grid,
        &[Key::char('a')],
        A::InsertRow,
        "a",
        "stage an insert (a blank row at the top)",
        Some(("Insert", "a")),
    ),
    b(
        C::Grid,
        &[Key::char('D')],
        A::SetDefault,
        "D",
        "stage Set default on the cell",
        None,
    ),
    b(
        C::Grid,
        &[Key::char('/')],
        A::Find,
        "/",
        "filter the loaded rows · esc clears",
        Some(("Filter", "/")),
    ),
    b(
        C::Grid,
        &[Key::char('F')],
        A::FilterForm,
        "F",
        "filter the table (column, operator, value)",
        None,
    ),
    b(
        C::Grid,
        &[Key::char('s')],
        A::Sort,
        "s",
        "sort by it: ascending, descending, off",
        None,
    ),
    b(
        C::Grid,
        &[Key::char('n')],
        A::NextPage,
        "n  p",
        "next / previous page",
        None,
    ),
    b(C::Grid, &[Key::char('p')], A::PrevPage, "", "", None),
    b(
        C::Grid,
        &[Key::char('r')],
        A::ReloadPage,
        "r",
        "read the page again",
        None,
    ),
    b(
        C::Grid,
        ESC,
        A::GridBack,
        "esc",
        "clear the filter, else back to the panel",
        Some(("Back", "esc")),
    ),
    // A cell being edited: printable keys are its text.
    b(
        C::CellEdit,
        ENTER,
        A::ApplyEdit,
        "enter",
        "stage the value",
        Some(("Save", "enter")),
    ),
    b(
        C::CellEdit,
        ESC,
        A::CancelEdit,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // The `/` filter being typed.
    b(
        C::Find,
        ENTER,
        A::ApplyFind,
        "enter",
        "keep the filter",
        Some(("Apply", "enter")),
    ),
    b(
        C::Find,
        ESC,
        A::ClearFind,
        "esc",
        "clear it",
        Some(("Clear", "esc")),
    ),
    // The `F` form: Tab moves between column, operator and value; the
    // arrows choose a column or an operator; the value takes printable keys.
    b(
        C::FilterForm,
        &[Key::code(KeyCode::Tab)],
        A::FormField,
        "tab",
        "next field",
        Some(("Next field", "tab")),
    ),
    b(
        C::FilterForm,
        &[Key::code(KeyCode::Up), Key::code(KeyCode::Left)],
        A::FormPrev,
        "arrows",
        "choose the column or operator",
        Some(("Choose", "arrows")),
    ),
    b(
        C::FilterForm,
        &[Key::code(KeyCode::Down), Key::code(KeyCode::Right)],
        A::FormNext,
        "",
        "",
        None,
    ),
    b(
        C::FilterForm,
        ENTER,
        A::ApplyForm,
        "enter",
        "filter",
        Some(("Apply", "enter")),
    ),
    b(
        C::FilterForm,
        ESC,
        A::CancelForm,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // The help.
    b(
        C::Help,
        &[
            Key::code(KeyCode::Esc),
            Key::char('?'),
            Key::char('q'),
            Key::code(KeyCode::Enter),
        ],
        A::Close,
        "esc",
        "close",
        Some(("Close", "esc")),
    ),
    b(
        C::Help,
        DOWN,
        A::ScrollDown,
        "j k",
        "scroll",
        Some(("Scroll", "j/k")),
    ),
    b(C::Help, UP, A::ScrollUp, "", "", None),
    // "Quit?"
    b(
        C::ConfirmQuit,
        &[Key::char('y'), Key::code(KeyCode::Enter), Key::ctrl('c')],
        A::Confirm,
        "y",
        "quit",
        Some(("Quit", "y")),
    ),
    b(
        C::ConfirmQuit,
        &[Key::char('n'), Key::code(KeyCode::Esc), Key::char('q')],
        A::Cancel,
        "esc",
        "stay",
        Some(("Stay", "esc")),
    ),
    // The picker.
    b(
        C::Picker,
        ENTER,
        A::Open,
        "enter",
        "choose",
        Some(("Choose", "enter")),
    ),
    b(
        C::Picker,
        DOWN,
        A::Down,
        "j k",
        "select",
        Some(("Select", "j/k")),
    ),
    b(C::Picker, UP, A::Up, "", "", None),
    b(
        C::Picker,
        ESC,
        A::Back,
        "esc",
        "back",
        Some(("Back", "esc")),
    ),
    // A password prompt: printable keys are its text.
    b(
        C::Password,
        ENTER,
        A::Submit,
        "enter",
        "connect",
        Some(("Connect", "enter")),
    ),
    b(
        C::Password,
        &[Key::code(KeyCode::Tab)],
        A::ToggleSave,
        "tab",
        "save the password",
        Some(("Save password", "tab")),
    ),
    b(
        C::Password,
        ESC,
        A::Cancel,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // An unknown SSH host key.
    b(
        C::Trust,
        &[Key::char('t')],
        A::Trust,
        "t",
        "trust and connect",
        Some(("Trust and connect", "t")),
    ),
    b(
        C::Trust,
        &[Key::code(KeyCode::Esc), Key::char('n')],
        A::Cancel,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // A failed connect.
    b(
        C::ProblemRetry,
        &[Key::char('r')],
        A::Retry,
        "r",
        "enter the password again and connect",
        Some(("Enter the password again", "r")),
    ),
    b(
        C::Problem,
        &[Key::code(KeyCode::Esc), Key::code(KeyCode::Enter)],
        A::Close,
        "esc",
        "close",
        Some(("Close", "esc")),
    ),
    b(
        C::Notice,
        &[Key::code(KeyCode::Enter), Key::code(KeyCode::Esc)],
        A::Close,
        "enter",
        "close",
        Some(("OK", "enter")),
    ),
    // The keychain wait box.
    b(
        C::Keychain,
        ESC,
        A::GiveUp,
        "esc",
        "stop waiting",
        Some(("Stop waiting", "esc")),
    ),
    // A DuckDB helper that didn't start or stopped.
    b(
        C::ProblemReconnect,
        &[Key::char('r')],
        A::Reconnect,
        "r",
        "connect again",
        Some(("Connect again", "r")),
    ),
    // DuckDB support isn't installed (the DuckDB helper plan, Task 7).
    b(
        C::InstallChecking,
        ESC,
        A::StopInstall,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    b(
        C::InstallAsk,
        ENTER,
        A::Download,
        "enter",
        "download",
        Some(("Download", "enter")),
    ),
    b(
        C::InstallAsk,
        ESC,
        A::StopInstall,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    b(
        C::InstallDownloading,
        ESC,
        A::StopInstall,
        "esc",
        "stop the download",
        Some(("Stop", "esc")),
    ),
    b(
        C::InstallFailed,
        &[Key::char('r')],
        A::InstallRetry,
        "r",
        "try again",
        Some(("Retry", "r")),
    ),
    b(
        C::InstallFailed,
        &[Key::code(KeyCode::Esc), Key::code(KeyCode::Enter)],
        A::StopInstall,
        "esc",
        "close",
        Some(("Close", "esc")),
    ),
    b(
        C::InstallFailedFinal,
        &[Key::code(KeyCode::Esc), Key::code(KeyCode::Enter)],
        A::StopInstall,
        "esc",
        "close",
        Some(("Close", "esc")),
    ),
    // The commit dialog (the prototype's `s.modal === 'commit'`).
    b(
        C::Commit,
        ENTER,
        A::Execute,
        "enter",
        "execute",
        Some(("Execute", "enter")),
    ),
    b(
        C::Commit,
        &[Key::char('p')],
        A::PreviewSql,
        "p",
        "preview the SQL",
        Some(("Preview SQL", "p")),
    ),
    b(
        C::Commit,
        ESC,
        A::Cancel,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    b(
        C::CommitProd,
        &[Key::code(KeyCode::Tab)],
        A::PreviewSql,
        "tab",
        "preview the SQL",
        Some(("Preview SQL", "tab")),
    ),
    // "Discard N staged changes?"
    b(
        C::ConfirmDiscard,
        &[Key::char('y'), Key::code(KeyCode::Enter)],
        A::ConfirmDiscard,
        "y",
        "discard them",
        Some(("Discard", "y")),
    ),
    b(
        C::ConfirmDiscard,
        &[Key::char('n'), Key::code(KeyCode::Esc)],
        A::Cancel,
        "esc",
        "keep them",
        Some(("Keep", "esc")),
    ),
    // "Commit again?": the last commit lost its connection and may have
    // been applied (the DuckDB helper plan's probe F1).
    b(
        C::ConfirmRecommit,
        &[Key::char('y')],
        A::ConfirmRecommit,
        "y",
        "commit them again",
        Some(("Commit again", "y")),
    ),
    b(
        C::ConfirmRecommit,
        &[Key::char('n'), Key::code(KeyCode::Esc)],
        A::Cancel,
        "esc",
        "don't commit",
        Some(("Cancel", "esc")),
    ),
    // Changes staged on another connection.
    b(
        C::QueueSwitch,
        &[Key::char('k')],
        A::KeepQueue,
        "k",
        "keep them staged",
        Some(("Keep them", "k")),
    ),
    b(
        C::QueueSwitch,
        &[Key::char('d')],
        A::DiscardQueue,
        "d",
        "discard them",
        Some(("Discard them", "d")),
    ),
    b(
        C::QueueSwitch,
        ESC,
        A::Cancel,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // A staged value being edited: printable keys are its text.
    b(
        C::EditValue,
        ENTER,
        A::ApplyValue,
        "enter",
        "stage the value",
        Some(("Save", "enter")),
    ),
    b(
        C::EditValue,
        ESC,
        A::Cancel,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // The query view (Task 6): keys that work in the editor and the results.
    b(
        C::Query,
        &[
            Key::ctrl('r'),
            Key::code(KeyCode::F(5)),
            Key::ctrl_code(KeyCode::Enter),
        ],
        A::RunAll,
        "ctrl+r  f5",
        "run the whole text (ctrl+enter too)",
        Some(("Run", "ctrl+r")),
    ),
    // Ctrl+E first (probe F8): macOS terminals type `®` for Option+R
    // unless Option is set to send Meta. Alt+R stays.
    b(
        C::Query,
        &[Key::ctrl('e'), Key::alt('r')],
        A::RunCurrent,
        "ctrl+e  alt+r",
        "run the statement at the cursor",
        Some(("Run statement", "ctrl+e")),
    ),
    b(
        C::Query,
        &[Key::ctrl('x')],
        A::Explain,
        "ctrl+x",
        "explain the statement at the cursor",
        Some(("Explain", "ctrl+x")),
    ),
    b(
        C::Query,
        &[Key::ctrl('k')],
        A::Ask,
        "ctrl+k",
        "ask AI for SQL (:ask too)",
        Some(("Ask AI", "ctrl+k")),
    ),
    b(
        C::Query,
        &[Key::alt('x')],
        A::Analyze,
        "alt+x",
        "EXPLAIN ANALYZE (:analyze too; asks unless a SELECT)",
        None,
    ),
    b(
        C::Query,
        &[Key::ctrl('s')],
        A::SaveQuery,
        "ctrl+s",
        "save as a saved query",
        Some(("Save", "ctrl+s")),
    ),
    b(
        C::Query,
        &[Key::ctrl('o')],
        A::ExternalEditor,
        "ctrl+o",
        "edit in $VISUAL or $EDITOR",
        Some(("Open in $EDITOR", "ctrl+o")),
    ),
    b(
        C::Query,
        &[Key::ctrl('w')],
        A::TogglePane,
        "ctrl+w",
        "the editor or the results",
        Some(("Results", "ctrl+w")),
    ),
    // Insert mode: printable keys are text.
    b(
        C::Insert,
        ESC,
        A::NormalMode,
        "esc",
        "Normal mode",
        Some(("Normal mode", "esc")),
    ),
    b(
        C::Insert,
        &[Key::code(KeyCode::Tab), Key::ctrl(' ')],
        A::Complete,
        "tab  ctrl+space",
        "complete a name (opens itself after .)",
        Some(("Complete", "tab")),
    ),
    // Normal mode (Q7 A).
    b(
        C::Normal,
        &[Key::char('i')],
        A::Ed(Normal::Insert),
        "i  a  A  I",
        "Insert mode: before, after, end, start",
        Some(("Insert", "i")),
    ),
    b(
        C::Normal,
        &[Key::char('a')],
        A::Ed(Normal::Append),
        "",
        "",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('A')],
        A::Ed(Normal::AppendEnd),
        "",
        "",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('I')],
        A::Ed(Normal::InsertStart),
        "",
        "",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('o')],
        A::Ed(Normal::OpenBelow),
        "o  O",
        "a new line below / above",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('O')],
        A::Ed(Normal::OpenAbove),
        "",
        "",
        None,
    ),
    b(
        C::Normal,
        LEFT,
        A::Ed(Normal::Left),
        "h j k l  arrows",
        "move",
        None,
    ),
    b(C::Normal, DOWN, A::Ed(Normal::Down), "", "", None),
    b(C::Normal, UP, A::Ed(Normal::Up), "", "", None),
    b(C::Normal, RIGHT, A::Ed(Normal::Right), "", "", None),
    b(
        C::Normal,
        &[Key::char('w')],
        A::Ed(Normal::WordForward),
        "w  b  e",
        "next word, previous word, end of word",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('b')],
        A::Ed(Normal::WordBack),
        "",
        "",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('e')],
        A::Ed(Normal::WordEnd),
        "",
        "",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('0')],
        A::Ed(Normal::LineStart),
        "0  $",
        "start / end of the line",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('$')],
        A::Ed(Normal::LineEnd),
        "",
        "",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('g')],
        A::Ed(Normal::G),
        "g  G",
        "gg the first line, G the last",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('G')],
        A::Ed(Normal::Bottom),
        "",
        "",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('d')],
        A::Ed(Normal::D),
        "d",
        "dd deletes the line",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('y')],
        A::Ed(Normal::Y),
        "y",
        "yy copies the line",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('p')],
        A::Ed(Normal::Paste),
        "p",
        "paste after the cursor (a line below)",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('x')],
        A::Ed(Normal::DeleteChar),
        "x",
        "delete the character",
        None,
    ),
    b(
        C::Normal,
        &[Key::char('u')],
        A::Ed(Normal::Undo),
        "u",
        "undo the last edit of the text",
        Some(("Undo", "u")),
    ),
    b(
        C::Normal,
        &[Key::char('R')],
        A::RunCurrent,
        "R",
        "run the statement at the cursor",
        Some(("Run statement", "R")),
    ),
    b(
        C::Normal,
        &[Key::char(':')],
        A::StartCommand,
        ":",
        ":w :explain :analyze :all :q (:q! forces)",
        Some(("Command", ":")),
    ),
    // The `:` line.
    b(
        C::Command,
        ENTER,
        A::RunCommand,
        "enter",
        "run the command",
        Some(("Run", "enter")),
    ),
    b(
        C::Command,
        ESC,
        A::CancelCommand,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // The completion popup.
    b(
        C::Completion,
        &[Key::code(KeyCode::Down), Key::ctrl('n')],
        A::CompleteDown,
        "arrows",
        "choose (ctrl+n and ctrl+p too)",
        Some(("Choose", "arrows")),
    ),
    b(
        C::Completion,
        &[Key::code(KeyCode::Up), Key::ctrl('p')],
        A::CompleteUp,
        "",
        "",
        None,
    ),
    b(
        C::Completion,
        &[Key::code(KeyCode::Tab), Key::code(KeyCode::Enter)],
        A::AcceptCompletion,
        "tab  enter",
        "insert it",
        Some(("Insert", "tab")),
    ),
    b(
        C::Completion,
        ESC,
        A::CloseCompletion,
        "esc",
        "close it",
        Some(("Close", "esc")),
    ),
    // The results.
    b(
        C::Results,
        DOWN,
        A::ResultDown,
        "h j k l  arrows",
        "move the cell cursor",
        Some(("Move", "h j k l")),
    ),
    b(C::Results, UP, A::ResultUp, "", "", None),
    b(C::Results, LEFT, A::ResultLeft, "", "", None),
    b(C::Results, RIGHT, A::ResultRight, "", "", None),
    b(
        C::Results,
        &[Key::char('g')],
        A::ResultFirst,
        "g  G",
        "first / last row",
        None,
    ),
    b(C::Results, &[Key::char('G')], A::ResultLast, "", "", None),
    b(
        C::Results,
        &[Key::char('n')],
        A::ResultNextPage,
        "n  p",
        "next / previous page",
        Some(("Page", "n p")),
    ),
    b(
        C::Results,
        &[Key::char('p')],
        A::ResultPrevPage,
        "",
        "",
        None,
    ),
    b(
        C::Results,
        &[Key::char('(')],
        A::PrevStatement,
        "(  )",
        "previous / next statement with rows",
        None,
    ),
    b(
        C::Results,
        &[Key::char(')')],
        A::NextStatement,
        "",
        "",
        None,
    ),
    b(
        C::Results,
        ENTER,
        A::OpenCell,
        "enter",
        "the cell full size",
        Some(("Open cell", "enter")),
    ),
    b(
        C::Results,
        ESC,
        A::ResultsBack,
        "esc",
        "cancel the run, else back to the editor",
        Some(("Back", "esc")),
    ),
    // The `{{param}}` form: printable keys are the values.
    b(
        C::Params,
        &[Key::code(KeyCode::Tab), Key::code(KeyCode::Down)],
        A::ParamsNext,
        "tab",
        "next value",
        Some(("Next", "tab")),
    ),
    b(
        C::Params,
        &[Key::code(KeyCode::BackTab), Key::code(KeyCode::Up)],
        A::ParamsPrev,
        "shift+tab",
        "previous value",
        None,
    ),
    b(
        C::Params,
        ENTER,
        A::ParamsSubmit,
        "enter",
        "run",
        Some(("Run", "enter")),
    ),
    b(
        C::Params,
        ESC,
        A::Cancel,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // "Run it?"
    b(
        C::RunConfirm,
        ENTER,
        A::ConfirmRun,
        "enter",
        "run it",
        Some(("Run", "enter")),
    ),
    b(
        C::RunConfirm,
        ESC,
        A::Cancel,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // A saved query's name.
    b(
        C::SaveAs,
        ENTER,
        A::SaveAsSubmit,
        "enter",
        "save",
        Some(("Save", "enter")),
    ),
    b(
        C::SaveAs,
        ESC,
        A::Cancel,
        "esc",
        "cancel",
        Some(("Cancel", "esc")),
    ),
    // A cell full size.
    b(
        C::Cell,
        DOWN,
        A::CellScrollDown,
        "j k",
        "scroll",
        Some(("Scroll", "j/k")),
    ),
    b(C::Cell, UP, A::CellScrollUp, "", "", None),
    b(
        C::Cell,
        &[
            Key::code(KeyCode::Esc),
            Key::code(KeyCode::Enter),
            Key::char('q'),
        ],
        A::Close,
        "esc",
        "close",
        Some(("Close", "esc")),
    ),
    // Ask AI (Task 7, design 1d): printable keys are the request.
    b(
        C::AskPrompt,
        ENTER,
        A::AskSend,
        "enter",
        "send",
        Some(("Ask", "enter")),
    ),
    b(
        C::AskPrompt,
        &[Key::char('@')],
        A::AskMention,
        "@",
        "name a table, saved query or dashboard",
        Some(("Mention", "@")),
    ),
    b(
        C::AskPrompt,
        ESC,
        A::Close,
        "esc",
        "close",
        Some(("Close", "esc")),
    ),
    b(
        C::AskMention,
        &[Key::code(KeyCode::Tab), Key::code(KeyCode::Enter)],
        A::MentionAccept,
        "tab",
        "insert the name",
        Some(("Insert", "tab")),
    ),
    b(
        C::AskMention,
        &[Key::code(KeyCode::Down), Key::ctrl('n')],
        A::MentionDown,
        "arrows",
        "choose",
        Some(("Choose", "arrows")),
    ),
    b(
        C::AskMention,
        &[Key::code(KeyCode::Up), Key::ctrl('p')],
        A::MentionUp,
        "",
        "",
        None,
    ),
    b(
        C::AskMention,
        ESC,
        A::MentionClose,
        "esc",
        "close the list",
        Some(("Close", "esc")),
    ),
    b(
        C::AskWaiting,
        ESC,
        A::AskStop,
        "esc",
        "stop waiting and drop the request",
        Some(("Stop", "esc")),
    ),
    b(
        C::AskAnswer,
        ENTER,
        A::AskInsert,
        "enter",
        "insert at the cursor",
        Some(("Insert", "enter")),
    ),
    b(
        C::AskAnswer,
        &[Key::ctrl('r')],
        A::AskRun,
        "ctrl+r",
        "insert and run it, if it only reads",
        Some(("Run", "ctrl+r")),
    ),
    b(
        C::AskAnswer,
        &[Key::code(KeyCode::Tab)],
        A::AskRefine,
        "tab",
        "refine it",
        Some(("Refine", "tab")),
    ),
    b(
        C::AskAnswer,
        &[Key::ctrl('s')],
        A::AskSave,
        "ctrl+s",
        "save as a saved query",
        Some(("Save as", "ctrl+s")),
    ),
    b(
        C::AskAnswer,
        ESC,
        A::Close,
        "esc",
        "close",
        Some(("Close", "esc")),
    ),
    b(
        C::AskDone,
        &[Key::code(KeyCode::Tab)],
        A::AskRefine,
        "tab",
        "refine it",
        Some(("Refine", "tab")),
    ),
    b(
        C::AskDone,
        &[Key::ctrl('s')],
        A::AskSave,
        "ctrl+s",
        "save as a saved query",
        Some(("Save as", "ctrl+s")),
    ),
    b(
        C::AskDone,
        &[Key::code(KeyCode::Esc), Key::code(KeyCode::Enter)],
        A::Close,
        "esc",
        "close",
        Some(("Close", "esc")),
    ),
];

/// What the key bar is showing for: the focused panel, or a dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarContext {
    Connection,
    Tables,
    Saved,
    Pending,
    Main,
    Grid,
    CellEdit,
    Find,
    FilterForm,
    Help,
    ConfirmQuit,
    Picker,
    Password,
    /// The password prompt while the secret store isn't available: no
    /// "Save password" (probe F4).
    PasswordNoSave,
    Trust,
    Problem,
    ProblemRetry,
    Notice,
    Keychain,
    ProblemReconnect,
    InstallChecking,
    InstallAsk,
    InstallDownloading,
    InstallFailed,
    InstallFailedFinal,
    Commit,
    CommitProd,
    ConfirmDiscard,
    ConfirmRecommit,
    QueueSwitch,
    EditValue,
    QueryInsert,
    QueryNormal,
    QueryCommand,
    Completion,
    Results,
    Params,
    RunConfirm,
    SaveAs,
    Cell,
    AskPrompt,
    AskMention,
    AskWaiting,
    AskAnswer,
    AskDone,
}

impl BarContext {
    pub const ALL: [BarContext; 45] = [
        BarContext::Connection,
        BarContext::Tables,
        BarContext::Saved,
        BarContext::Pending,
        BarContext::Main,
        BarContext::Grid,
        BarContext::CellEdit,
        BarContext::Find,
        BarContext::FilterForm,
        BarContext::Help,
        BarContext::ConfirmQuit,
        BarContext::Picker,
        BarContext::Password,
        BarContext::PasswordNoSave,
        BarContext::Trust,
        BarContext::Problem,
        BarContext::ProblemRetry,
        BarContext::Notice,
        BarContext::Keychain,
        BarContext::ProblemReconnect,
        BarContext::InstallChecking,
        BarContext::InstallAsk,
        BarContext::InstallDownloading,
        BarContext::InstallFailed,
        BarContext::InstallFailedFinal,
        BarContext::Commit,
        BarContext::CommitProd,
        BarContext::ConfirmDiscard,
        BarContext::ConfirmRecommit,
        BarContext::QueueSwitch,
        BarContext::EditValue,
        BarContext::QueryInsert,
        BarContext::QueryNormal,
        BarContext::QueryCommand,
        BarContext::Completion,
        BarContext::Results,
        BarContext::Params,
        BarContext::RunConfirm,
        BarContext::SaveAs,
        BarContext::Cell,
        BarContext::AskPrompt,
        BarContext::AskMention,
        BarContext::AskWaiting,
        BarContext::AskAnswer,
        BarContext::AskDone,
    ];

    /// The contexts a key is looked up in, most specific first. Dialogs
    /// take every key: nothing global fires while one is open.
    pub fn chain(self) -> &'static [Context] {
        match self {
            BarContext::Connection => &[C::Connection, C::Global],
            BarContext::Tables => &[C::Tables, C::Panel, C::Global],
            BarContext::Saved => &[C::Saved, C::Panel, C::Global],
            BarContext::Pending => &[C::Pending, C::Panel, C::Global],
            BarContext::Main => &[C::Main, C::Global],
            BarContext::Grid => &[C::Grid, C::Main, C::Global],
            BarContext::CellEdit => &[C::CellEdit],
            BarContext::Find => &[C::Find],
            BarContext::FilterForm => &[C::FilterForm],
            BarContext::Help => &[C::Help],
            BarContext::ConfirmQuit => &[C::ConfirmQuit],
            BarContext::Picker => &[C::Picker],
            BarContext::Password | BarContext::PasswordNoSave => &[C::Password],
            BarContext::Trust => &[C::Trust],
            BarContext::Problem => &[C::Problem],
            BarContext::ProblemRetry => &[C::ProblemRetry, C::Problem],
            BarContext::Notice => &[C::Notice],
            BarContext::Keychain => &[C::Keychain],
            BarContext::ProblemReconnect => &[C::ProblemReconnect, C::Problem],
            BarContext::InstallChecking => &[C::InstallChecking],
            BarContext::InstallAsk => &[C::InstallAsk],
            BarContext::InstallDownloading => &[C::InstallDownloading],
            BarContext::InstallFailed => &[C::InstallFailed],
            BarContext::InstallFailedFinal => &[C::InstallFailedFinal],
            BarContext::Commit => &[C::Commit],
            BarContext::CommitProd => &[C::CommitProd, C::Commit],
            BarContext::ConfirmDiscard => &[C::ConfirmDiscard],
            BarContext::ConfirmRecommit => &[C::ConfirmRecommit],
            BarContext::QueueSwitch => &[C::QueueSwitch],
            BarContext::EditValue => &[C::EditValue],
            BarContext::QueryInsert => &[C::Insert, C::Query, C::Global],
            BarContext::QueryNormal => &[C::Normal, C::Query, C::Global],
            BarContext::QueryCommand => &[C::Command],
            BarContext::Completion => &[C::Completion, C::Insert, C::Query, C::Global],
            BarContext::Results => &[C::Results, C::Query, C::Global],
            BarContext::Params => &[C::Params],
            BarContext::RunConfirm => &[C::RunConfirm],
            BarContext::SaveAs => &[C::SaveAs],
            BarContext::Cell => &[C::Cell],
            BarContext::AskPrompt => &[C::AskPrompt],
            BarContext::AskMention => &[C::AskMention, C::AskPrompt],
            BarContext::AskWaiting => &[C::AskWaiting],
            BarContext::AskAnswer => &[C::AskAnswer],
            BarContext::AskDone => &[C::AskDone],
        }
    }

    /// The actions the bar lists, in order (the prototype's bars, minus
    /// what later tasks add).
    fn bar_actions(self) -> &'static [Action] {
        match self {
            BarContext::Connection => &[A::Pick, A::NextPanel, A::Focus, A::FocusMain, A::Help],
            BarContext::Tables => &[
                A::Down,
                A::Open,
                A::CycleTab,
                A::Reload,
                A::NextPanel,
                A::Undo,
                A::Commit,
                A::ShowQuery,
                A::Help,
            ],
            BarContext::Saved => &[
                A::Down,
                A::Open,
                A::OpenInQuery,
                A::CycleTab,
                A::Reload,
                A::NextPanel,
                A::ShowQuery,
                A::Help,
            ],
            // Design 1c's bar.
            BarContext::Pending => &[
                A::Unstage,
                A::Undo,
                A::EditValue,
                A::Commit,
                A::DiscardAll,
                A::Help,
            ],
            BarContext::Main => &[A::CycleTab, A::Back, A::Commit, A::Help],
            BarContext::Grid => &[
                A::CellDown,
                A::EditCell,
                A::StageDelete,
                A::InsertRow,
                A::Find,
                A::CycleTab,
                A::Undo,
                A::Commit,
                A::GridBack,
                A::Help,
            ],
            BarContext::CellEdit => &[A::ApplyEdit, A::CancelEdit],
            BarContext::Find => &[A::ApplyFind, A::ClearFind],
            BarContext::FilterForm => &[A::FormField, A::FormPrev, A::ApplyForm, A::CancelForm],
            BarContext::Help => &[A::Close, A::ScrollDown],
            BarContext::ConfirmQuit => &[A::Confirm, A::Cancel],
            BarContext::Picker => &[A::Down, A::Open, A::Back],
            BarContext::Password => &[A::ToggleSave, A::Submit, A::Cancel],
            BarContext::PasswordNoSave => &[A::Submit, A::Cancel],
            BarContext::Trust => &[A::Trust, A::Cancel],
            BarContext::Problem => &[A::Close],
            BarContext::ProblemRetry => &[A::Retry, A::Close],
            BarContext::Notice => &[A::Close],
            BarContext::Keychain => &[A::GiveUp],
            BarContext::ProblemReconnect => &[A::Reconnect, A::Close],
            BarContext::InstallChecking => &[A::StopInstall],
            BarContext::InstallAsk => &[A::Download, A::StopInstall],
            BarContext::InstallDownloading => &[A::StopInstall],
            BarContext::InstallFailed => &[A::InstallRetry, A::StopInstall],
            BarContext::InstallFailedFinal => &[A::StopInstall],
            BarContext::Commit => &[A::Execute, A::PreviewSql, A::Cancel],
            BarContext::CommitProd => &[A::Execute, A::PreviewSql, A::Cancel],
            BarContext::ConfirmDiscard => &[A::ConfirmDiscard, A::Cancel],
            BarContext::ConfirmRecommit => &[A::ConfirmRecommit, A::Cancel],
            BarContext::QueueSwitch => &[A::KeepQueue, A::DiscardQueue, A::Cancel],
            BarContext::EditValue => &[A::ApplyValue, A::Cancel],
            // Design 1b's bar (Decision 14's keys).
            BarContext::QueryInsert => &[
                A::RunAll,
                A::RunCurrent,
                A::Explain,
                A::Ask,
                A::SaveQuery,
                A::ExternalEditor,
                A::Complete,
                A::TogglePane,
                A::NormalMode,
            ],
            BarContext::QueryNormal => &[
                A::RunAll,
                A::RunCurrent,
                A::Ed(Normal::Insert),
                A::StartCommand,
                A::Ed(Normal::Undo),
                A::CycleTab,
                A::TogglePane,
                A::Help,
            ],
            BarContext::QueryCommand => &[A::RunCommand, A::CancelCommand],
            BarContext::Completion => &[A::AcceptCompletion, A::CompleteDown, A::CloseCompletion],
            BarContext::Results => &[
                A::ResultDown,
                A::OpenCell,
                A::ResultNextPage,
                A::CycleTab,
                A::RunAll,
                A::TogglePane,
                A::ResultsBack,
                A::Help,
            ],
            BarContext::Params => &[A::ParamsNext, A::ParamsSubmit, A::Cancel],
            BarContext::RunConfirm => &[A::ConfirmRun, A::Cancel],
            BarContext::SaveAs => &[A::SaveAsSubmit, A::Cancel],
            BarContext::Cell => &[A::CellScrollDown, A::Close],
            // Design 1d's keys.
            BarContext::AskPrompt => &[A::AskSend, A::AskMention, A::Close],
            BarContext::AskMention => &[A::MentionAccept, A::MentionDown, A::MentionClose],
            BarContext::AskWaiting => &[A::AskStop],
            BarContext::AskAnswer => &[A::AskInsert, A::AskRun, A::AskRefine, A::AskSave, A::Close],
            BarContext::AskDone => &[A::AskRefine, A::AskSave, A::Close],
        }
    }
}

/// The binding `key` triggers in `chain`, most specific context first.
pub fn lookup(chain: &[Context], key: Key) -> Option<&'static Binding> {
    chain.iter().find_map(|context| {
        BINDINGS
            .iter()
            .find(|b| b.context == *context && b.keys.contains(&key))
    })
}

/// The binding of `action` the bar of `context` names: the first in its
/// chain.
fn resolve(context: BarContext, action: Action) -> Option<&'static Binding> {
    context.chain().iter().find_map(|c| {
        BINDINGS
            .iter()
            .find(|b| b.context == *c && b.action == action && b.bar.is_some())
    })
}

/// The key bar's `(label, keys)` pairs for `context`.
pub fn bar(context: BarContext) -> Vec<(&'static str, &'static str)> {
    context
        .bar_actions()
        .iter()
        .filter_map(|action| resolve(context, *action).and_then(|b| b.bar))
        .collect()
}

/// One section of the `?` help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub title: &'static str,
    /// `(keys, what they do)`.
    pub lines: Vec<(&'static str, &'static str)>,
}

/// The `?` help: each context with a title, its bindings in table order,
/// then the terminal's notes. A binding with no help line (`k` beside
/// `j k`) is described by the one before it.
pub fn help() -> Vec<HelpSection> {
    let mut sections: Vec<HelpSection> = Vec::new();
    for binding in BINDINGS.iter().filter(|b| !b.help.is_empty()) {
        let Some(title) = binding.context.help_title() else {
            continue;
        };
        let line = (binding.help_keys, binding.help);
        match sections.iter_mut().find(|s| s.title == title) {
            Some(section) => section.lines.push(line),
            None => sections.push(HelpSection {
                title,
                lines: vec![line],
            }),
        }
    }
    sections.push(HelpSection {
        title: super::text::HELP_TERMINAL_TITLE,
        lines: super::text::HELP_TERMINAL.to_vec(),
    });
    sections
}

/// How many lines the help draws: a title per section and a line per
/// binding.
pub fn help_line_count() -> usize {
    help().iter().map(|s| 1 + s.lines.len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn shift_is_part_of_a_character_and_of_back_tab() {
        assert_eq!(
            Key::from_event(&press(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Key::char('G'))
        );
        assert_eq!(
            Key::from_event(&press(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            Some(Key::char('?'))
        );
        assert_eq!(
            Key::from_event(&press(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Key::code(KeyCode::BackTab))
        );
        assert_eq!(
            Key::from_event(&press(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Key::ctrl('c'))
        );
        let shift_enter = Key::from_event(&press(KeyCode::Enter, KeyModifiers::SHIFT)).unwrap();
        assert!(shift_enter.shift);
        let mut release = press(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(Key::from_event(&release), None);
        let mut repeat = press(KeyCode::Char('j'), KeyModifiers::NONE);
        repeat.kind = KeyEventKind::Repeat;
        assert_eq!(Key::from_event(&repeat), Some(Key::char('j')));
    }

    #[test]
    fn no_two_bindings_in_one_context_share_a_key() {
        for (i, a) in BINDINGS.iter().enumerate() {
            for b in &BINDINGS[i + 1..] {
                if a.context != b.context {
                    continue;
                }
                for key in a.keys {
                    assert!(
                        !b.keys.contains(key),
                        "{:?} has {key:?} for both {:?} and {:?}",
                        a.context,
                        a.action,
                        b.action
                    );
                }
            }
        }
    }

    #[test]
    fn a_more_specific_context_shadows_a_global_key() {
        let saved = lookup(BarContext::Saved.chain(), Key::char(']')).unwrap();
        assert_eq!((saved.context, saved.action), (C::Saved, A::CycleTab));
        let tables = lookup(BarContext::Tables.chain(), Key::char(']')).unwrap();
        assert_eq!((tables.context, tables.action), (C::Global, A::CycleTab));
        let pending = lookup(BarContext::Pending.chain(), Key::code(KeyCode::Enter)).unwrap();
        assert_eq!(pending.context, C::Pending);
        assert_eq!(
            lookup(BarContext::Tables.chain(), Key::char('j')).map(|b| b.action),
            Some(A::Down)
        );
        assert!(lookup(BarContext::Main.chain(), Key::char('j')).is_none());
    }

    #[test]
    fn dialogs_take_every_key() {
        for key in [Key::char('1'), Key::code(KeyCode::Tab), Key::char('[')] {
            assert!(lookup(BarContext::Help.chain(), key).is_none(), "{key:?}");
            assert!(
                lookup(BarContext::ConfirmQuit.chain(), key).is_none(),
                "{key:?}"
            );
        }
        // The connect dialogs: no global key gets through either.
        for context in [
            BarContext::Picker,
            BarContext::Password,
            BarContext::Trust,
            BarContext::Problem,
            BarContext::ProblemRetry,
            BarContext::Notice,
            BarContext::Keychain,
            BarContext::ProblemReconnect,
            BarContext::InstallChecking,
            BarContext::InstallAsk,
            BarContext::InstallDownloading,
            BarContext::InstallFailed,
            BarContext::InstallFailedFinal,
            BarContext::Commit,
            BarContext::CommitProd,
            BarContext::ConfirmDiscard,
            BarContext::ConfirmRecommit,
            BarContext::QueueSwitch,
            BarContext::EditValue,
            BarContext::QueryCommand,
            BarContext::Params,
            BarContext::RunConfirm,
            BarContext::SaveAs,
            BarContext::AskPrompt,
            BarContext::AskMention,
            BarContext::AskWaiting,
            BarContext::AskAnswer,
            BarContext::AskDone,
        ] {
            for key in [
                Key::char('1'),
                Key::char('['),
                Key::char('q'),
                Key::char('?'),
            ] {
                assert!(
                    lookup(context.chain(), key).is_none(),
                    "{context:?} {key:?}"
                );
            }
        }
        assert_eq!(
            lookup(BarContext::Password.chain(), Key::code(KeyCode::Tab)).map(|b| b.action),
            Some(A::ToggleSave)
        );
        assert_eq!(
            lookup(BarContext::ProblemRetry.chain(), Key::char('r')).map(|b| b.action),
            Some(A::Retry)
        );
        assert!(lookup(BarContext::Problem.chain(), Key::char('r')).is_none());
        // The DuckDB helper's dialogs (Task 7 of the DuckDB helper plan).
        assert_eq!(
            lookup(BarContext::ProblemReconnect.chain(), Key::char('r')).map(|b| b.action),
            Some(A::Reconnect)
        );
        assert_eq!(
            lookup(BarContext::InstallAsk.chain(), Key::code(KeyCode::Enter)).map(|b| b.action),
            Some(A::Download)
        );
        assert_eq!(
            lookup(BarContext::InstallFailed.chain(), Key::char('r')).map(|b| b.action),
            Some(A::InstallRetry)
        );
        for context in [
            BarContext::InstallChecking,
            BarContext::InstallAsk,
            BarContext::InstallDownloading,
            BarContext::InstallFailed,
            BarContext::InstallFailedFinal,
        ] {
            assert_eq!(
                lookup(context.chain(), Key::code(KeyCode::Esc)).map(|b| b.action),
                Some(A::StopInstall),
                "{context:?}"
            );
        }
        // Enter mustn't start a download anywhere but the question.
        for context in [BarContext::InstallChecking, BarContext::InstallDownloading] {
            assert!(lookup(context.chain(), Key::code(KeyCode::Enter)).is_none());
        }
        // A cell full size is read-only: like the help, `q` closes it.
        for key in [Key::char('1'), Key::char('['), Key::char('?')] {
            assert!(lookup(BarContext::Cell.chain(), key).is_none(), "{key:?}");
        }
        assert_eq!(
            lookup(BarContext::Cell.chain(), Key::char('q')).map(|b| b.action),
            Some(A::Close)
        );
        for key in [
            Key::code(KeyCode::Esc),
            Key::char('?'),
            Key::char('q'),
            Key::code(KeyCode::Enter),
        ] {
            assert_eq!(
                lookup(BarContext::Help.chain(), key).map(|b| b.action),
                Some(A::Close),
                "{key:?}"
            );
        }
    }

    #[test]
    fn every_bar_entry_is_a_binding_its_context_reaches() {
        for context in BarContext::ALL {
            let bar = bar(context);
            assert_eq!(bar.len(), context.bar_actions().len(), "{context:?}");
            for action in context.bar_actions() {
                let binding = resolve(context, *action)
                    .unwrap_or_else(|| panic!("{context:?} has no binding for {action:?}"));
                assert!(
                    binding.bar.is_some(),
                    "{context:?} {action:?} has no bar label"
                );
                assert!(context.chain().contains(&binding.context));
                // The key the bar names reaches that very binding.
                let key = binding.keys[0];
                assert!(std::ptr::eq(lookup(context.chain(), key).unwrap(), binding));
            }
        }
    }

    /// Probe F8: on macOS, Option+R and Option+X type `®` and `≈` unless
    /// the terminal is set to send Option as Meta (off by default in
    /// Terminal.app and iTerm2). So no bar names an Alt key: each action
    /// the bar shows has a key every terminal sends.
    #[test]
    fn no_bar_names_an_alt_key() {
        for context in BarContext::ALL {
            for (label, keys) in bar(context) {
                assert!(!keys.contains("alt+"), "{context:?}: {label} is {keys}");
            }
        }
        // The Alt binding stays, beside its alias.
        let chain = BarContext::QueryInsert.chain();
        assert_eq!(
            lookup(chain, Key::alt('r')).map(|b| b.action),
            lookup(chain, Key::ctrl('e')).map(|b| b.action)
        );
        assert_eq!(lookup(chain, Key::alt('r')).unwrap().action, A::RunCurrent);
    }

    /// Probe F2, F3 and F8: what the terminal does with the mouse and the
    /// Esc and Option keys, at the end of the help.
    #[test]
    fn the_help_ends_with_the_terminal_notes() {
        let sections = help();
        let last = sections.last().unwrap();
        assert_eq!(last.title, "Terminal");
        let text: Vec<String> = last.lines.iter().map(|(k, w)| format!("{k} {w}")).collect();
        let has = |s: &str| text.iter().any(|t| t.contains(s));
        assert!(has("shift+drag selects text in most terminals"), "{text:?}");
        assert!(has("--no-mouse"), "{text:?}");
        assert!(has("set -sg escape-time 10"), "{text:?}");
        assert!(has("Option"), "{text:?}");
        // Each fits the help's width (64, less the border and the keys).
        for (keys, what) in &last.lines {
            assert!(
                keys.len() <= 16 && what.chars().count() <= 62 - 18,
                "{what}"
            );
        }
    }

    #[test]
    fn the_help_lists_every_binding_with_a_help_line() {
        let all = help();
        let notes = all.last().unwrap().lines.len();
        let sections = &all[..all.len() - 1];
        let listed: usize = sections.iter().map(|s| s.lines.len()).sum();
        let described = BINDINGS
            .iter()
            .filter(|b| !b.help.is_empty() && b.context.help_title().is_some())
            .count();
        assert_eq!(listed, described);
        let titles: Vec<_> = sections.iter().map(|s| s.title).collect();
        assert_eq!(
            titles,
            [
                "Global",
                "Panels",
                "Tables and views",
                "Saved and History",
                "Pending changes",
                "Connection",
                "Main view",
                "Data grid",
                "Editing a cell",
                "Filtering",
                "Query",
                "Query editor: Insert mode",
                "Query editor: Normal mode",
                "Completion",
                "Results"
            ]
        );
        assert_eq!(sections[0].lines[0], ("1 2 3 4", "focus a panel"));
        // A line without help is described by the one before it.
        for binding in BINDINGS.iter().filter(|b| b.help.is_empty()) {
            assert!(
                binding.help_keys.is_empty() && binding.bar.is_none(),
                "{binding:?}"
            );
        }
        assert_eq!(help_line_count(), listed + sections.len() + 1 + notes);
    }

    /// The keys a piece of hand-written text names (`esc`, `j k`, `1-4`,
    /// `[ ]`, `shift+tab`, `arrows`), each as the keys it may mean.
    fn named(text: &str) -> Vec<Vec<Key>> {
        let arrows = [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right].map(Key::code);
        text.split(|c: char| c.is_whitespace() || c == '/')
            .filter(|t| !t.is_empty())
            .map(|token| match token {
                "esc" => vec![Key::code(KeyCode::Esc)],
                "enter" => vec![Key::code(KeyCode::Enter)],
                "tab" => vec![Key::code(KeyCode::Tab)],
                "space" => vec![Key::char(' ')],
                "shift+tab" => vec![Key::code(KeyCode::BackTab)],
                "arrows" => arrows.to_vec(),
                "ctrl+enter" => vec![Key::ctrl_code(KeyCode::Enter)],
                "ctrl+space" => vec![Key::ctrl(' ')],
                "f5" => vec![Key::code(KeyCode::F(5))],
                t if t.starts_with("alt+") => vec![Key::alt(t.chars().last().unwrap())],
                t if t.starts_with("ctrl+") => vec![Key::ctrl(t.chars().last().unwrap())],
                t if t.len() == 3 && t.as_bytes()[1] == b'-' => {
                    let (a, z) = (t.as_bytes()[0], t.as_bytes()[2]);
                    (a..=z).map(|c| Key::char(c as char)).collect()
                }
                t if t.chars().count() == 1 => vec![Key::char(t.chars().next().unwrap())],
                t => panic!("unknown key name {t:?}"),
            })
            .collect()
    }

    /// Every key a help line or bar entry names is one the binding (or the
    /// help-less ones right after it, `k` for `j k`) takes.
    #[test]
    fn help_and_bar_texts_name_only_bound_keys() {
        for (i, binding) in BINDINGS.iter().enumerate() {
            let mut keys: Vec<Key> = binding.keys.to_vec();
            for next in BINDINGS[i + 1..].iter().take_while(|b| b.help.is_empty()) {
                keys.extend_from_slice(next.keys);
            }
            let bar_keys = binding.bar.map(|(_, k)| k).unwrap_or("");
            for text in [binding.help_keys, bar_keys] {
                for meaning in named(text) {
                    // A range or `arrows` names several keys; each must be bound.
                    for key in &meaning {
                        let any_arrow = meaning.len() == 4
                            && meaning.iter().all(|k| {
                                matches!(
                                    k.code,
                                    KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
                                )
                            });
                        if any_arrow {
                            assert!(
                                meaning.iter().any(|k| keys.contains(k)),
                                "{binding:?}: {text}"
                            );
                            break;
                        }
                        assert!(keys.contains(key), "{binding:?} names {key:?} in {text:?}");
                    }
                }
            }
        }
    }

    /// The dialogs' footers (`text.rs`) name keys their context binds:
    /// `y quit   esc stay` and `esc close`.
    #[test]
    fn dialog_footers_name_bound_keys() {
        use crate::state::text::{HELP_FOOTER, PENDING_HINT, QUIT_FOOTER};
        for (footer, chain) in [
            (QUIT_FOOTER, BarContext::ConfirmQuit.chain()),
            (HELP_FOOTER, BarContext::Help.chain()),
            (PENDING_HINT, BarContext::Pending.chain()),
        ] {
            // Pairs of "key what".
            let words: Vec<&str> = footer.split_whitespace().collect();
            assert_eq!(words.len() % 2, 0, "{footer}");
            for pair in words.chunks(2) {
                for key in named(pair[0]).concat() {
                    assert!(lookup(chain, key).is_some(), "{footer}: {key:?}");
                }
            }
        }
    }
}
