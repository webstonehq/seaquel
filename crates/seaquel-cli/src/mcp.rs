//! `seaquel-cli mcp`: open the desktop app's data read-only, resolve the
//! exposed connections, and serve the MCP server over stdio until stdin
//! closes or the process gets SIGINT or SIGTERM (Ctrl+C on Windows). Then
//! every connection and SSH tunnel it opened is closed.
//!
//! stdout carries only JSON-RPC. Logs, rmcp's `tracing` events and Core's
//! `log` records alike, go to stderr at `--log-level`, and so does a startup
//! failure, which exits non-zero. sqlx's `sqlx::query` target (any
//! statement slower than 1 s at WARN, with its full SQL) and
//! `sqlx::postgres::notice` (a `RAISE`'s text) are dropped at every level,
//! and so is tiberius's token stream (SQL Server errors and `PRINT` text);
//! the rest of tiberius passes at WARN at most.
//!
//! **Where it reads from.** The data dir is
//! `seaquel_storage::data_dir("app.seaquel.desktop")` (`.dev` in a debug
//! build, like the dev app), or `SEAQUEL_DATA_DIR`. Secrets come from the
//! keychain service `app.seaquel.desktop`, SSH host keys from
//! `~/.ssh/known_hosts`, which is never written.
//!
//! **Test hooks, debug builds only.** A release build ignores these, so the
//! shipped binary has no way to take secrets from a file:
//!
//! - `SEAQUEL_CLI_TEST_SECRETS`: a JSON file `{"db:<id>": "…"}` loaded into a
//!   `MemoryStore` in place of the keychain;
//! - `SEAQUEL_CLI_TEST_KNOWN_HOSTS`: the known_hosts file to check;
//! - `SEAQUEL_CLI_TEST_CALL_TIMEOUT_MS`: the per-call timeout.
//!
//! The tests set all of them (with `SEAQUEL_DATA_DIR`), so they never touch
//! the real keychain, data dir or `~/.ssh`.

use std::process::ExitCode;
use std::sync::Arc;

use rmcp::ServiceExt;
use seaquel_core::secrets::{KeychainStore, SecretStore, DESKTOP_SERVICE};
use seaquel_core::storage::{data_dir, StorageOptions};
use seaquel_core::{CoreError, WorkspaceSpec};
use seaquel_mcp::{McpServer, SecretWait, Selection, ServerOptions};
use tracing_subscriber::filter::{LevelFilter, Targets};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::{LogLevel, McpArgs, VERSION};

/// The desktop app's identifier, which names its data dir.
const APP_IDENTIFIER: &str = if cfg!(debug_assertions) {
    "app.seaquel.desktop.dev"
} else {
    "app.seaquel.desktop"
};

pub const TEST_SECRETS_ENV: &str = "SEAQUEL_CLI_TEST_SECRETS";
pub const TEST_KNOWN_HOSTS_ENV: &str = "SEAQUEL_CLI_TEST_KNOWN_HOSTS";
pub const TEST_CALL_TIMEOUT_ENV: &str = "SEAQUEL_CLI_TEST_CALL_TIMEOUT_MS";

pub fn run(args: McpArgs) -> ExitCode {
    init_logging(args.log_level);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("seaquel-cli mcp: can't start the runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = runtime.block_on(serve(args));
    // After a signal, tokio's stdin reader is still blocked in a read on a
    // blocking thread that nothing can interrupt, and dropping the runtime
    // would wait for it forever. Everything that matters is closed by now.
    runtime.shutdown_background();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("seaquel-cli mcp: {message}");
            ExitCode::FAILURE
        }
    }
}

/// sqlx's statement log target. It logs every statement at DEBUG, and each
/// one slower than a second at WARN, with its whole SQL, which may quote
/// values. The drivers turn it off; this drops it too, at every level.
const SQLX_QUERY_TARGET: &str = "sqlx::query";

