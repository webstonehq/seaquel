//! The `seaquel-cli` command line: argument parsing and dispatch. `main.rs`
//! only calls [`run`].
//!
//! The binary is named `seaquel-cli`, not `seaquel`: the desktop app already
//! owns `seaquel` on every platform (phase 4 plan, finding 1). It doesn't
//! check for a license; `--version` and `--help` point to the terms instead
//! (decision 14 of the design doc).
//!
//! stdout belongs to the MCP protocol under `seaquel-cli mcp`: messages and
//! logs go to stderr.

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};

mod mcp;

/// The line `--version` and `--help` print about the terms. A macro so it can
/// go into `concat!`.
macro_rules! terms_line {
    () => {
        "Free for personal use; commercial use needs a license. Terms: https://seaquel.app/terms"
    };
}

/// The terms line printed by `--version` and `--help`.
pub const TERMS_LINE: &str = terms_line!();

/// The desktop app's version, which the bundled CLI shares (see `build.rs`).
pub const VERSION: &str = env!("SEAQUEL_APP_VERSION");

/// `seaquel-cli --version`: the version, then the terms line.
const VERSION_TEXT: &str = concat!(env!("SEAQUEL_APP_VERSION"), "\n", terms_line!());

#[derive(Debug, Parser)]
#[command(
    name = "seaquel-cli",
    version = VERSION_TEXT,
    about = "Seaquel's command line, bundled with the desktop app.",
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// Runs a parsed command line and returns the process exit code.
pub fn run(cli: Cli) -> ExitCode {
    match cli.command {
        Command::Mcp(args) => mcp::run(args),
    }
}
