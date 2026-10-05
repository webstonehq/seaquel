//! A read command's Core and the app's data, opened read-only (as `mcp`
//! does), and closed again; the runtime it runs on and the signals that
//! stop it.
//!
//! Storage is opened read-only, so a command never writes `seaquel.db`, and
//! a file the app hasn't upgraded yet (`STORAGE_NEEDS_UPGRADE`) or hasn't
//! made (`STORAGE_NOT_FOUND`) is refused with a hint to open the app once.
//! Connections it opens carry no write origin: they belong to no window,
//! like the MCP server's.

use std::future::Future;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use seaquel_core::secrets::SecretWait;
use seaquel_core::storage::StorageOptions;
use seaquel_core::{Core, CoreError, Workspace, WorkspaceSpec};
use seaquel_terminal::{shutdown_signal, CoreOptions, ShutdownSignal, TestHooks};
use tokio::task::JoinHandle;

/// The prefix of the CLI's test hooks (`SEAQUEL_CLI_TEST_SECRETS`, …).
pub const TEST_HOOKS_PREFIX: &str = "SEAQUEL_CLI_TEST";

/// How long closing connections may take after a command or a signal.
pub const CLOSE_WAIT: Duration = Duration::from_secs(5);

/// How long the runtime's tasks get after the command: a Postgres or MySQL
/// cancel Core spawned for a dropped stream still goes out.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

/// How long a keychain call may be pending before the notice shows.
const KEYCHAIN_NOTICE_AFTER: Duration = Duration::from_millis(250);

const KEYCHAIN_NOTICE: &str = "Waiting for the system keychain (macOS may ask behind this window)…";

/// The codes [`Session::open`] answers when the app hasn't opened its data
/// yet, or not since an upgrade.
const OPEN_THE_APP_CODES: &[&str] = &["STORAGE_NEEDS_UPGRADE", "STORAGE_NOT_FOUND"];

const OPEN_THE_APP: &str = "Open the Seaquel app once, then run this again.";

pub struct Session {
    pub core: Arc<Core>,
    pub ws: Arc<Workspace>,
    /// Prints [`KEYCHAIN_NOTICE`] once; aborted with the session.
    notice: JoinHandle<()>,
}

impl Session {
    /// The CLI's Core and the app's data, read-only, with the keychain (or,
    /// in a debug build, the test hooks' secrets) behind a [`SecretWait`].
    pub async fn open() -> Result<Self, CoreError> {
        let hooks = TestHooks::from_env(TEST_HOOKS_PREFIX);
        let dir = seaquel_terminal::data_dir()?;
        let core = Arc::new(
            seaquel_terminal::core_builder(CoreOptions::default().with_hooks(&hooks)).build(),
        );
        let store = hooks
            .secret_store()
            .await
            .map_err(|message| CoreError::new("SECRET_STORE_ERROR", message))?;
        let secret_wait = SecretWait::new();
        let spec = WorkspaceSpec::new(&dir)
            .with_storage_options(StorageOptions {
                read_only: true,
                ..StorageOptions::default()
            })
            .with_secrets(secret_wait.watch(store));
        let ws = core.open_workspace(spec).await?;
        let notice = tokio::spawn(keychain_notice(secret_wait.clone()));
        Ok(Self { core, ws, notice })
    }

    /// Close every connection this command opened, within [`CLOSE_WAIT`],
    /// then storage.
    pub async fn close(self) {
        let _ = tokio::time::timeout(CLOSE_WAIT, self.ws.close_all(&self.core)).await;
        self.ws.close().await;
    }
}

#[cfg(test)]
impl Session {
    /// A session over a Core and workspace a test built.
    pub fn for_test(core: Arc<Core>, ws: Arc<Workspace>) -> Self {
        Self {
            core,
            ws,
            notice: tokio::spawn(async {}),
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.notice.abort();
    }
}

/// Prints [`KEYCHAIN_NOTICE`] to stderr once a keychain call has been
/// pending for [`KEYCHAIN_NOTICE_AFTER`], then ends.
async fn keychain_notice(wait: Arc<SecretWait>) {
    let mut changed = wait.changed();
    loop {
        if changed.wait_for(|pending| *pending).await.is_err() {
            return;
        }
        let since = wait.pending_for().unwrap_or_default();
        if since < KEYCHAIN_NOTICE_AFTER {
            tokio::select! {
                () = tokio::time::sleep(KEYCHAIN_NOTICE_AFTER - since) => {}
                ended = changed.wait_for(|pending| !*pending) => {
                    if ended.is_err() {
                        return;
                    }
                    continue;
                }
            }
        }
        // A call that ended during the sleep and one that started since
        // don't count; the loop waits for the next.
        if wait
            .pending_for()
            .is_some_and(|d| d >= KEYCHAIN_NOTICE_AFTER)
        {
            say(KEYCHAIN_NOTICE);
            return;
        }
    }
}

/// Prints `seaquel-cli <command>: CODE: message` to stderr, with the hint
/// to open the app for a file it hasn't made or upgraded yet. Exit code 1.
pub fn fail(command: &str, e: &CoreError) -> ExitCode {
    say(&error_text(command, e));
    ExitCode::FAILURE
}

/// Writes `line` and a newline to stderr. Unlike `eprintln!`, a closed
/// stderr isn't a panic: the line is dropped.
pub fn say(line: &str) {
    use std::io::Write;
    let mut err = std::io::stderr().lock();
    let _ = writeln!(err, "{line}");
}

/// Writes `text` to stderr as it is (its own newlines), as [`say`] does.
pub fn say_text(text: &str) {
    use std::io::Write;
    let mut err = std::io::stderr().lock();
    let _ = err.write_all(text.as_bytes());
}

fn error_text(command: &str, e: &CoreError) -> String {
    let mut text = format!("seaquel-cli {command}: {}: {}", e.code, e.message);
    if OPEN_THE_APP_CODES.contains(&e.code.as_str()) {
        text.push('\n');
        text.push_str(OPEN_THE_APP);
    }
    text
}

/// A multi-threaded runtime, `f` on it, then a bounded shutdown
/// ([`SHUTDOWN_WAIT`]) so a cancel Core spawned can still go out.
pub fn block_on<F: Future<Output = ExitCode>>(command: &str, f: F) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            say(&format!(
                "seaquel-cli {command}: can't start the runtime: {e}"
            ));
            return ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(f);
    runtime.shutdown_timeout(SHUTDOWN_WAIT);
    code
}

