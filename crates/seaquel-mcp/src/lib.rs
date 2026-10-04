//! Seaquel's MCP server, served by `seaquel-cli mcp` over stdio.
//!
//! It lets an MCP host (Claude Desktop, Claude Code, …) list the connections
//! the user exposed on the command line, read their schemas and run read-only
//! queries and saved queries on them, through the same Core, storage,
//! keychain and SSH code as the app. See Decisions 5 and 7 in
//! `docs/plans/2026-09-30-rust-core-phase-4-plan.md`.
//!
//! stdout belongs to the MCP protocol: nothing here may print to it, and logs
//! go to stderr.
//!
//! ## Tools
//!
//! `list_connections`, `list_schemas`, `list_tables`, `describe_table`,
//! `run_query`, `explain_query`, `list_saved_queries` and `run_saved_query`,
//! all marked read-only. Their arguments, schemas, renderers and runner are
//! Core's tool registry (`seaquel_core::ai::tools`, `Profile::Mcp`), shared
//! with the in-app assistant; this crate keeps the exposed set, the sharing
//! check, connecting on first use and the timeout (`tools/mod.rs`).
//!
//! - **The exposed set.** Only the connections named on the command line
//!   (`--connection`, `--project`), resolved once at startup by id, else by
//!   exact, case-sensitive name ([`exposed::resolve`]). Tools resolve their
//!   `connection` argument against that set only.
//! - **Sharing.** A connection whose AI schema or data sharing is off (after
//!   the global default in the app's AI settings) refuses the schema or data
//!   tools, with `SCHEMA_SHARING_OFF` or `DATA_SHARING_OFF`.
//! - **Connections** open on first use through `Workspace::connect` with a
//!   saved target, `HostKeyPolicy::KnownOnly` and `restricted` (DuckDB: no
//!   files beyond its database, no extension installs or loads,
//!   configuration locked), and stay open until [`McpServer::close`]. The
//!   workspace owns them, and every query goes through it.
//! - **Reads only.** Queries run through Core's read-only path with a row
//!   limit (default 100, at most 1000), and every call has a timeout (60 s,
//!   not counting a pending keychain prompt: [`SecretWait`]), after which its
//!   query is cancelled.
//! - **Size.** A cell's text is cut at 64 KB
//!   (`seaquel_core::ai::tools::format::MAX_CELL_BYTES`,
//!   marked as a `{"truncated": true, …}` object) and a result stops taking
//!   rows before its JSON passes about 4 MB, with `truncated` and a message.
//! - **Transport.** [`transport::stdio`] rather than rmcp's, so a line that
//!   isn't JSON gets a `-32700` parse error and a warning instead of silence.
//! - **Errors** are tool results with `isError: true` and the text
//!   `CODE: message`, with Core's code and message.

pub mod error;
pub mod exposed;
mod server;
mod tools;
pub mod transport;

pub use error::ToolError;
pub use exposed::{Exposed, Selection, Sharing};
/// The keychain wait the call timeout leaves out; it lives in
/// `seaquel-secrets` since phase 7a (Decision 7), where the TUI uses it too.
pub use seaquel_core::secrets::SecretWait;
pub use server::{McpServer, ServerOptions, DEFAULT_CALL_TIMEOUT, INSTRUCTIONS};
pub use tools::NO_CONNECTIONS_HINT;
