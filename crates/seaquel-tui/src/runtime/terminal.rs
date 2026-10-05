//! The terminal's lifecycle. Entering turns on raw mode, the
//! alternate screen, mouse capture (unless `--no-mouse`) and, where the
//! terminal answers the query, the kitty keyboard protocol's
//! disambiguation flag (so Ctrl+Enter is a key of its own). [`restore`]
//! undoes all of it in reverse, at most once per [`enter`].
//!
//! **Every way the process can end, and what restores the terminal:**
//!
//! - **`run` returns** (quit, an error, `?`): [`Guard`]'s `Drop`.
//! - **A panic on the main thread:** the panic hook ([`install_panic_hook`])
//!   restores first, then the previous hook prints; the guard's `Drop`
//!   while unwinding finds nothing left to do.
//! - **A panic in a spawned task** (a Core call, on a worker thread): the
//!   hook only logs it and keeps its message (Core catches some on
//!   purpose). An uncaught one ends the task: the loop sees
//!   `JoinError::is_panic`, ends with `Exit::Crashed`, the guard restores,
//!   and `lib::run` prints the message and exits non-zero.
//! - **SIGTERM, SIGHUP, SIGINT:** the loop's signal branch ends the loop;
//!   the guard restores. (Ctrl+C in raw mode is a key, not SIGINT.)
//! - **Ctrl+Z** and an outside **SIGTSTP** (Unix): [`suspend`] restores,
//!   stops the process (SIGSTOP), and on SIGCONT enters again; the loop then
//!   draws on a fresh `Terminal`, never through `Terminal::clear()` (spike
//!   its cursor query hangs where nothing answers). An outside SIGSTOP
//!   can't be caught: the terminal stays raw until SIGCONT.
//! - **`$EDITOR`** (Ctrl+O): [`outside`] restores, runs the editor, and
//!   enters again whatever the editor did (one that can't start, or exits
//!   with an error, included); the loop then draws on a fresh `Terminal`.
//!   The event reader is stopped first (`input.rs`), or it would take the
//!   editor's keys.
//! - **The terminal closes** (its window, an SSH session): the
//!   kernel sends SIGHUP and every write to it fails with EIO. Whichever
//!   the loop meets first ends it (`Exit::Signal`, or an EIO from a draw
//!   or the event reader, which [`gone`] tells apart, as
//!   `Exit::TerminalClosed`); the restore writes fail harmlessly, and Core's
//!   connections close and the state file is written as on any other
//!   exit (bounded: the tasks are aborted and closing gets at most
//!   `SETTLE_WITHIN`). Nothing that writes to the terminal or stderr may
//!   panic on the way out: stderr lines go through `crate::say`, and a
//!   `Terminal` is given up through [`release`], never ratatui's `Drop`
//!   (its `eprintln!`).
//! - **SIGKILL:** nothing can; `reset` fixes the shell.
//! - **Windows console close:** the process ends; crossterm restores on
//!   drop where it can. (Not probed.)
//!
//! Tests: `tests/terminal.rs` runs the binary under `script` for SIGTERM,
//! SIGHUP and both panics; the probe covers Ctrl+Z and real terminals.

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

/// The real terminal.
pub type Tui = Terminal<CrosstermBackend<Stdout>>;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static MOUSE: AtomicBool = AtomicBool::new(false);
static KITTY: AtomicBool = AtomicBool::new(false);
static CRASHED: AtomicBool = AtomicBool::new(false);

/// What [`enter`] turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub mouse: bool,
}

/// One step of putting the terminal back, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    PopKeyboardFlags,
    DisableBracketedPaste,
    DisableMouseCapture,
    LeaveAlternateScreen,
    ShowCursor,
    DisableRawMode,
}

/// The steps that undo an [`enter`] that turned on the kitty flags and
/// mouse capture as given: the reverse of entering.
pub fn restore_steps(kitty: bool, mouse: bool) -> Vec<Step> {
    let mut steps = Vec::with_capacity(5);
    if kitty {
        steps.push(Step::PopKeyboardFlags);
    }
    steps.push(Step::DisableBracketedPaste);
    if mouse {
        steps.push(Step::DisableMouseCapture);
    }
    steps.extend([
        Step::LeaveAlternateScreen,
        Step::ShowCursor,
        Step::DisableRawMode,
    ]);
    steps
}

