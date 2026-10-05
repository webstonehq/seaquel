//! The TUI's strings, English only, in one place so a later translation has
//! one module to look at. The key bar's and help's strings live beside
//! their keys in `keymap.rs`.

pub const APP_NAME: &str = "seaquel";

pub const PANEL_CONNECTION: &str = "Connection";
pub const TAB_TABLES: &str = "Tables";
pub const TAB_VIEWS: &str = "Views";
pub const TAB_SAVED: &str = "Saved";
pub const TAB_HISTORY: &str = "History";
pub const PANEL_PENDING: &str = "Pending Changes";
pub const COMMAND_LOG: &str = "Command Log";

/// The main view's tabs for a table.
pub const MAIN_TABS_TABLE: &[&str] = &["Data", "Structure", "Indexes", "Constraints", "DDL"];
/// For a view: its rows and its columns.
pub const MAIN_TABS_VIEW: &[&str] = &["Data", "Columns"];
/// For a saved query or a history row.
pub const MAIN_TABS_SQL: &[&str] = &["SQL"];
/// For a staged change.
pub const MAIN_TABS_PENDING: &[&str] = &["Diff", "SQL"];

pub const NOT_CONNECTED: &str = "not connected";
pub const NO_CONNECTION: &str = "no connection";
pub const NO_SAVED: &str = "no saved queries";
pub const NO_HISTORY: &str = "no history";
pub const NOTHING_STAGED: &str = "nothing staged";
pub const NOTHING_SELECTED: &str = "nothing selected";
pub const LOG_EMPTY: &str = "statements Seaquel runs show here";

pub const CONNECTING: &str = "connecting";
pub const CLOSED: &str = "closed";
pub const PICK_HINT: &str = "enter to pick a connection";
pub const NO_TABLES: &str = "no tables";
pub const NO_VIEWS: &str = "no views";
pub const LOADING: &str = "loading…";
pub const LOAD_FAILED: &str = "couldn't read; r to try again";
pub const SHARED_MARKER: &str = "⇄";

pub const PICKER_PROJECTS: &str = "Projects";
pub const PICKER_CONNECTIONS: &str = "Connections";
pub const NO_PROJECTS: &str = "No projects yet. Create one in the Seaquel app.";
pub const NO_CONNECTIONS: &str = "No connections in this project. Add one in the Seaquel app.";

pub const PASSWORD_TITLE_DB: &str = "Password";
pub const PASSWORD_TITLE_SSH: &str = "SSH password";
pub const PASSWORD_TITLE_SSH_KEY: &str = "SSH key passphrase";
pub const SAVE_PASSWORD: &str = "Save password";
pub const PASSWORD_AGAIN: &str =
    "The last connect failed (the error is in the command log). If the password was the \
     cause, enter it again.";
pub const SAVE_PASSWORD_OFF: &str = "Save password (not available here)";

pub const TRUST_TITLE: &str = "Unknown SSH host";
pub const TRUST_BODY: &str =
    "This host's key isn't in known_hosts. Check the fingerprint with the server's \
     administrator before you trust it:";

pub const PROBLEM_TITLE_CONNECT: &str = "Couldn't connect";
pub const PROBLEM_TITLE_HOST_KEY: &str = "The SSH host key changed";
pub const PROBLEM_TITLE_ENGINE: &str = "This build can't connect to it";
pub const PROBLEM_TITLE_GONE: &str = "That connection is gone";
pub const PROBLEM_TITLE_NOT_INSTALLED: &str = "The DuckDB helper can't be used";
pub const PROBLEM_TITLE_HELPER: &str = "DuckDB didn't start";
pub const PROBLEM_TITLE_CLOSED: &str = "The connection was lost";
pub const KEYCHAIN_SAVING: &str = "The password is saved once you answer the dialog.";

/// The secret store each platform has, which the keychain dialogs name:
/// macOS's keychain, the Secret Service on Linux, Windows
/// Credential Manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    MacKeychain,
    SecretService,
    CredentialManager,
}

impl Store {
    /// This platform's.
    pub const fn here() -> Store {
        if cfg!(target_os = "macos") {
            Store::MacKeychain
        } else if cfg!(windows) {
            Store::CredentialManager
        } else {
            Store::SecretService
        }
    }
}

/// The store in a title: "keychain", "keyring", "Credential Manager".
pub const fn store_short(store: Store) -> &'static str {
    match store {
        Store::MacKeychain => "keychain",
        Store::SecretService => "keyring",
        Store::CredentialManager => "Credential Manager",
    }
}

