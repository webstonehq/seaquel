//! What a read command may ask the user: a password the keychain doesn't
//! hold, whether to trust an SSH host, whether to run a destructive
//! statement.
//!
//! The CLI asks only when it is interactive: stdin and stderr are both
//! terminals and `--no-input` isn't set. So SQL piped through stdin turns
//! prompts off, and a missing password or an unknown host key fails with a
//! message saying how to get past it instead.
//!
//! Questions go to stderr (a password prompt to the terminal itself), never
//! stdout. A typed password is a [`Zeroizing`] string and is never logged.
//!
//! The terminal is read on a thread of its own, which nothing waits for, so
//! a read command that races its work against SIGINT and SIGTERM
//! (`session::stopped`) stops at once while a prompt waits. A prompt
//! stopped that way puts the terminal's settings back (echo on, after a
//! password prompt) and ends the line.

use std::io::{BufRead, IsTerminal, Write};

use zeroize::Zeroizing;

/// What the CLI may ask. [`Prompts::Terminal`] asks on the terminal,
/// [`Prompts::None`] answers nothing; tests script the answers.
pub trait Prompter {
    /// Whether anything can be asked at all.
    fn interactive(&self) -> bool;
    /// A masked line; `None` when nothing can be asked or the user gave up.
    /// An empty line counts as given (trust auth exists).
    async fn password(&mut self, prompt: &str) -> Option<Zeroizing<String>>;
    /// `question [y/N]` on stderr; false when nothing can be asked.
    async fn confirm(&mut self, question: &str) -> bool;
}

/// The prompter a command gets.
pub enum Prompts {
    /// Ask on the terminal.
    Terminal,
    /// Not interactive, or `--no-input`: nothing is asked.
    None,
}

/// [`Prompts::Terminal`] when stdin and stderr are both terminals and
/// `--no-input` isn't set, else [`Prompts::None`].
pub fn for_command(no_input: bool) -> Prompts {
    let interactive =
        !no_input && std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if interactive {
        Prompts::Terminal
    } else {
        Prompts::None
    }
}

impl Prompter for Prompts {
    fn interactive(&self) -> bool {
        matches!(self, Prompts::Terminal)
    }

    async fn password(&mut self, prompt: &str) -> Option<Zeroizing<String>> {
        if !self.interactive() {
            return None;
        }
        let prompt = prompt.to_string();
        // rpassword turns echo off on the terminal (`/dev/tty`, the console
        // on Windows) and writes the prompt there. An I/O error is `None`.
        on_terminal(move || rpassword::prompt_password(prompt).ok().map(Zeroizing::new)).await
    }

    async fn confirm(&mut self, question: &str) -> bool {
        if !self.interactive() {
            return false;
        }
        let mut stderr = std::io::stderr().lock();
        let _ = write!(stderr, "{question} [y/N] ");
        let _ = stderr.flush();
        drop(stderr);
        let line = on_terminal(|| {
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line).ok()?;
            Some(line)
        })
        .await;
        line.is_some_and(|l| is_yes(&l))
    }
}

/// Runs `read` on a thread of its own and waits for its answer. Dropped
/// before the answer (a signal stopped the command), the terminal gets its
/// settings back and the prompt's line is ended; the thread is left to the
/// process's exit, which neither the runtime's shutdown nor anything else
/// waits for. `None` when no thread could be started.
async fn on_terminal<T: Send + 'static>(
    read: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    let mut guard = TtyGuard::save();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("seaquel-cli-prompt".into())
        .spawn(move || {
            let _ = tx.send(read());
        });
    if spawned.is_err() {
        // Nothing was asked: nothing to put back.
        guard.finished = true;
        return None;
    }
    let answer = rx.await.ok().flatten();
    guard.finished = true;
    answer
}