/// Raw mode, the alternate screen, mouse capture, bracketed paste and the
/// kitty flags.
pub fn enter(options: Options) -> io::Result<()> {
    use crossterm::event::{
        EnableMouseCapture, KeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    };
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};

    // Each flag is set before its step, so a failure halfway is undone too.
    ACTIVE.store(true, Ordering::SeqCst);
    let entered = (|| {
        enable_raw_mode()?;
        crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
        if options.mouse {
            MOUSE.store(true, Ordering::SeqCst);
            crossterm::execute!(io::stdout(), EnableMouseCapture)?;
        }
        // A paste arrives as one event, not as keys the editor would read
        // (Enter accepting a completion, Tab completing).
        crossterm::execute!(io::stdout(), crossterm::event::EnableBracketedPaste)?;
        // The query waits up to 2 s for a terminal that doesn't answer;
        // then the flags stay off.
        if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
            KITTY.store(true, Ordering::SeqCst);
            crossterm::execute!(
                io::stdout(),
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?;
        }
        Ok(())
    })();
    if entered.is_err() {
        restore();
    }
    entered
}

/// Puts the terminal back as [`enter`] found it. Safe to call any number
/// of times, from the panic hook too.
pub fn restore() {
    use crossterm::cursor::Show;
    use crossterm::event::{DisableMouseCapture, PopKeyboardEnhancementFlags};
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};

    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let kitty = KITTY.swap(false, Ordering::SeqCst);
    let mouse = MOUSE.swap(false, Ordering::SeqCst);
    let mut out = io::stdout();
    // Every step is tried, whatever an earlier one did (a closed terminal
    // fails them all, harmlessly).
    for step in restore_steps(kitty, mouse) {
        let _ = match step {
            Step::PopKeyboardFlags => crossterm::execute!(out, PopKeyboardEnhancementFlags),
            Step::DisableBracketedPaste => {
                crossterm::execute!(out, crossterm::event::DisableBracketedPaste)
            }
            Step::DisableMouseCapture => crossterm::execute!(out, DisableMouseCapture),
            Step::LeaveAlternateScreen => crossterm::execute!(out, LeaveAlternateScreen),
            Step::ShowCursor => crossterm::execute!(out, Show),
            Step::DisableRawMode => disable_raw_mode(),
        };
    }
}

/// A fresh `Terminal` on stdout. A fresh one repaints everything on its
/// first draw, so a resume never needs `Terminal::clear()`.
pub fn new_terminal() -> io::Result<Tui> {
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

/// Gives up a `Terminal`. ratatui's `Drop` shows the cursor and, when that
/// fails, `eprintln!`s, which panics on a closed terminal (and aborts if it's already unwinding).
/// So the cursor is shown here, and a
/// terminal that can't take it is forgotten rather than dropped.
pub fn release<B: ratatui::backend::Backend>(mut terminal: Terminal<B>) {
    if terminal.show_cursor().is_err() {
        std::mem::forget(terminal);
    }
}

/// Whether `e` means the terminal is gone (closed window, dropped SSH
/// session): EIO, ENXIO or a broken pipe, on the error or the one it wraps
/// (a draw's backend error).
pub fn gone(e: &io::Error) -> bool {
    fn closed(e: &io::Error) -> bool {
        #[cfg(unix)]
        if matches!(e.raw_os_error(), Some(libc::EIO | libc::ENXIO)) {
            return true;
        }
        e.kind() == io::ErrorKind::BrokenPipe
    }
    closed(e)
        || e.get_ref()
            .and_then(|inner| inner.downcast_ref::<io::Error>())
            .is_some_and(closed)
}

/// Restores the terminal when dropped.
#[derive(Debug)]
pub struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

/// Whether the kitty keyboard flags are pushed now.
pub fn keyboard_enhanced() -> bool {
    KITTY.load(Ordering::SeqCst)
}

/// Whether a panic has happened (the loop then draws nothing more).
pub fn crashed() -> bool {
    CRASHED.load(Ordering::SeqCst)
}

/// The last panic message a worker thread kept off the screen.
static PANIC_MESSAGE: Mutex<Option<String>> = Mutex::new(None);

/// The panic hook.
///
/// - **On the thread that installed it** (the main thread, which runs the
///   loop): restore the terminal, log where it happened (not its message,
///   which may quote a value), run the previous hook, say where the log is.
///   [`crashed`] is set, so nothing more is drawn.
/// - **On any other thread** (a tokio worker running a Core call): log where
///   it happened and keep the message for later, but neither restore the
///   terminal nor set [`crashed`]. Core catches some panics on purpose
///   (MSSQL's session, DuckDB's blocking calls, a keychain `JoinError`),
///   and those must not end the UI. One that isn't caught reaches the loop
///   as a task's `JoinError::is_panic`, which ends it with `Exit::Crashed`;
///   the [`Guard`] restores and `lib::run` prints [`take_panic_message`].
///   While the screen isn't up the previous hook prints as usual.
///
/// The kept message is the latest, not the first (an earlier caught panic
/// mustn't stand in for the one that ended the TUI).
pub fn install_panic_hook(log: Option<PathBuf>) {
    let main = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        match info.location() {
            Some(at) => log::error!("Panicked at {}:{}", at.file(), at.line()),
            None => log::error!("Panicked"),
        }
        if std::thread::current().id() != main {
            if ACTIVE.load(Ordering::SeqCst) {
                if let Ok(mut kept) = PANIC_MESSAGE.lock() {
                    *kept = Some(info.to_string());
                }
            } else {
                previous(info);
            }
            return;
        }
        CRASHED.store(true, Ordering::SeqCst);
        restore();
        previous(info);
        // Never `eprintln!` here: on a closed terminal it panics inside the
        // hook, which aborts.
        crate::say(crate::state::text::crashed(log.as_deref()));
    }));
}