pub const fn problem_title_keychain(store: Store) -> &'static str {
    match store {
        Store::MacKeychain => "Couldn't read the keychain",
        Store::SecretService => "Couldn't read the keyring",
        Store::CredentialManager => "Couldn't read Credential Manager",
    }
}

pub const fn keychain_title(store: Store) -> &'static str {
    match store {
        Store::MacKeychain => "Waiting for the keychain",
        Store::SecretService => "Waiting for the keyring",
        Store::CredentialManager => "Waiting for Credential Manager",
    }
}

pub const fn keychain_body(store: Store) -> &'static str {
    match store {
        Store::MacKeychain => {
            "Waiting for macOS to allow access to the keychain. Choose Always Allow in the \
             dialog, which may be behind this window."
        }
        Store::SecretService => {
            "Waiting for the system keyring (Secret Service). Unlock it in the dialog, which \
             may be behind this window."
        }
        Store::CredentialManager => "Waiting for Windows Credential Manager to answer.",
    }
}

pub const fn keychain_gave_up(store: Store) -> &'static str {
    match store {
        Store::MacKeychain => {
            "Stopped waiting for the keychain. Connect again to retry, and choose Always \
             Allow in the keychain dialog."
        }
        Store::SecretService => {
            "Stopped waiting for the system keyring (Secret Service). Connect again to \
             retry, and unlock the keyring when it asks."
        }
        Store::CredentialManager => {
            "Stopped waiting for Windows Credential Manager. Connect again to retry."
        }
    }
}

pub const fn ask_keychain_gave_up(store: Store) -> &'static str {
    match store {
        Store::MacKeychain => {
            "Stopped waiting for the keychain, so no request was sent. Ask again to retry, \
             and choose Always Allow in the keychain dialog."
        }
        Store::SecretService => {
            "Stopped waiting for the system keyring (Secret Service), so no request was \
             sent. Ask again to retry, and unlock the keyring when it asks."
        }
        Store::CredentialManager => {
            "Stopped waiting for Windows Credential Manager, so no request was sent. Ask \
             again to retry."
        }
    }
}

pub const fn save_password_hint(store: Store) -> &'static str {
    match store {
        Store::MacKeychain => {
            "Ticked, the password is saved in the macOS keychain once the connect succeeds."
        }
        Store::SecretService => {
            "Ticked, the password is saved in the system keyring (Secret Service) once the \
             connect succeeds."
        }
        Store::CredentialManager => {
            "Ticked, the password is saved in Windows Credential Manager once the connect \
             succeeds."
        }
    }
}

/// Why the prompt asks for a saved password, with saving off: the store
/// isn't there in this session.
pub const fn store_unavailable(store: Store) -> &'static str {
    match store {
        Store::MacKeychain => {
            "The keychain can't be used in this session (as over SSH), so the saved \
             password can't be read or saved here. Enter it to connect."
        }
        Store::SecretService => {
            "There's no system keyring (Secret Service) in this session, so the saved \
             password can't be read or saved here. Enter it to connect."
        }
        Store::CredentialManager => {
            "Windows Credential Manager isn't available, so the saved password can't be \
             read or saved here. Enter it to connect."
        }
    }
}

pub const NOT_CONNECTED_LOG: &str = "not connected";

// Browse.
pub const META_LOADING: &str = "the table's columns are still loading";
pub const NO_ROWS_MATCH: &str = "no rows match the filter · esc to clear";
pub const PAGE_LOADING: &str = "loading the page…";
pub const OPEN_HINT: &str = "enter to open it";
/// An unopened table's preview: `schema_tables` lists no columns on any
/// engine, so they're read when the table opens.
pub const LOAD_COLUMNS_HINT: &str = "enter to open it and load its columns";
pub const FILTER_PLACEHOLDER: &str = "filter…";
pub const DEFAULT_CELL: &str = "DEFAULT";
pub const DDL_APPROXIMATE: &str =
    "-- approximate: built by Core from the table's columns, keys and indexes";
pub const NO_INDEXES: &str = "no indexes";
pub const NO_CONSTRAINTS: &str = "no primary key, foreign keys or unique columns";
pub const CHECKS_NOT_LISTED: &str = "-- CHECK constraints aren't listed";
pub const MODE_EDIT: &str = "EDIT";
pub const ROW_DELETED: &str = "the row is staged for delete; d unstages it";