/// The terminal's settings when a prompt started (Unix). Dropped before
/// the prompt finished, it sets them again (`tcsetattr`, `TCSANOW`), so a
/// password prompt stopped by a signal doesn't leave echo off, and writes
/// a newline to stderr.
struct TtyGuard {
    #[cfg(unix)]
    saved: Option<(std::fs::File, libc::termios)>,
    finished: bool,
}

impl TtyGuard {
    #[cfg(unix)]
    fn save() -> Self {
        use std::os::fd::AsRawFd;
        let saved = std::fs::File::open("/dev/tty").ok().and_then(|tty| {
            let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
            // SAFETY: `tty` is an open descriptor and `termios` is written
            // in full when `tcgetattr` answers 0, the only case read.
            let read = unsafe { libc::tcgetattr(tty.as_raw_fd(), termios.as_mut_ptr()) } == 0;
            read.then(|| (tty, unsafe { termios.assume_init() }))
        });
        Self {
            saved,
            finished: false,
        }
    }

    // Windows: rpassword turns the console's echo off with
    // `SetConsoleMode`; restoring it after a stopped prompt is a follow-up.
    #[cfg(not(unix))]
    fn save() -> Self {
        Self { finished: false }
    }
}

impl Drop for TtyGuard {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        #[cfg(unix)]
        if let Some((tty, termios)) = &self.saved {
            use std::os::fd::AsRawFd;
            // SAFETY: `tty` is still open and `termios` came from
            // `tcgetattr` on the same terminal.
            unsafe {
                libc::tcsetattr(tty.as_raw_fd(), libc::TCSANOW, termios);
            }
        }
        let _ = writeln!(std::io::stderr());
    }
}

/// `y` or `yes`, in any case; anything else (an empty line included) is no.
fn is_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Answers from a script and records every question, for tests.
#[cfg(test)]
#[derive(Default)]
pub struct Scripted {
    pub passwords: std::collections::VecDeque<Option<String>>,
    pub confirms: std::collections::VecDeque<bool>,
    pub asked: Vec<String>,
    /// False: behaves as [`Prompts::None`] (asks nothing, records nothing).
    pub interactive: bool,
}

#[cfg(test)]
impl Prompter for Scripted {
    fn interactive(&self) -> bool {
        self.interactive
    }

    async fn password(&mut self, prompt: &str) -> Option<Zeroizing<String>> {
        if !self.interactive {
            return None;
        }
        self.asked.push(prompt.to_string());
        self.passwords
            .pop_front()
            .expect("an unscripted password prompt")
            .map(Zeroizing::new)
    }

    async fn confirm(&mut self, question: &str) -> bool {
        if !self.interactive {
            return false;
        }
        self.asked.push(question.to_string());
        self.confirms
            .pop_front()
            .expect("an unscripted confirmation")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_y_and_yes_are_yes() {
        for yes in ["y", "Y", "yes", "YES", " yes\n"] {
            assert!(is_yes(yes), "{yes:?}");
        }
        for no in ["", "\n", "n", "no", "yep", "ye s"] {
            assert!(!is_yes(no), "{no:?}");
        }
    }

    #[tokio::test]
    async fn a_prompt_s_answer_comes_from_its_thread() {
        assert_eq!(on_terminal(|| Some(5)).await, Some(5));
        assert_eq!(on_terminal(|| None::<u8>).await, None);
    }

    /// A stopped prompt doesn't wait for its read, which may never end.
    #[tokio::test]
    async fn a_dropped_prompt_doesnt_wait_for_its_read() {
        let (_hold, never) = std::sync::mpsc::channel::<()>();
        let read = on_terminal(move || never.recv().ok());
        let stopped = tokio::time::timeout(std::time::Duration::from_millis(50), read).await;
        assert!(stopped.is_err());
    }

    #[tokio::test]
    async fn no_prompts_asks_nothing() {
        let mut p = Prompts::None;
        assert!(!p.interactive());
        assert!(p.password("Password: ").await.is_none());
        assert!(!p.confirm("Trust this host?").await);
    }
}