/// The exit code of a command stopped by SIGINT or SIGTERM.
const STOPPED_EXIT: u8 = 130;

/// Runs `f` on the session unless SIGINT or SIGTERM arrives first, then
/// closes the session ([`close_unless_stopped`]). A stopped `f` is
/// dropped, which drops whatever it was waiting on (a connect, a call, a
/// run's stream); `seaquel-cli <command>: stopped` goes to stderr and the
/// answer is `Err` with exit code 130.
pub async fn until_stopped<T>(
    command: &str,
    s: Session,
    f: impl AsyncFnOnce(&Session) -> T,
) -> Result<T, ExitCode> {
    let result = tokio::select! {
        r = f(&s) => Some(r),
        () = stopped() => None,
    };
    close_unless_stopped(s).await;
    result.ok_or_else(|| {
        say(&format!("seaquel-cli {command}: stopped"));
        ExitCode::from(STOPPED_EXIT)
    })
}

/// [`Session::close`], unless SIGINT or SIGTERM arrives first: a second
/// Ctrl+C after a command was stopped quits at once rather than waiting
/// for connections to close. (Once a command listens for signals, their
/// default action is gone, so without this the second would be ignored.)
async fn close_unless_stopped(s: Session) {
    tokio::select! {
        () = s.close() => {}
        () = stopped() => {}
    }
}

/// Resolves when SIGINT or SIGTERM (Ctrl+C on Windows) arrives; never when
/// no handler could be installed (the default action stays).
pub async fn stopped() {
    if shutdown_signal(&[ShutdownSignal::Interrupt, ShutdownSignal::Terminate])
        .await
        .is_none()
    {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use seaquel_core::secrets::{MemoryStore, SecretError, SecretStore};

    use super::*;

    #[test]
    fn a_file_the_app_has_not_opened_says_to_open_it() {
        for code in OPEN_THE_APP_CODES {
            let text = error_text("conn", &CoreError::new(*code, "the message"));
            assert_eq!(
                text,
                format!("seaquel-cli conn: {code}: the message\n{OPEN_THE_APP}")
            );
        }
        let text = error_text("conn", &CoreError::new("STORAGE_CORRUPT", "bad"));
        assert_eq!(text, "seaquel-cli conn: STORAGE_CORRUPT: bad");
    }

    /// A keychain dialog nobody answers: every call waits forever.
    struct Unanswered;

    #[seaquel_runtime::async_trait]
    impl SecretStore for Unanswered {
        async fn get(&self, _: &str) -> Result<Option<String>, SecretError> {
            std::future::pending().await
        }
        async fn set(&self, _: &str, _: &str) -> Result<(), SecretError> {
            std::future::pending().await
        }
        async fn delete(&self, _: &str) -> Result<(), SecretError> {
            std::future::pending().await
        }
    }

    /// Quick calls leave the notice waiting; one pending past the
    /// threshold prints it, and the task ends.
    #[tokio::test]
    async fn the_keychain_notice_waits_for_a_slow_call() {
        let wait = SecretWait::new();
        let task = tokio::spawn(keychain_notice(wait.clone()));

        let quick = wait.watch(Arc::new(MemoryStore::default()));
        for _ in 0..3 {
            let _ = quick.get("db:x").await;
        }
        tokio::time::sleep(KEYCHAIN_NOTICE_AFTER * 2).await;
        assert!(!task.is_finished());

        let slow = wait.watch(Arc::new(Unanswered));
        let call = tokio::spawn(async move { slow.get("db:x").await });
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the notice after a slow call")
            .expect("the notice task");
        call.abort();
    }
}
