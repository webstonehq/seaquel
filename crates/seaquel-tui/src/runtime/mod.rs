//! The runtime (Decision 2): it owns the terminal, the event loop and the
//! Core calls. It turns `update`'s effects into actions and feeds their
//! results back as messages.
//!
//! **Startup** (Task 3): Core opens before the terminal is entered, so a
//! refused data dir (`STORAGE_NOT_FOUND`, `STORAGE_NEEDS_UPGRADE`,
//! `LEGACY_STORAGE`, `STORAGE_CORRUPT`) or a `--project`/`--connection`
//! that names nothing is printed to a normal terminal with a non-zero exit.
//! The open races the shutdown signals: a SIGTERM during a slow open (a
//! keychain dialog, a migration lock) drops it, which closes what it had
//! opened.

pub mod clock;
pub mod core;
pub mod effects;
pub mod event_loop;
pub mod external_editor;
pub mod input;
pub mod logging;
pub mod state_file;
pub mod terminal;

#[cfg(test)]
mod app_tests;
#[cfg(test)]
mod ask_tests;
#[cfg(test)]
mod browse_tests;
#[cfg(test)]
mod commit_tests;
#[cfg(test)]
mod install_tests;
#[cfg(test)]
mod query_tests;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use seaquel_terminal::TestHooks;

use crate::state::app::{update, Model, Msg, SavedTab, TablesTab};
use crate::state::connect;
use crate::state::picker::{resolve_start, Remembered};
use crate::view::theme::Theme;
use crate::view::view;
use effects::Runner;
use event_loop::Exit;

/// What [`run_app`] needs from the command line and the environment.
#[derive(Debug)]
pub struct AppOptions {
    pub mouse: bool,
    pub theme: Theme,
    pub hooks: TestHooks,
    /// The log file, which the panic hook names; `None` when the data dir
    /// doesn't exist (no log is written then).
    pub log: Option<PathBuf>,
    pub data_dir: PathBuf,
    /// `--project` and `--connection`.
    pub project: Option<String>,
    pub connection: Option<String>,
    /// The state file as read.
    pub remembered: Remembered,
    /// `--page-size`.
    pub page_size: Option<u32>,
}

/// Why [`run_app`] failed.
#[derive(Debug)]
pub enum AppError {
    /// Refused before the screen was taken; the text is for stderr.
    Startup(String),
    Terminal(io::Error),
}

impl From<io::Error> for AppError {
    fn from(e: io::Error) -> AppError {
        AppError::Terminal(e)
    }
}

/// Opens Core, enters the terminal, runs the loop and restores the
/// terminal on every path out (`terminal.rs` lists them), then closes
/// Core's connections and writes the state file.
pub async fn run_app(options: AppOptions) -> Result<Exit, AppError> {
    use seaquel_terminal::{ShutdownSignal, ShutdownSignals};

    // Before anything else, so a signal during startup is kept rather than
    // fatal.
    let mut signals = ShutdownSignals::install(&[
        ShutdownSignal::Terminate,
        ShutdownSignal::Hangup,
        ShutdownSignal::Interrupt,
    ]);
    let suspend = outside_suspend();
    terminal::install_panic_hook(options.log.clone());

    let opening = open_core(&options);
    let (session, library, start) = tokio::select! {
        opened = opening => opened?,
        name = event_loop::wait_signal(&mut signals) => {
            log::info!("Got {name} while starting");
            return Ok(Exit::Signal(name));
        }
    };
    let session = Arc::new(session);
    // Leftover `$EDITOR` files of a run that crashed (review M2).
    let swept = external_editor::sweep(
        &options.data_dir,
        std::time::Duration::from_secs(24 * 60 * 60),
    );
    if swept > 0 {
        log::info!(activity = "tui.external_editor", removed = swept; "Removed old edit files");
    }

    let terminal_options = terminal::Options {
        mouse: options.mouse,
    };
    terminal::enter(terminal_options)?;
    let guard = terminal::Guard;
    let mut tui = terminal::new_terminal()?;
    let size = tui.size()?;
    let mut model = Model::new((size.width, size.height), options.theme);
    model.kitty_keys = terminal::keyboard_enhanced();
    if options.remembered.tables_tab.as_deref() == Some("views") {
        model.tables_tab = TablesTab::Views;
    }
    if options.remembered.saved_tab.as_deref() == Some("history") {
        model.saved_tab = SavedTab::History;
    }
    model.remembered = options.remembered.clone();
    crate::state::query::restore(
        &mut model,
        &options.remembered.query_tabs,
        options.remembered.query_active,
    );
    model.page_size = crate::state::browse::page_size(options.page_size);
    let (mut runner, mut inbox) =
        Runner::new(Some(session.clone()), Some(options.data_dir.clone()));
    runner.listen();
    let mut initial = update(&mut model, Msg::Library(Ok(library)));
    initial.extend(connect::start(&mut model, start));
    match options.hooks.panic() {
        Some("main") => {
            tui.draw(|frame| view(&model, frame))?;
            panic!("{}=main", options.hooks.env_name("PANIC"));
        }
        Some("task") => {
            let name = options.hooks.env_name("PANIC");
            runner.tasks.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                panic!("{name}=task");
            });
        }
        _ => {}
    }
    log::info!("Started");
    let input = input::Input::default();
    let mut host = RealHost {
        options: terminal_options,
        data_dir: options.data_dir.clone(),
        input: input.clone(),
    };
    let exit = event_loop::run(
        &mut tui,
        &mut model,
        input,
        &mut host,
        &mut runner,
        &mut inbox,
        event_loop::LoopSignals {
            shutdown: signals,
            suspend,
        },
        initial,
    )
    .await;
    // A terminal that went away while the host had it (Ctrl+Z, `$EDITOR`)
    // ends as a closed one does.
    let exit = match exit {
        Err(e) if terminal::gone(&e) => Ok(Exit::TerminalClosed),
        other => other,
    };
    // The terminal first: closing connections can take a moment. ratatui's
    // `Drop` would `eprintln!` on a closed terminal (probe F5).
    terminal::release(tui);
    drop(guard);
    match &exit {
        Ok(exit) => log::info!("Ended: {exit:?}"),
        Err(e) => log::error!("Ended by a terminal error: {}", e.kind()),
    }
    if !matches!(exit, Ok(Exit::Crashed)) {
        crate::state::query::remember(&mut model);
        if let Err(e) = state_file::save(&options.data_dir, &model.remembered) {
            log::warn!(activity = "tui.state_file", error = format!("{:?}", e.kind()).as_str(); "Can't write the state file");
        }
    }
    // Bounded, whichever way the loop ended (`q`, a hang-up, a signal;
    // review I1): the calls in flight are dropped first (an `EXPLAIN
    // ANALYZE` of a long statement isn't a stream `close_all` cancels), and
    // closing gets at most `SETTLE_WITHIN`.
    runner.tasks.abort_all();
    if tokio::time::timeout(SETTLE_WITHIN, runner.close())
        .await
        .is_err()
    {
        log::warn!(activity = "tui.exit"; "Closing the connections timed out");
    }
    exit.map_err(AppError::Terminal)
}