/// Staging on a connection other than the queue's (I3).
pub fn queue_elsewhere(count: usize, connection: &str) -> String {
    let changes = if count == 1 {
        "change is"
    } else {
        "changes are"
    };
    format!("{count} {changes} staged on {connection}; commit or discard them before staging here")
}

/// A plan Core couldn't make (not a refusal of the edit): kept, planned
/// again at commit.
pub fn plan_failed(code: &str, message: &str) -> String {
    format!("plan failed: {code}: {message} (kept; planned again at commit)")
}

/// `n matches on this page`, while `/` filters.
pub fn matches_on_page(n: usize) -> String {
    format!("{n} match{} on this page", if n == 1 { "" } else { "es" })
}

pub const COMPARES_AS_TEXT: &str = "compares as text";
// Query.
pub const MODE_INSERT: &str = "INSERT";
pub const QUERY_TABS_NEW: &str = "+";
/// A tab whose text was too long to keep in the state file.
pub const TAB_NOT_KEPT: &str = "this tab's text was over 1 MiB, so it wasn't kept between runs";
/// A restored saved query's tab before the library names it.
pub const SAVED_TAB: &str = "saved query";
pub const RESULT_TABS: &[&str] = &["Results", "Explain", "Messages"];
pub const NOT_CONNECTED_RUN: &str = "not connected: pick a connection in panel 1 (enter)";
pub const NOTHING_TO_RUN: &str = "nothing to run";
pub const NOTHING_TO_EXPLAIN: &str = "nothing to explain at the cursor";
pub const EXPLAIN_PARAMS: &str =
    "EXPLAIN doesn't fill {{parameters}}: replace them with values first";
pub const NEEDS_PROJECT: &str = "pick a project in panel 1 before saving";
pub const NAME_NEEDED: &str = "a saved query needs a name";
pub const SAVED_SHARED: &str =
    "this saved query is shared: the app writes it to the repo on its next sync";
pub const TAB_MODIFIED: &str = "the tab has changes that aren't saved: :w saves, :q! closes anyway";
pub const CANCELLED: &str = "cancelled";
/// A cancelled explain: the server may go on planning.
pub const EXPLAIN_STOPPED: &str = "Stopped waiting; the plan may still finish on the server";
/// A run cut at the cap was cancelled, so Core recorded no history (M8).
pub const NO_HISTORY_ROW: &str = "no history row was recorded";
pub const RUNNING: &str = "running… esc or ctrl+c cancels";
pub const NO_RESULTS: &str = "ctrl+r runs the text, ctrl+e the statement at the cursor";
pub const NO_ROWS_STATEMENT: &str = "no statement returned rows: the Messages tab says what ran";
pub const EXPLAIN_HINT: &str = "ctrl+x explains the statement at the cursor, :analyze with ANALYZE";
pub const EXPLAINING: &str = "explaining…";
pub const NO_RESULT_ROWS: &str = "no rows";
pub const EDITOR_PLACEHOLDER: &str = "type SQL · ctrl+r runs it · esc for Normal mode";
pub const PARAMS_TITLE: &str = "Parameters";
pub const PARAMS_HINT: &str = "NULL sets NULL; \\NULL is the text";
pub const RUN_CONFIRM_TITLE: &str = "Run it?";
pub const ANALYZE_TITLE: &str = "EXPLAIN ANALYZE runs the statement";
pub const SAVE_AS_TITLE: &str = "Save query";
pub const SAVE_AS_LABEL: &str = "name › ";

/// A new tab's title.
pub fn untitled(n: u32) -> String {
    format!("untitled-{n}")
}

/// `:something` the editor doesn't know.
pub fn unknown_command(command: &str) -> String {
    format!("unknown command :{}", super::grid::clean(command))
}

/// The command log's line for a statement that failed: its code.
pub fn statement_failed(code: &str) -> String {
    format!("statement failed: {code}")
}

/// "Stream all" stopped at the cap.
pub fn row_cap(cap: usize) -> String {
    format!(
        "stopped at {} rows (the most the TUI keeps)",
        super::grid::thousands(cap as u64)
    )
}

/// `NAME_TAKEN`, naming the saved query that has it.
pub fn name_taken(name: Option<&str>) -> String {
    match name {
        Some(name) => format!(
            "\"{}\" already exists in this project: choose another name",
            super::grid::clean(name)
        ),
        None => "That name is taken in this project: choose another".to_string(),
    }
}

/// The command log's line for a save.
pub fn saved_line(name: &str) -> String {
    format!("saved {}", super::grid::clean(name))
}