/// sqlx's Postgres notice target: a `RAISE WARNING`'s text, which the query
/// chooses and can fill with values (probe M5). Dropped at every level.
const SQLX_NOTICE_TARGET: &str = "sqlx::postgres::notice";

/// tiberius's token stream: every SQL Server error (ERROR) and `PRINT`
/// (INFO) with the server's text, which can quote values. Dropped at every
/// level; the rest of tiberius passes at WARN at most (its TLS warnings).
const TIBERIUS_TOKEN_TARGET: &str = "tiberius::tds::stream::token";

/// What `--log-level` lets through: everything at `level`, except
/// `sqlx::query`, `sqlx::postgres::notice`, tiberius's token stream and
/// `sqlparser`, which log SQL or query text and never pass, and the rest of
/// tiberius, held at WARN.
pub(crate) fn log_filter(level: LogLevel) -> Targets {
    let filter = match level {
        LogLevel::Off => LevelFilter::OFF,
        LogLevel::Error => LevelFilter::ERROR,
        LogLevel::Warn => LevelFilter::WARN,
        LogLevel::Info => LevelFilter::INFO,
        LogLevel::Debug => LevelFilter::DEBUG,
        LogLevel::Trace => LevelFilter::TRACE,
    };
    Targets::new()
        .with_default(filter)
        .with_target(SQLX_QUERY_TARGET, LevelFilter::OFF)
        .with_target(SQLX_NOTICE_TARGET, LevelFilter::OFF)
        .with_target(TIBERIUS_TOKEN_TARGET, LevelFilter::OFF)
        .with_target("tiberius", filter.min(LevelFilter::WARN))
        .with_target("sqlparser", LevelFilter::OFF)
}

fn init_logging(level: LogLevel) {
    // `try_init` also installs the `log` bridge, so Core's and sqlx's records
    // land here.
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false),
        )
        .with(log_filter(level))
        .try_init();
}

/// A debug build's test hook, or `None`.
fn test_hook(name: &str) -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    } else {
        None
    }
}

async fn secret_store() -> Result<Arc<dyn SecretStore>, String> {
    let Some(path) = test_hook(TEST_SECRETS_ENV) else {
        return Ok(Arc::new(KeychainStore::new(DESKTOP_SERVICE)));
    };
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{TEST_SECRETS_ENV}: {e}"))?;
    let entries: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&text).map_err(|e| format!("{TEST_SECRETS_ENV}: {e}"))?;
    let store = seaquel_core::secrets::MemoryStore::new();
    for (key, value) in entries {
        store
            .set(&key, &value)
            .await
            .map_err(|e| format!("{TEST_SECRETS_ENV}: {e}"))?;
    }
    Ok(Arc::new(store))
}

fn startup_error(e: CoreError) -> String {
    format!("{}: {}", e.code, e.message)
}

