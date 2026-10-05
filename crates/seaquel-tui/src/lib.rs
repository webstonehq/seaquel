//! `seaquel-tui`: Seaquel in the terminal, a lazygit-style client over Core
//! in a binary of its own (phase 7a). `main.rs` only calls [`run`].
//!
//! - `state/`: the model and `update`, pure (no Core, no I/O, no clock);
//! - `view/`: the renderer, which only reads the model;
//! - `runtime/`: the terminal, the event loop, the log file, the state file
//!   and the Core calls (Core opens as a second process beside the app,
//!   before the terminal is entered).
//!
//! Nothing prints to stdout or stderr while the screen is up: logs go to
//! `<data_dir>/logs/tui.log`, and a startup failure is printed after the
//! terminal is restored, with a non-zero exit.
//!
//! **Test hooks, debug builds only** (`seaquel-terminal`'s `TestHooks`
//! under `SEAQUEL_TUI_TEST`): `SEAQUEL_TUI_TEST_SECRETS`,
//! `SEAQUEL_TUI_TEST_KNOWN_HOSTS`, `SEAQUEL_TUI_TEST_ORIGIN` and
//! `SEAQUEL_TUI_TEST_PANIC` (`main` or `task`). A release build ignores
//! them. `_SECRETS` replaces the keychain (and takes "Save password"'s
//! writes), `_KNOWN_HOSTS` replaces `~/.ssh/known_hosts` (and takes a
//! trusted host's line), `_ORIGIN` fixes the write origin.

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;
use seaquel_terminal::{LogLevel, TestHooks, TERMS_LINE, VERSION_TEXT};

pub mod runtime;
pub mod state;
pub mod view;

#[cfg(test)]
mod testing;

use runtime::event_loop::Exit;
use runtime::AppError;
use state::text;
use view::theme::{Theme, ThemeChoice, ThemeEnv};

/// The prefix of the TUI's test hooks.
pub const TEST_HOOKS_PREFIX: &str = "SEAQUEL_TUI_TEST";

#[derive(Parser)]
#[command(
    name = "seaquel-tui",
    version = VERSION_TEXT,
    about = "Seaquel in the terminal: browse, edit and query your saved connections.",
    after_help = TERMS_LINE
)]
pub struct TuiArgs {
    /// Start in this project, by id or exact name.
    #[arg(long, value_name = "NAME_OR_ID")]
    pub project: Option<String>,
    /// Connect to this saved connection, by id or exact name.
    #[arg(long, value_name = "NAME_OR_ID")]
    pub connection: Option<String>,
    /// The colour theme (default dark). NO_COLOR turns colour off.
    #[arg(long, value_enum)]
    pub theme: Option<ThemeChoice>,
    /// Don't capture the mouse (clicks and scrolling then belong to the
    /// terminal).
    #[arg(long)]
    pub no_mouse: bool,
    /// Rows per page in grids (default 100).
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub page_size: Option<u32>,
    /// How much to log to <data dir>/logs/tui.log.
    #[arg(long, value_enum, default_value_t = LogLevel::Warn)]
    pub log_level: LogLevel,
}

/// No names in `Debug`: only which options were given.
impl std::fmt::Debug for TuiArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiArgs")
            .field("project", &self.project.is_some())
            .field("connection", &self.connection.is_some())
            .field("theme", &self.theme)
            .field("no_mouse", &self.no_mouse)
            .field("page_size", &self.page_size)
            .field("log_level", &self.log_level)
            .finish()
    }
}

/// Runs the TUI and returns the process exit code.
pub fn run(args: TuiArgs) -> ExitCode {
    // Both ends: keys come from stdin. (crossterm would fall back to
    // reopening /dev/tty, which macOS's kqueue can't poll: it spins.)
    if !std::io::stdout().is_terminal() || !std::io::stdin().is_terminal() {
        say(text::NEEDS_A_TERMINAL);
        return ExitCode::FAILURE;
    }
    let hooks = TestHooks::from_env(TEST_HOOKS_PREFIX);
    let dir = match seaquel_terminal::data_dir() {
        Ok(dir) => dir,
        Err(e) => {
            say(format_args!("seaquel-tui: {}: {}", e.code, e.message));
            return ExitCode::FAILURE;
        }
    };
    let log = match runtime::logging::init(&dir, args.log_level) {
        Ok(log) => log,
        Err(e) => {
            say(format_args!("seaquel-tui: can't open the log file: {e}"));
            return ExitCode::FAILURE;
        }
    };
    // The state file: `--theme` is remembered once given.
    let mut remembered = runtime::state_file::load(&dir);
    let choice = match args.theme {
        Some(choice) => {
            remembered.theme = Some(theme_name(choice).to_string());
            choice
        }
        None => match remembered.theme.as_deref() {
            Some("light") => ThemeChoice::Light,
            _ => ThemeChoice::Dark,
        },
    };
    let theme = Theme::pick(&ThemeEnv::from_process(), choice);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            say(format_args!("seaquel-tui: can't start the runtime: {e}"));
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(runtime::run_app(runtime::AppOptions {
        mouse: !args.no_mouse,
        theme,
        hooks,
        log: log.clone(),
        data_dir: dir,
        project: args.project,
        connection: args.connection,
        remembered,
        page_size: args.page_size,
    }));
    // Core stops a statement still running on the server from a task of
    // its own, spawned as the stream drops (Postgres's `pg_cancel_backend`,
    // MySQL's `KILL QUERY`, each with a 5 s limit): give those time to land
    // (a closed terminal left a streaming statement running).
    runtime.block_on(runtime::settle(runtime::SETTLE_WITHIN));
    // crossterm's event reader may still sit in a blocking read; nothing
    // that matters is left running.
    runtime.shutdown_background();
    // The terminal is restored by now (the guard), so this is the screen
    // the user's shell sees.
    match result {
        Ok(Exit::Crashed) => {
            // A Core task's panic: the hook kept its message off the screen.
            if let Some(message) = runtime::terminal::take_panic_message() {
                say(message);
            }
            say(text::crashed(log.as_deref()));
            ExitCode::FAILURE
        }
        // A closed terminal (SIGHUP, or EIO first) ends as SIGHUP does.
        Ok(Exit::Quit | Exit::Signal(_) | Exit::EventsEnded | Exit::TerminalClosed) => {
            ExitCode::SUCCESS
        }
        Err(AppError::Startup(message)) => {
            say(format_args!("seaquel-tui: {message}"));
            ExitCode::FAILURE
        }
        Err(AppError::Terminal(e)) => {
            say(format_args!("seaquel-tui: {e}"));
            ExitCode::FAILURE
        }
    }
}

/// A line on stderr. A failed write is ignored: once the terminal is
/// closed every write fails with EIO, and `eprintln!` would panic (then
/// abort, in the panic hook) before Core's connections were closed.
pub(crate) fn say(line: impl std::fmt::Display) {
    use std::io::Write;
    let _ = writeln!(std::io::stderr(), "{line}");
}

fn theme_name(choice: ThemeChoice) -> &'static str {
    match choice {
        ThemeChoice::Dark => "dark",
        ThemeChoice::Light => "light",
    }
}