/// `$EDITOR` couldn't run, or exited with an error.
pub fn editor_failed(why: &str) -> String {
    format!("The external editor didn't run, so the text is unchanged: {why}")
}

/// The analyze question: what the statement does when it runs.
pub fn analyze_question(verb: &str) -> String {
    format!(
        "EXPLAIN ANALYZE runs this {} statement, so it changes what it changes. Run it?",
        super::grid::clean(verb)
    )
}

/// The destructive question.
pub fn destructive_question(total: u32) -> String {
    format!(
        "This run holds {total} destructive statement{}:",
        if total == 1 { "" } else { "s" }
    )
}

/// `n`/`p` on results from another connection.
pub fn results_elsewhere(connection: &str) -> String {
    format!(
        "these results came from {}; connect to it (or run again here) to page",
        super::grid::clean(connection)
    )
}

/// `statement 2 of 3` on the results box.
pub fn statement_of(n: usize, of: usize) -> String {
    format!("statement {n} of {of}")
}

/// A statement's line in Messages: `1 rows affected`.
pub fn rows_affected(n: u64) -> String {
    format!(
        "{} row{} affected",
        super::grid::thousands(n),
        if n == 1 { "" } else { "s" }
    )
}

/// A statement's line in Messages: `40 rows`.
pub fn rows_returned(n: u64) -> String {
    format!(
        "{} row{}",
        super::grid::thousands(n),
        if n == 1 { "" } else { "s" }
    )
}
pub const MODE_NORMAL: &str = "NORMAL";

/// The key bar's right side in the editor: `INSERT · Ln 7, Col 13`.
pub fn editor_mode(mode: &str, line: usize, column: usize) -> String {
    format!("{mode} · Ln {line}, Col {column}")
}
pub const MODE_FILTER: &str = "FILTER";

/// "id is the primary key" (prototype `startEdit`).
pub fn primary_key(column: &str) -> String {
    format!("{column} is the primary key")
}

/// An undo step's (and an unstaging's) words: "edit total · id 48109".
pub fn undo_edit(column: &str, key: &str) -> String {
    format!("edit {column} · {key}")
}

/// "set default total · id 48109".
pub fn undo_default(column: &str, key: &str) -> String {
    format!("set default {column} · {key}")
}

/// "delete id 48106".
pub fn undo_delete(key: &str) -> String {
    format!("delete {key}")
}

/// "insert into invoices".
pub fn undo_insert(table: &str) -> String {
    format!("insert into {table}")
}

// Pending Changes and commit.

/// Panel 4's last line (design 1c; "commit all" is "commit", so the words
/// pair up with their keys).
pub const PENDING_HINT: &str = "space unstage   u undo   c commit";
pub const NOTHING_STAGED_HINT: &str = "edit a cell with e or stage a delete with d";
pub const COMMIT_RUNNING: &str = "a commit is running; wait for it to finish";
pub const NOTHING_TO_COMMIT: &str = "nothing to commit: an inserted row needs a value first";
pub const DELETE_HAS_NO_VALUE: &str = "a delete has no value to edit; space unstages it";
pub const INSERT_EDITS_IN_GRID: &str =
    "an insert's values are edited in the grid: open its table from panel 2";
pub const NOT_PLANNED_YET: &str = "-- not planned yet: Core plans it at commit";
pub const PLANNING: &str = "-- Core is planning it…";
pub const PLANNED_BY_CORE: &str = "-- planned by Core";
pub const DIFF_DEFAULT: &str = "DEFAULT";
pub const NEW_ROW: &str = "new row";
pub const EMPTY_INSERT: &str = "-- an inserted row with no value yet: nothing to send";
pub const PROD_WARNING_DELETE: &str = "⚠ Includes a DELETE on a connection tagged production.";
pub const PROD_WARNING: &str = "⚠ This connection is tagged production.";
pub const TYPE_PROD: &str = "type prod to confirm › ";
pub const DESTRUCTIVE_HEADER: &str = "⚠ Destructive statements:";
pub const DISCARD_TITLE: &str = "Discard?";
/// A commit cut off by its connection.
pub const COMMIT_INTERRUPTED: &str =
    "the connection closed during the commit: it may have been applied; check the data before committing again";
