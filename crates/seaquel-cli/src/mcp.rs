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
//! **Where it reads from.** The data dir is `seaquel_terminal::data_dir()`:
//! `seaquel_storage::data_dir("app.seaquel.desktop")` (`.dev` in a debug
//! build, like the dev app), or `SEAQUEL_DATA_DIR`. Secrets come from the
//! keychain service `app.seaquel.desktop`, SSH host keys from
//! `~/.ssh/known_hosts`, which is never written.
//!
//! **Test hooks, debug builds only** (`seaquel_terminal::TestHooks` under
//! `SEAQUEL_CLI_TEST`). A release build ignores these, so the shipped
//! binary has no way to take secrets from a file:
//!
//! - `SEAQUEL_CLI_TEST_SECRETS`: a JSON file `{"db:<id>": "…"}` loaded into a
//!   `MemoryStore` in place of the keychain;
//! - `SEAQUEL_CLI_TEST_KNOWN_HOSTS`: the known_hosts file to check;
//! - `SEAQUEL_CLI_TEST_CALL_TIMEOUT_MS`: the per-call timeout;
//! - `SEAQUEL_CLI_TEST_DUCKDB_HELPER`: a built `seaquel-duckdb`, linked
//!   into `<data dir>/bin/duckdb/<version>/` so DuckDB connections run in
//!   it (the CLI links no DuckDB itself);
//! - `SEAQUEL_CLI_TEST_DUCKDB_RELEASES`: `seaquel-cli duckdb install`'s
//!   release server, on 127.0.0.1 (`duckdb.rs`).
//!
//! The tests set them (with `SEAQUEL_DATA_DIR`), so they never touch
//! the real keychain, data dir or `~/.ssh`.

use std::process::ExitCode;
use std::sync::Arc;

use rmcp::ServiceExt;
use seaquel_core::secrets::SecretWait;
use seaquel_core::storage::StorageOptions;
use seaquel_core::{CoreError, WorkspaceSpec};
use seaquel_mcp::{McpServer, Selection, ServerOptions};
use seaquel_terminal::{log_filter_holding, shutdown_signal, ShutdownSignal, TestHooks};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::{LogLevel, McpArgs, VERSION};

/// The prefix of the CLI's test hooks (`SEAQUEL_CLI_TEST_SECRETS`, …).
const TEST_HOOKS_PREFIX: &str = "SEAQUEL_CLI_TEST";

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

fn init_logging(level: LogLevel) {
    // `try_init` also installs the `log` bridge, so Core's and sqlx's records
    // land here.
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false),
        )
        .with(filter(level))
        .try_init();
}

/// Held at WARN on top of the shared filter: rmcp logs each request at
/// DEBUG with its arguments, which carry SQL (phase 6's follow-up, closed
/// in phase 7a by Decision 17).
const HELD_AT_WARN: &[&str] = &["rmcp"];

/// What `--log-level` lets through.
fn filter(level: LogLevel) -> tracing_subscriber::filter::Targets {
    log_filter_holding(level, HELD_AT_WARN)
}

fn startup_error(e: CoreError) -> String {
    format!("{}: {}", e.code, e.message)
}

/// The CLI's Core, as the desktop app builds its own: every engine (DuckDB
/// through the `seaquel-duckdb` helper, the DuckDB helper plan), the
/// user's files (phase 5e, Decision 31) and the import paths. No command
/// uses the shared projection or the imports yet (Q26); the read-only
/// storage refuses their writes. `hooks` (debug builds) bring the
/// known_hosts file and a built DuckDB helper.
fn core_builder(
    import_paths: Option<seaquel_core::ImportPaths>,
    hooks: &TestHooks,
) -> seaquel_core::CoreBuilder {
    let builder = seaquel_terminal::core_builder(
        seaquel_terminal::CoreOptions {
            local_files: Some(seaquel_core::LocalFiles::Allowed),
            ..seaquel_terminal::CoreOptions::default()
        }
        .with_hooks(hooks),
    );
    match import_paths {
        Some(paths) => builder.import_paths(paths),
        None => builder,
    }
}

async fn serve(args: McpArgs) -> Result<(), String> {
    let hooks = TestHooks::from_env(TEST_HOOKS_PREFIX);
    let dir = seaquel_terminal::data_dir().map_err(startup_error)?;
    // The MCP server connects to the user's own saved connections, as the
    // desktop app would.
    let builder = core_builder(seaquel_core::ImportPaths::from_env(), &hooks);
    let core = Arc::new(builder.build());

    // Keychain reads are timed so a pending prompt doesn't use up a call's
    // timeout.
    let secret_wait = SecretWait::new();
    let spec = WorkspaceSpec::new(&dir)
        .with_storage_options(StorageOptions {
            read_only: true,
            ..StorageOptions::default()
        })
        .with_secrets(secret_wait.watch(hooks.secret_store().await?));
    let workspace = core.open_workspace(spec).await.map_err(startup_error)?;

    let mut options = ServerOptions::default()
        .with_version(VERSION)
        .with_secret_wait(secret_wait);
    if let Some(timeout) = hooks.call_timeout() {
        options = options.with_call_timeout(timeout);
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
    // A DuckDB connection without its helper fails its tools; say so once
    // here too (Decision 13). stderr only, whatever the log level.
    if let Some(notice) = server.duckdb_helper_notice() {
        eprintln!("seaquel-cli mcp: {notice}");
    }
    log::info!("Serving MCP on stdio");

    let result = match server.clone().serve(seaquel_mcp::transport::stdio()).await {
        Ok(running) => {
            // A signal ends the session the way stdin EOF does.
            let session = running.cancellation_token();
            let signals = tokio::spawn(async move {
                if let Some(signal) =
                    shutdown_signal(&[ShutdownSignal::Terminate, ShutdownSignal::Interrupt]).await
                {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 5e: the CLI's Core may read the user's files, as the
    /// desktop's does, and its imports read the home it's given.
    #[test]
    fn the_cli_core_may_read_local_files() {
        let core = core_builder(
            Some(seaquel_core::ImportPaths::new("/nowhere")),
            &TestHooks::default(),
        )
        .build();
        assert_eq!(core.local_files(), Some(seaquel_core::LocalFiles::Allowed));
    }

    // The log filter's tests moved with it to `seaquel-terminal`
    // (`logging.rs`), unchanged.

    /// rmcp's DEBUG request lines carry the tools' arguments (SQL): held at
    /// WARN at every level, while everything else keeps the level.
    #[test]
    fn rmcp_is_held_at_warn() {
        use tracing_subscriber::filter::LevelFilter;
        let enables = |level: LogLevel, target: &str, at: LevelFilter| {
            filter(level).would_enable(target, &at.into_level().unwrap())
        };
        for level in [LogLevel::Info, LogLevel::Debug, LogLevel::Trace] {
            for at in [LevelFilter::INFO, LevelFilter::DEBUG, LevelFilter::TRACE] {
                assert!(!enables(level, "rmcp", at), "{level:?} {at:?}");
                assert!(!enables(level, "rmcp::service", at), "{level:?} {at:?}");
            }
            assert!(enables(level, "rmcp", LevelFilter::WARN));
        }
        assert!(enables(LogLevel::Debug, "seaquel_mcp", LevelFilter::DEBUG));
        assert!(!enables(LogLevel::Trace, "sqlx::query", LevelFilter::WARN));
    }
}
