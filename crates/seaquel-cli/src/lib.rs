//! The `seaquel-cli` command line: argument parsing and dispatch. `main.rs`
//! only calls [`run`].
//!
//! The binary is named `seaquel-cli`, not `seaquel`: the desktop app already
//! owns `seaquel` on every platform. It doesn't
//! check for a license; `--version` and `--help` point to the terms instead
//! (decision 14 of the design doc).
//!
//! stdout belongs to the MCP protocol under `seaquel-cli mcp`: messages and
//! logs go to stderr. Under `seaquel-cli duckdb` it carries only the answer
//! (a path or a status word).

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

mod duckdb;
mod mcp;

pub use duckdb::{DuckdbArgs, DuckdbCommand, InstallArgs};

use seaquel_terminal::VERSION_TEXT;
/// The terms line and the desktop app's version, which the CLI shares
/// (`seaquel-terminal`'s `build.rs` reads it).
pub use seaquel_terminal::{LogLevel, TERMS_LINE, VERSION};

#[derive(Debug, Parser)]
#[command(
    name = "seaquel-cli",
    version = VERSION_TEXT,
    about = "Seaquel's command line, downloaded alongside the desktop app.",
    after_help = TERMS_LINE,
    subcommand_required = true,
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run an MCP server over stdio for an MCP host such as Claude Desktop or
    /// Claude Code.
    ///
    /// Only the saved connections named with --connection or --project are
    /// exposed, read-only. With neither, the server starts with none.
    Mcp(McpArgs),
    /// DuckDB support: the `seaquel-duckdb` helper DuckDB connections run
    /// in, downloaded separately for this version.
    Duckdb(DuckdbArgs),
}

#[derive(Debug, Args)]
pub struct McpArgs {
    /// Expose this saved connection, by id or exact name. Repeatable.
    #[arg(long = "connection", value_name = "NAME_OR_ID")]
    pub connections: Vec<String>,
    /// Expose every connection of this project, by id or exact name.
    /// Repeatable.
    #[arg(long = "project", value_name = "NAME_OR_ID")]
    pub projects: Vec<String>,
    /// How much to log. Logs go to stderr; stdout carries only the MCP
    /// protocol.
    #[arg(long, value_enum, default_value_t = LogLevel::Warn)]
    pub log_level: LogLevel,
}

/// Runs a parsed command line and returns the process exit code.
pub fn run(cli: Cli) -> ExitCode {
    match cli.command {
        Command::Mcp(args) => mcp::run(args),
        Command::Duckdb(args) => duckdb::run(args),
    }
}