pub const MAYBE_APPLIED_HINT: &str = "may be partly applied: check the data";
pub const RECOMMIT_TITLE: &str = "Commit again?";
pub const RECOMMIT_QUESTION: &str = "The last commit was cut off when the connection closed and may have been applied. Committing again runs every change again: check the data first.";
pub const SWITCH_TITLE: &str = "Staged changes";
pub const BEGIN: &str = "BEGIN;";
pub const STAGING_ELSEWHERE: &str =
    "Keep them staged there (connect to it to commit them), or discard them to stage here.";

/// The switch question's choices.
pub fn switch_choices(to: &str) -> String {
    format!("Keep them staged there and connect to {to}, discard them and connect, or stay.")
}

/// "Edit value · total".
pub fn edit_value_title(column: &str) -> String {
    format!("Edit value · {}", super::grid::clean(column))
}

/// "Commit 4 changes".
pub fn commit_title(count: usize) -> String {
    format!("Commit {count} change{}", if count == 1 { "" } else { "s" })
}

/// "Run on prod-analytics in a single transaction:".
pub fn run_on(connection: &str, atomic: bool) -> String {
    if atomic {
        format!("Run on {connection} in a single transaction:")
    } else {
        format!("Run on {connection}:")
    }
}

/// While Core plans what wasn't planned.
pub fn planning(count: usize) -> String {
    format!(
        "Core is planning {count} change{}…",
        if count == 1 { "" } else { "s" }
    )
}

/// A plan Core couldn't make before the commit.
pub fn not_planned(count: usize) -> String {
    format!(
        "{count} change{} couldn't be planned (the command log says why); esc, then c to try again",
        if count == 1 { "" } else { "s" }
    )
}

/// "…N more", a list cut to fit.
pub fn more(count: usize) -> String {
    format!("…{count} more")
}

/// "…and N more" past Core's list.
pub fn and_more(count: u32) -> String {
    format!("…and {count} more")
}

/// "Discard 3 staged changes?"
pub fn discard_question(count: usize) -> String {
    format!(
        "Discard {count} staged change{}? This can't be undone.",
        if count == 1 { "" } else { "s" }
    )
}

/// "discarded 3 changes".
pub fn discarded(count: usize) -> String {
    format!(
        "discarded {count} change{}",
        if count == 1 { "" } else { "s" }
    )
}

/// The switch question's body.
pub fn staged_on(count: usize, connection: &str) -> String {
    let changes = if count == 1 {
        "change is"
    } else {
        "changes are"
    };
    format!("{count} {changes} staged on {connection}.")
}

/// `c` while the queue belongs to another connection.
pub fn commit_elsewhere(count: usize, connection: &str) -> String {
    format!(
        "{count} change{} staged on {connection}; connect to it to commit",
        if count == 1 { " is" } else { "s are" }
    )
}

/// An undo step's words for an unstaging: "unstage edit total · id 1".
pub fn undo_unstage(what: &str) -> String {
    format!("unstage {what}")
}

/// `NO_ROWS_AFFECTED`, naming the change's table and key.
pub fn no_row(table: &str, key: &str) -> String {
    format!(
        "no row matched in {table} · {key}: it was changed or deleted since it was loaded \
         (r in the grid reads the page again)"
    )
}

/// An apply that stopped: the change's code and Core's message.
pub fn apply_failed(code: &str, message: &str) -> String {
    format!("{code}: {message}")
}

/// "COMMIT; 4 statements".
pub fn committed(count: usize) -> String {
    format!(
        "COMMIT; {count} statement{}",
        if count == 1 { "" } else { "s" }
    )
}

/// "ROLLBACK; NO_ROWS_AFFECTED".
pub fn rolled_back(code: &str) -> String {
    format!("ROLLBACK; {code}")
}

/// A destructive statement's reason, worded.
pub fn destructive_reason(
    reason: seaquel_core::sql::statements::DestructiveReason,
) -> &'static str {
    use seaquel_core::sql::statements::DestructiveReason as R;
    match reason {
        R::DropTable => "drops a table",
        R::DropIndex => "drops an index",
        R::DropView => "drops a view",
        R::DropSchema => "drops a schema",
        R::DropDatabase => "drops a database",
        R::DropSequence => "drops a sequence",
        R::DropFunction => "drops a function",
        R::DropColumn => "drops a column",
        R::Truncate => "empties a table",
        R::DeleteNoWhere => "DELETE without WHERE",
        R::UpdateNoWhere => "UPDATE without WHERE",
        R::MergeDelete => "MERGE that deletes",
    }
}

pub const HELP_TITLE: &str = "Keybindings";
pub const HELP_FOOTER: &str = "esc close";