/// How long [`settle`] waits at most: past the engines' 5 s cancel limit.
pub const SETTLE_WITHIN: std::time::Duration = std::time::Duration::from_secs(6);

/// Waits, at most `within`, until no task is left on the runtime: after
/// [`run_app`] that's the cancels Core spawned for statements still
/// running, which would otherwise die with the runtime.
pub async fn settle(within: std::time::Duration) {
    let start = std::time::Instant::now();
    let metrics = tokio::runtime::Handle::current().metrics();
    while metrics.num_alive_tasks() > 0 {
        if start.elapsed() >= within {
            log::warn!(
                activity = "tui.exit",
                tasks = metrics.num_alive_tasks();
                "Tasks still running at exit"
            );
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

/// Core, the library and where to start; every refusal worded for stderr.
async fn open_core(
    options: &AppOptions,
) -> Result<
    (
        core::Session,
        crate::state::panels::Library,
        crate::state::picker::Start,
    ),
    AppError,
> {
    let store = options
        .hooks
        .secret_store()
        .await
        .map_err(AppError::Startup)?;
    let session = core::open(core::OpenOptions {
        data_dir: options.data_dir.clone(),
        store,
        core: seaquel_terminal::CoreOptions::default().with_hooks(&options.hooks),
        poll: core::EXTERNAL_POLL,
        origin: options.hooks.origin().map(str::to_string),
    })
    .await
    .map_err(|e| AppError::Startup(core::startup_message(&e)))?;
    let library = match session.library().await {
        Ok(library) => library,
        Err(e) => {
            session.close().await;
            return Err(AppError::Startup(e.message));
        }
    };
    match resolve_start(
        &library,
        options.project.as_deref(),
        options.connection.as_deref(),
        &options.remembered,
    ) {
        Ok(start) => Ok((session, library, start)),
        Err(message) => {
            session.close().await;
            Err(AppError::Startup(message))
        }
    }
}

/// SIGTSTP from outside (`kill -TSTP`), as a stream the loop turns into a
/// suspend. Installed here, so it replaces the default stop: the terminal
/// is given back before the process stops.
#[cfg(unix)]
fn outside_suspend() -> Option<std::pin::Pin<Box<dyn futures::Stream<Item = ()> + Send>>> {
    use tokio::signal::unix::{signal, SignalKind};
    match signal(SignalKind::from_raw(libc::SIGTSTP)) {
        Ok(mut tstp) => Some(Box::pin(futures::stream::poll_fn(move |cx| {
            tstp.poll_recv(cx)
        }))),
        Err(e) => {
            log::warn!("Can't handle SIGTSTP: {}", e.kind());
            None
        }
    }
}

#[cfg(not(unix))]
fn outside_suspend() -> Option<std::pin::Pin<Box<dyn futures::Stream<Item = ()> + Send>>> {
    None
}

/// The real terminal's [`Host`](event_loop::Host).
struct RealHost {
    options: terminal::Options,
    data_dir: PathBuf,
    input: input::Input,
}

impl event_loop::Host<ratatui::backend::CrosstermBackend<std::io::Stdout>> for RealHost {
    fn edit(&mut self, tui: &mut terminal::Tui, text: &str) -> io::Result<Result<String, String>> {
        // The event reader stops first, or it would read the editor's keys.
        self.input.pause();
        let command = external_editor::editor_from_env();
        let result = terminal::outside(self.options, || {
            external_editor::edit_with(text, &self.data_dir, &command, true)
        })?;
        // A fresh terminal repaints everything; never `clear()` (S5).
        terminal::release(std::mem::replace(tui, terminal::new_terminal()?));
        if let Err(why) = &result {
            log::warn!(activity = "tui.external_editor", reason = why.split(':').next().unwrap_or(""); "The external editor didn't run");
        }
        Ok(result)
    }

    fn suspend(&mut self, tui: &mut terminal::Tui) -> io::Result<()> {
        #[cfg(unix)]
        {
            terminal::suspend(self.options)?;
            // A fresh terminal repaints everything; never `clear()` (S5).
            terminal::release(std::mem::replace(tui, terminal::new_terminal()?));
        }
        #[cfg(not(unix))]
        let _ = (tui, self.options);
        Ok(())
    }
}