async fn serve(args: McpArgs) -> Result<(), String> {
    let dir = data_dir(APP_IDENTIFIER).map_err(|e| startup_error(e.into()))?;
    // The MCP server connects to the user's own saved connections, as the
    // desktop app would.
    let mut builder = seaquel_core::with_default_plugins()
        .connect_policy(seaquel_core::ConnectPolicy::Unrestricted)
        .executor(Arc::new(seaquel_runtime::TokioExecutor));
    if let Some(path) = test_hook(TEST_KNOWN_HOSTS_ENV) {
        builder = builder.ssh_known_hosts(path);
    }
    let core = Arc::new(builder.build());

    // Keychain reads are timed so a pending prompt doesn't use up a call's
    // timeout.
    let secret_wait = SecretWait::new();
    let spec = WorkspaceSpec::new(&dir)
        .with_storage_options(StorageOptions {
            read_only: true,
            ..StorageOptions::default()
        })
        .with_secrets(secret_wait.watch(secret_store().await?));
    let workspace = core.open_workspace(spec).await.map_err(startup_error)?;

    let mut options = ServerOptions::default()
        .with_version(VERSION)
        .with_secret_wait(secret_wait);
    if let Some(ms) = test_hook(TEST_CALL_TIMEOUT_ENV).and_then(|v| v.parse().ok()) {
        options = options.with_call_timeout(std::time::Duration::from_millis(ms));
    }
    let selection = Selection {
        connections: args.connections,
        projects: args.projects,
    };
    let server = match McpServer::start(core, workspace.clone(), &selection, options).await {
        Ok(server) => server,
        Err(e) => {
            workspace.close().await;
            return Err(e.to_string());
        }
    };
    log::info!("Serving MCP on stdio");

    let result = match server.clone().serve(seaquel_mcp::transport::stdio()).await {
        Ok(running) => {
            // A signal ends the session the way stdin EOF does.
            let session = running.cancellation_token();
            let signals = tokio::spawn(async move {
                if let Some(signal) = shutdown_signal().await {
                    log::info!("Got {signal}, shutting down");
                    session.cancel();
                }
            });
            let ended = running
                .waiting()
                .await
                .map(|reason| log::info!("MCP session ended: {reason:?}"))
                .map_err(|e| format!("the MCP session failed: {e}"));
            signals.abort();
            ended
        }
        Err(e) => Err(format!("the MCP handshake failed: {e}")),
    };
    server.close().await;
    result
}

/// Waits for SIGINT or SIGTERM (Ctrl+C elsewhere) and names it; `None` if
/// the handlers can't be installed, in which case the default action (exit)
/// stays.
async fn shutdown_signal() -> Option<&'static str> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Can't handle SIGTERM: {e}");
                return None;
            }
        };
        let mut int = match signal(SignalKind::interrupt()) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Can't handle SIGINT: {e}");
                return None;
            }
        };
        tokio::select! {
            _ = term.recv() => Some("SIGTERM"),
            _ = int.recv() => Some("SIGINT"),
        }
    }
    #[cfg(not(unix))]
    {
        match tokio::signal::ctrl_c().await {
            Ok(()) => Some("Ctrl+C"),
            Err(e) => {
                log::warn!("Can't handle Ctrl+C: {e}");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The levels, as `LevelFilter`s (the CLI doesn't depend on `tracing`).
    struct Level;
    impl Level {
        const ERROR: LevelFilter = LevelFilter::ERROR;
        const WARN: LevelFilter = LevelFilter::WARN;
        const INFO: LevelFilter = LevelFilter::INFO;
        const DEBUG: LevelFilter = LevelFilter::DEBUG;
        const TRACE: LevelFilter = LevelFilter::TRACE;
    }

    fn enables(t: &Targets, target: &str, level: LevelFilter) -> bool {
        t.would_enable(target, &level.into_level().unwrap())
    }

    #[test]
    fn sqlx_statement_logs_never_pass() {
        for level in [
            LogLevel::Off,
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let t = log_filter(level);
            for l in [
                Level::ERROR,
                Level::WARN,
                Level::INFO,
                Level::DEBUG,
                Level::TRACE,
            ] {
                assert!(!enables(&t, "sqlx::query", l), "{level:?} {l:?}");
                assert!(!enables(&t, "sqlx::postgres::notice", l), "{level:?} {l:?}");
                assert!(
                    !enables(&t, "tiberius::tds::stream::token", l),
                    "{level:?} {l:?}"
                );
                if l > Level::WARN {
                    assert!(!enables(&t, "tiberius::client", l), "{level:?} {l:?}");
                }
            }
        }
        assert!(enables(
            &log_filter(LogLevel::Warn),
            "seaquel_mcp::transport",
            Level::WARN
        ));
        assert!(enables(
            &log_filter(LogLevel::Warn),
            "sqlx::pool",
            Level::WARN
        ));
        assert!(enables(
            &log_filter(LogLevel::Error),
            "seaquel_mcp",
            Level::ERROR
        ));
    }
}