/// The panic a worker thread printed nothing about while the screen was up
/// (the latest), for `lib::run` to print once the terminal is restored.
pub fn take_panic_message() -> Option<String> {
    PANIC_MESSAGE.lock().ok().and_then(|mut kept| kept.take())
}

/// Tests that panic on purpose while the hook is installed hold this, so
/// one's kept message isn't another's.
#[cfg(test)]
pub(crate) static PANIC_TESTS: Mutex<()> = Mutex::new(());

/// Tests: the hook installed once for the whole test binary, with no log.
#[cfg(test)]
pub(crate) fn install_panic_hook_for_tests() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| install_panic_hook(None));
}

/// Runs `f` with the terminal given back (cooked mode, the main screen),
/// then enters again (`$EDITOR`). Entering again happens whatever `f`
/// answered.
pub fn outside<T>(options: Options, f: impl FnOnce() -> T) -> io::Result<T> {
    restore();
    let result = f();
    enter(options)?;
    Ok(result)
}

/// Ctrl+Z, or SIGTSTP from outside: gives the terminal back, stops the
/// process, and enters again once it's continued. It stops with SIGSTOP,
/// since the TUI handles SIGTSTP itself (an outside `kill -TSTP` lands in
/// the loop and comes here); the shell sees a stopped job either way.
#[cfg(unix)]
pub fn suspend(options: Options) -> io::Result<()> {
    restore();
    // SAFETY: `raise` only sends a signal to this process; SIGSTOP stops it
    // until SIGCONT, and then `raise` returns.
    unsafe {
        libc::raise(libc::SIGSTOP);
    }
    enter(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic Core catches (MSSQL's session, DuckDB's blocking calls, a
    /// keychain `JoinError`) on a worker thread is no crash: the terminal
    /// stays as it is and nothing is printed over the screen.
    #[test]
    fn a_panic_caught_on_another_thread_is_not_a_crash() {
        let _serial = PANIC_TESTS.lock().unwrap_or_else(|e| e.into_inner());
        install_panic_hook_for_tests();
        ACTIVE.store(true, Ordering::SeqCst);
        let caught =
            std::thread::spawn(|| std::panic::catch_unwind(|| panic!("caught-marker")).is_err())
                .join()
                .unwrap();
        let still_active = ACTIVE.swap(false, Ordering::SeqCst);
        assert!(caught);
        assert!(!crashed());
        assert!(still_active, "the terminal was restored");
        let message = take_panic_message().expect("the message is kept");
        assert!(message.contains("caught-marker"), "{message}");
        assert!(message.contains("panicked at"), "{message}");
    }

    #[test]
    fn a_closed_terminal_is_told_from_other_errors() {
        #[cfg(unix)]
        {
            let eio = io::Error::from_raw_os_error(libc::EIO);
            assert!(gone(&eio));
            assert!(gone(&io::Error::from_raw_os_error(libc::ENXIO)));
            // A draw's error, wrapped by the loop.
            assert!(gone(&io::Error::other(eio)));
        }
        assert!(gone(&io::Error::from(io::ErrorKind::BrokenPipe)));
        assert!(!gone(&io::Error::other("tty gone")));
        assert!(!gone(&io::Error::from(io::ErrorKind::PermissionDenied)));
    }

    #[test]
    fn restoring_undoes_entering_in_reverse() {
        use Step::*;
        assert_eq!(
            restore_steps(true, true),
            [
                PopKeyboardFlags,
                DisableBracketedPaste,
                DisableMouseCapture,
                LeaveAlternateScreen,
                ShowCursor,
                DisableRawMode
            ]
        );
        assert_eq!(
            restore_steps(false, true),
            [
                DisableBracketedPaste,
                DisableMouseCapture,
                LeaveAlternateScreen,
                ShowCursor,
                DisableRawMode
            ]
        );
        assert_eq!(
            restore_steps(true, false),
            [
                PopKeyboardFlags,
                DisableBracketedPaste,
                LeaveAlternateScreen,
                ShowCursor,
                DisableRawMode
            ]
        );
        assert_eq!(
            restore_steps(false, false),
            [
                DisableBracketedPaste,
                LeaveAlternateScreen,
                ShowCursor,
                DisableRawMode
            ]
        );
    }
}