/// The help's last section: what the terminal does with the mouse and
/// the Esc and Option keys. `(keys, text)`; an empty
/// key continues the line above.
pub const HELP_TERMINAL_TITLE: &str = "Terminal";
pub const HELP_TERMINAL: &[(&str, &str)] = &[
    ("shift+drag", "selects text in most terminals"),
    ("", "(option+drag in iTerm2, fn+drag in Terminal)"),
    ("--no-mouse", "leaves clicks and the wheel to the terminal"),
    ("tmux", "add set -sg escape-time 10 to tmux.conf,"),
    ("", "or esc then a key may read as alt+key"),
    (
        "option (macOS)",
        "types ® ≈ unless Option sends Meta (Esc+)",
    ),
    ("", "in the terminal's settings: ctrl+e, ctrl+x"),
    ("", "and :analyze need no Option"),
];

pub const QUIT_TITLE: &str = "Quit?";
pub const QUIT_PLAIN: &str = "Quit seaquel-tui?";
pub const QUIT_COMMITTING: &str =
    "A commit is running; quitting stops it and it may be partly applied.";
pub const QUIT_RUNNING: &str = "A statement is running; quitting cancels it.";
pub const QUIT_FOOTER: &str = "y quit   esc stay";

/// "Seaquel needs at least 80×24 (this is W×H)".
pub fn too_small(width: u16, height: u16) -> String {
    format!("Seaquel needs at least 80×24 (this is {width}×{height})")
}

/// "n of m", a list's counter on its bottom border.
pub fn counter(selected: usize, len: usize) -> String {
    format!("{} of {len}", selected + 1)
}

/// "N staged changes will be lost."
pub fn quit_staged(count: usize) -> String {
    if count == 1 {
        "1 staged change will be lost.".to_string()
    } else {
        format!("{count} staged changes will be lost.")
    }
}

/// The key bar's right side when no mode is on: "seaquel <version>".
pub fn version_label() -> String {
    format!("{APP_NAME} {}", seaquel_terminal::VERSION)
}

/// A password saved from the TUI. On macOS the app is asked
/// once to read a keychain item the TUI created.
pub fn password_saved(macos: bool) -> String {
    if macos {
        "Saved. The Seaquel app may ask once to read this password; choose Always Allow."
            .to_string()
    } else {
        "Saved.".to_string()
    }
}

/// A save that failed: the connection stays up with the typed password.
pub fn password_not_saved(message: &str) -> String {
    format!("Connected, but the password wasn't saved: {message}")
}

/// The command log's line for a connect.
pub fn connected_line(name: &str, engine: &str) -> String {
    format!("connected {name} ({engine})")
}

/// The command log's line for a failed connect or load: the code only.
pub fn failed_line(what: &str, code: &str) -> String {
    format!("{what} failed: {code}")
}

// ── The DuckDB helper's install dialog ──

pub const INSTALL_TITLE: &str = "DuckDB support";
pub const INSTALL_ASK_TITLE: &str = "Download DuckDB support?";
pub const INSTALL_FAILED_TITLE: &str = "DuckDB support wasn't installed";

/// The size lookup.
pub const INSTALL_CHECKING: &str = "DuckDB support isn't installed. Looking up its download…";

/// The question: the size before anything is fetched.
pub fn install_ask(size: &str) -> String {
    format!("DuckDB support is a separate download of {size}.")
}
pub const INSTALL_QUESTION: &str = "Download now?";

/// Which binary's version it is for, on a line of its own (so the
/// version's length never moves a line break).
pub fn install_for() -> String {
    format!("For seaquel-tui {}.", seaquel_terminal::VERSION)
}

pub const INSTALL_CHECKED: &str =
    "Its size and SHA-256 are checked before it's saved in Seaquel's data folder. The connection \
     continues once it's installed.";

/// The helper there sits in a folder someone else can change (`Unsafe`).
pub const INSTALL_REPAIR: &str =
    "The DuckDB helper already there is in a folder that isn't private. Installing again makes \
     it private.";

pub const INSTALL_DOWNLOADING: &str = "Downloading DuckDB support…";
pub const INSTALL_STARTING: &str = "starting…";

/// `4.2 MB of 11.7 MB`.
pub fn install_progress(bytes: &str, total: &str) -> String {
    format!("{bytes} of {total}")
}

