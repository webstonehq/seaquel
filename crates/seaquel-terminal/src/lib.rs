//! What the terminal binaries share (phase 7a): `seaquel-cli`
//! (the MCP server) and `seaquel-tui` both build on this crate, so the
//! policy they have in common lives once:
//!
//! - [`APP_IDENTIFIER`] and [`data_dir`]: the desktop app's data dir (`.dev`
//!   in a debug build, or `SEAQUEL_DATA_DIR`);
//! - [`core_builder`]: every engine, `ConnectPolicy::Unrestricted` and
//!   `TokioExecutor`, plus the options each binary asks for;
//! - [`TestHooks`]: the debug-only test hooks, read under each binary's own
//!   prefix (`SEAQUEL_CLI_TEST_…`, `SEAQUEL_TUI_TEST_…`);
//! - [`log_filter`] and [`LogLevel`]: the targets that never pass (SQL and
//!   query text) and the ones held at WARN;
//! - [`shutdown_signal`];
//! - [`host_key_fingerprint`] and [`destructive_reason`]: words for Core's
//!   answers;
//! - [`VERSION`], [`VERSION_TEXT`] and [`TERMS_LINE`] for `--version` and
//!   `--help` (`build.rs` reads the app's version).
//!
//! It may use Core, the runtime and the types, nothing else of ours
//! (`scripts/check-crate-deps.mjs`).

mod core;
mod hooks;
mod logging;
mod signals;
mod version;
mod words;

pub use crate::core::{core_builder, data_dir, duckdb_helper_dir, CoreOptions, APP_IDENTIFIER};
pub use hooks::TestHooks;
pub use logging::{log_filter, log_filter_holding, LogLevel};
pub use signals::{shutdown_signal, ShutdownSignal, ShutdownSignals};
pub use version::{TERMS_LINE, VERSION, VERSION_TEXT};
pub use words::{destructive_reason, host_key_fingerprint};
