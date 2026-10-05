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
//! (a path or a status word), and under the read commands (`conn`, `schema`,
//! `saved`, `query`) only results: rows, lists, a saved query's SQL, `ok`.
//! Prompts, footers, notices and errors go to stderr.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{ArgGroup, Args, Parser, Subcommand};

mod conn;
mod connect;
mod duckdb;
mod mcp;
mod output;
mod prompt;
mod query;
mod resolve;
mod saved;
mod schema;
mod session;

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
    /// Saved connections: list them, or check that one connects.
    Conn(ConnArgs),
    /// A connection's tables and views, or one table's columns.
    Schema(SchemaArgs),
    /// Saved queries: list them, or print one's SQL.
    Saved(SavedArgs),
    /// Run SQL on a saved connection and print the results.
    ///
    /// The SQL runs as typed, not read-only. A destructive statement (DROP,
    /// TRUNCATE, a DELETE or UPDATE without WHERE, …) is listed and asked
    /// about in a terminal; anywhere else it needs --yes. Runs aren't added
    /// to the app's history.
    ///
    /// The SQL comes from exactly one of: the SQL argument, --file, --saved,
    /// or stdin when it isn't a terminal.
    Query(QueryArgs),
}

/// `--format`'s values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FormatArg {
    Table,
    Json,
}

#[derive(Debug, Args)]
pub struct OutputArgs {
    /// table (the default on a terminal) or json (the default otherwise).
    #[arg(long, value_enum)]
    pub format: Option<FormatArg>,
}

#[derive(Debug, Args)]
pub struct InputArgs {
    /// Never ask for anything: a missing password, an unknown SSH host key
    /// or a destructive statement fails instead.
    #[arg(long)]
    pub no_input: bool,
}

#[derive(Debug, Args)]
pub struct ConnArgs {
    #[command(subcommand)]
    pub command: ConnCommand,
}

#[derive(Debug, Subcommand)]
pub enum ConnCommand {
    /// List the saved connections, of every project or of one.
    List {
        /// Only this project's connections, by id or exact name.
        #[arg(long, value_name = "NAME_OR_ID")]
        project: Option<String>,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Connect to a saved connection and disconnect again. Prints `ok`.
    Test {
        /// The saved connection, by id or exact name.
        connection: String,
        #[command(flatten)]
        input: InputArgs,
    },
}

#[derive(Debug, Args)]
pub struct SchemaArgs {
    /// The saved connection, by id or exact name.
    pub connection: String,
    /// One table or view (`schema.table`, or its name alone): list its
    /// columns instead.
    pub table: Option<String>,
    #[command(flatten)]
    pub output: OutputArgs,
    #[command(flatten)]
    pub input: InputArgs,
}

#[derive(Debug, Args)]
pub struct SavedArgs {
    #[command(subcommand)]
    pub command: SavedCommand,
}

#[derive(Debug, Subcommand)]
pub enum SavedCommand {
    /// List the saved queries, of every project or of one.
    List {
        /// Only this project's saved queries, by id or exact name.
        #[arg(long, value_name = "NAME_OR_ID")]
        project: Option<String>,
        #[command(flatten)]
        output: OutputArgs,
    },
    /// Print a saved query's SQL.
    Show {
        /// The saved query, by id or exact name.
        query: String,
        /// Look the name up in this project, by id or exact name.
        #[arg(long, value_name = "NAME_OR_ID")]
        project: Option<String>,
    },
}

/// The largest `--limit`: Core's page cap at its default
/// (`max_query_rows() - 1`).
const MAX_LIMIT: i64 = 99_999;

#[derive(Debug, Args)]
#[command(group(ArgGroup::new("source").args(["sql", "file", "saved"]).multiple(false)))]
pub struct QueryArgs {
    /// The saved connection to run on, by id or exact name.
    #[arg(short, long, value_name = "NAME_OR_ID")]
    pub connection: String,
    /// The SQL to run. One or more statements.
    // It may start with `-` (a `-- comment`, `-1`): taken as the SQL, not
    // a flag. Flags after it still parse as flags.
    #[arg(allow_hyphen_values = true)]
    pub sql: Option<String>,
    /// Read the SQL from this file (`-` for stdin).
    #[arg(short, long, value_name = "FILE")]
    pub file: Option<PathBuf>,
    /// Run this saved query, by id or exact name.
    #[arg(long, value_name = "NAME_OR_ID")]
    pub saved: Option<String>,
    /// Look --saved up in this project, by id or exact name.
    // clap drops `requires` when the required arg conflicts with one that
    // is there (`SQL --project P`), so the conflicts are named too.
    #[arg(long, value_name = "NAME_OR_ID", requires = "saved", conflicts_with_all = ["sql", "file"])]
    pub project: Option<String>,
    /// Bind `{{NAME}}` to VALUE, as text. Repeatable; every parameter the
    /// SQL uses needs one.
    #[arg(long = "param", value_name = "NAME=VALUE", value_parser = query::parse_param)]
    pub params: Vec<(String, String)>,
    /// Show at most this many rows of each SELECT (up to 99999), with the
    /// total count. 0 streams every row.
    // The bound is Core's page cap, `max_query_rows() - 1` (a page reads
    // one row more), at its default of 100,000 rows. SEAQUEL_MAX_QUERY_ROWS
    // can change that cap when Core starts: a lowered one is enforced by
    // Core's own INVALID_ARGUMENT, after connecting, and a raised one isn't
    // reachable from here (--limit 0 still streams everything).
    #[arg(
        long,
        value_name = "N",
        default_value_t = 1000,
        value_parser = clap::value_parser!(u32).range(0..=MAX_LIMIT)
    )]
    pub limit: u32,
    /// Run destructive statements without asking.
    #[arg(long)]
    pub yes: bool,
    #[command(flatten)]
    pub output: OutputArgs,
    #[command(flatten)]
    pub input: InputArgs,
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
        Command::Conn(args) => conn::run(args),
        Command::Schema(args) => schema::run(args),
        Command::Saved(args) => saved::run(args),
        Command::Query(args) => query::run(args),
    }
}