/// The command log's lines.
pub fn install_started_line(size: &str) -> String {
    format!("downloading DuckDB support ({size})")
}
pub const INSTALLED_LINE: &str = "installed DuckDB support";
/// A connect (after an install, or a reconnect) whose saved connection was
/// removed meanwhile.
pub const CONNECTION_REMOVED: &str = "That connection was removed. Pick another one.";
pub const INSTALL_STOPPED_LINE: &str = "DuckDB support's download stopped";

/// A failed lookup or download: a title and what to do (Core's message is
/// shown under them).
pub fn install_failure(code: &str) -> (&'static str, &'static str) {
    match code {
        "NETWORK_ERROR" => (
            "Couldn't reach the download server",
            "Check the network connection, or the proxy in HTTPS_PROXY, then retry.",
        ),
        "RELEASE_NOT_FOUND" | "ASSET_NOT_FOUND" => (
            "DuckDB support isn't published for this version",
            "This seaquel-tui has no DuckDB download for this platform yet. Retry later, or use \
             the Seaquel app.",
        ),
        "DIGEST_MISMATCH" | "SIZE_MISMATCH" | "GZIP_ERROR" => (
            "The download was damaged",
            "Nothing was installed. Retry to download it again.",
        ),
        "DIGEST_MISSING"
        | "DIGEST_INVALID"
        | "SIZE_INVALID"
        | "ASSET_TOO_LARGE"
        | "RELEASE_METADATA_INVALID"
        | "HTTP_ERROR"
        | "REDIRECT_REFUSED" => (
            "The download server's answer can't be used",
            "Nothing was installed. Retry later.",
        ),
        "FILE_ERROR" => (
            "Couldn't save DuckDB support",
            "Check that the disk has room and that Seaquel's data folder can be written, then \
             retry.",
        ),
        "UNSAFE_FOLDER" => (
            "Seaquel's data folder can't be used",
            "A folder on the way to the DuckDB helper belongs to another user or is a link. \
             Remove the bin/duckdb folder in Seaquel's data folder and install again; if \
             SEAQUEL_DATA_DIR is set, point it at a folder of your own on this computer.",
        ),
        "NOT_SUPPORTED" => (
            "DuckDB support can't be downloaded here",
            "This platform has no DuckDB download.",
        ),
        _ => (
            "Couldn't install DuckDB support",
            "Retry, or look for the code in the log.",
        ),
    }
}

/// "Password for <name>", the prompt's first line.
pub fn password_for(what: &str, name: &str) -> String {
    format!("{what} for {name}")
}

/// What `seaquel-tui` prints when stdout isn't a terminal.
pub const NEEDS_A_TERMINAL: &str = "seaquel-tui needs a terminal on stdin and stdout";

/// What the panic hook prints after the terminal is restored.
pub fn crashed(log: Option<&std::path::Path>) -> String {
    match log {
        Some(log) => format!("seaquel-tui crashed; the log is at {}", log.display()),
        None => "seaquel-tui crashed (no log: the data dir doesn't exist)".to_string(),
    }
}

// Ask AI.
pub const ASK_TITLE: &str = "Ask AI";
pub const ASK_PROMPT: &str = "› ";
pub const ASK_REFINE: &str = "refine › ";
pub const ASK_PLACEHOLDER: &str =
    "what should the SQL do? @ names a table, saved query or dashboard";
pub const ASK_WAITING: &str = "generating… esc stops";
pub const ASK_EMPTY: &str = "type what the SQL should do first";
pub const ASK_NO_SQL: &str = "The model answered without any SQL. Try asking another way.";
pub const ASK_STOPPED: &str = "Stopped. The request was dropped.";
pub const ASK_NEEDS_CONNECTION: &str =
    "Ask AI needs a saved connection: pick one in panel 1 (enter)";
pub const ASK_NO_TAB: &str = "Ask AI inserts into a query tab: Q opens one";
pub const ASK_NO_MENTIONS: &str = "nothing by that name";
/// The status line's first part for an AI turned off in the app.
pub const ASK_AI_OFF: &str = "AI is turned off · read-only";

/// The title's sharing line (Core's rule for the connection): what a model
/// call may send. "read-only" always: only reads run from the popup.
pub fn ask_sharing(schema: bool, data: bool) -> String {
    let shared = |on: bool| if on { "shared" } else { "not shared" };
    format!(
        "schema {} · data {} · read-only",
        shared(schema),
        shared(data)
    )
}

/// `✓ generated in 1.8s · <model>`.
pub fn ask_generated(elapsed_ms: u64, model: Option<&str>) -> String {
    let secs = format!("{:.1}s", elapsed_ms as f64 / 1000.0);
    match model.filter(|m| !m.is_empty()) {
        Some(model) => format!("✓ generated in {secs} · {}", super::grid::clean(model)),
        None => format!("✓ generated in {secs}"),
    }
}

/// Core's refusal of a model call, worded. A provider's own
/// message comes from Core already cut at 1 KiB with the key redacted.
pub fn ask_error(code: &str, message: &str) -> String {
    let message = super::grid::clean(message);
    match code {
        "NO_PROVIDER" => format!(
            "{message} Add an AI provider in the Seaquel app (Settings, AI), then ask again."
        ),
        "NO_API_KEY" => "The AI provider has no API key. Add it in the Seaquel app \
             (Settings, AI), then ask again."
            .to_string(),
        "NO_MODEL" => "This connection has no AI model chosen. Pick one in the Seaquel \
             app's assistant for this connection, then ask again."
            .to_string(),
        "AI_DISABLED" => "AI is turned off in the Seaquel app (Settings, AI).".to_string(),
        "PROVIDER_ERROR" => format!("The AI provider refused the request: {message}"),
        "RATE_LIMITED" => {
            "The AI provider is limiting requests right now. Wait a moment and ask again."
                .to_string()
        }
        "TIMEOUT" => "The AI provider took too long to answer. Ask again.".to_string(),
        "SECRET_UNREADABLE" | "SECRET_STORE_UNAVAILABLE" => format!(
            "The API key couldn't be read from the {}: {message}",
            store_short(Store::here())
        ),
        _ => format!("{code}: {message}"),
    }
}

/// Ctrl+R on SQL the read-only check refused: inserted, not run.
pub fn ask_not_run(verb: &str) -> String {
    if verb.is_empty() {
        "Inserted, not run: only read-only statements run from here. Read it, then run it \
         with ctrl+r in the editor."
            .to_string()
    } else {
        format!(
            "Inserted, not run: only read-only statements run from here, and this is {} \
             statement. Read it, then run it with ctrl+r in the editor.",
            super::grid::clean(&article(verb))
        )
    }
}

fn article(verb: &str) -> String {
    let vowel = verb
        .chars()
        .next()
        .is_some_and(|c| "AEIOUaeiou".contains(c));
    format!("{} {verb}", if vowel { "an" } else { "a" })
}

pub const ASK_NOT_RUN_SEVERAL: &str =
    "Inserted, not run: it holds more than one statement. Run them from the editor.";
pub const ASK_NOT_RUN_ALONE: &str =
    "Inserted, not run: it isn't a statement of its own where the cursor was.";
pub const ASK_NOT_CONNECTED: &str =
    "Inserted, not run: not connected. Connect in panel 1 (enter), then run it.";
pub const ASK_INSERTED: &str = "Already inserted: esc closes";

/// Ctrl+S saved the generated SQL.
pub fn ask_saved(name: &str) -> String {
    format!("Saved as {}", super::grid::clean(name))
}

#[cfg(test)]
mod store_tests {
    use super::*;

    /// The keychain dialogs name each platform's store, and only
    /// macOS's say "Always Allow".
    #[test]
    fn the_keychain_dialogs_are_worded_per_platform() {
        for (store, name, other) in [
            (Store::MacKeychain, "keychain", "Secret Service"),
            (Store::SecretService, "Secret Service", "macOS"),
            (Store::CredentialManager, "Credential Manager", "macOS"),
        ] {
            let texts = [
                keychain_body(store),
                keychain_gave_up(store),
                ask_keychain_gave_up(store),
                save_password_hint(store),
                store_unavailable(store),
            ];
            for (i, t) in texts.iter().enumerate() {
                assert!(t.contains(name), "{store:?}: {t}");
                assert!(!t.contains(other), "{store:?}: {t}");
                // The waiting box and the two give-ups name the dialog's button.
                assert_eq!(
                    t.contains("Always Allow"),
                    store == Store::MacKeychain && i < 3,
                    "{store:?}: {t}"
                );
            }
            assert!(problem_title_keychain(store).starts_with("Couldn't read"));
            assert!(keychain_title(store).starts_with("Waiting for"));
        }
        let here = Store::here();
        if cfg!(target_os = "macos") {
            assert_eq!(here, Store::MacKeychain);
        } else if cfg!(windows) {
            assert_eq!(here, Store::CredentialManager);
        } else {
            assert_eq!(here, Store::SecretService);
        }
    }
}
