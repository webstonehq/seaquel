//! The tool registry (Decision 3): each tool once, with its name, its
//! description, its argument type (and so its JSON schema), its parser and
//! its renderers, for two profiles.
//!
//! - [`Profile::Mcp`] is the MCP server's surface, unchanged (Decision 20):
//!   eight tools, each taking `connection`, results up to 4 MB.
//! - [`Profile::Assistant`] is the chat panel's, bound to the chat's
//!   connection: `run_query`, `explain_query`, `list_schemas`,
//!   `list_tables`, `describe_table`, `list_saved_queries` and
//!   `run_saved_query` (Q4), then the five dashboard tools the page runs
//!   (client tools, Decision 6). Results answer what MCP answers for that
//!   connection (Decision 30), within 256 KB.
//!
//! Nothing here runs a query or reads storage: Core does, through the paths
//! it already has (Decision 4), and hands what it read to [`render`].
//!
//! **A call in the assistant profile** goes through [`prepare`]: the tool
//! must exist (and a client tool be offered), its sharing flag must be on
//! (Decision 22: before the arguments are read), its arguments must parse
//! (Decision 24: unknown fields refused, serde's message with the field's
//! path) and `max_rows` must be in range. Core then runs
//! [`read_only_check`] and asks for approval (Decision 30) before it runs
//! anything. Every refusal is a [`ToolError`] that reaches the model as an
//! error result, `CODE: message`; the wire marks it as an error itself, so
//! nothing here adds a prefix.

mod args;
pub mod format;
pub mod render;
pub mod saved;

use std::fmt;

use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use seaquel_sql::SqlEngine;
use serde::de::DeserializeOwned;
use serde_json::{json, Map, Value as Json};

pub use args::{assistant, mcp};

use crate::limits;
use crate::sharing::Sharing;
use crate::wire::ToolSpec;

/// Which surface a tool is described and rendered for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Profile {
    /// The chat panel's assistant, bound to the chat's connection.
    Assistant,
    /// The MCP server (`seaquel-cli mcp`).
    Mcp,
}

/// Every tool either profile has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    /// MCP only.
    ListConnections,
    RunQuery,
    ExplainQuery,
    ListSchemas,
    ListTables,
    DescribeTable,
    ListSavedQueries,
    RunSavedQuery,
    /// The client tools (assistant only, run by the page).
    CreateDashboard,
    AddWidget,
    GetDashboard,
    UpdateWidget,
    RemoveWidget,
}

/// What a tool needs before it may run (Decision 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    /// Schema sharing: the introspection tools and the saved-query list.
    Schema,
    /// Data sharing: the tools that run SQL.
    Data,
    /// Offered by the page (`clientTools`); the page runs it.
    Client,
    /// Nothing (`list_connections`, which reports each connection's flags).
    Nothing,
}

impl Tool {
    /// The assistant's tools, in the order the model is offered them
    /// (`changes.json`'s `*` rule 6).
    pub const ASSISTANT: [Tool; 12] = [
        Tool::RunQuery,
        Tool::ExplainQuery,
        Tool::ListSchemas,
        Tool::ListTables,
        Tool::DescribeTable,
        Tool::ListSavedQueries,
        Tool::RunSavedQuery,
        Tool::CreateDashboard,
        Tool::AddWidget,
        Tool::GetDashboard,
        Tool::UpdateWidget,
        Tool::RemoveWidget,
    ];

    /// MCP's tools, in the order `tools/list` gives them (by name).
    pub const MCP: [Tool; 8] = [
        Tool::DescribeTable,
        Tool::ExplainQuery,
        Tool::ListConnections,
        Tool::ListSavedQueries,
        Tool::ListSchemas,
        Tool::ListTables,
        Tool::RunQuery,
        Tool::RunSavedQuery,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::ListConnections => "list_connections",
            Tool::RunQuery => "run_query",
            Tool::ExplainQuery => "explain_query",
            Tool::ListSchemas => "list_schemas",
            Tool::ListTables => "list_tables",
            Tool::DescribeTable => "describe_table",
            Tool::ListSavedQueries => "list_saved_queries",
            Tool::RunSavedQuery => "run_saved_query",
            Tool::CreateDashboard => "create_dashboard",
            Tool::AddWidget => "add_widget",
            Tool::GetDashboard => "get_dashboard",
            Tool::UpdateWidget => "update_widget",
            Tool::RemoveWidget => "remove_widget",
        }
    }

    /// The tool `name` names in `profile`, if it has one.
    pub fn find(profile: Profile, name: &str) -> Option<Tool> {
        let all: &[Tool] = match profile {
            Profile::Assistant => &Tool::ASSISTANT,
            Profile::Mcp => &Tool::MCP,
        };
        all.iter().copied().find(|t| t.name() == name)
    }

    pub fn needs(self) -> Needs {
        match self {
            Tool::ListConnections => Needs::Nothing,
            Tool::RunQuery | Tool::ExplainQuery | Tool::RunSavedQuery => Needs::Data,
            Tool::ListSchemas | Tool::ListTables | Tool::DescribeTable | Tool::ListSavedQueries => {
                Needs::Schema
            }
            Tool::CreateDashboard
            | Tool::AddWidget
            | Tool::GetDashboard
            | Tool::UpdateWidget
            | Tool::RemoveWidget => Needs::Client,
        }
    }

    /// A tool the page runs (Decision 6).
    pub fn is_client(self) -> bool {
        self.needs() == Needs::Client
    }

    /// The assistant asks the user before it runs (Decision 30): the tools
    /// that run SQL.
    pub fn asks_approval(self) -> bool {
        self.needs() == Needs::Data
    }
}

/// A tool as a provider or an MCP host is told about it.
#[derive(Clone, PartialEq)]
pub struct ToolDefinition {
    pub tool: Tool,
    pub name: &'static str,
    /// MCP's `annotations.title`.
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Json,
}

impl fmt::Debug for ToolDefinition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolDefinition")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl ToolDefinition {
    /// What a round sends the provider.
    pub fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.to_string(),
            description: self.description.to_string(),
            input_schema: self.input_schema.clone(),
        }
    }

    /// The tool as MCP's `tools/list` gives it.
    pub fn mcp_json(&self) -> Json {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "title": self.title,
                "readOnlyHint": true,
                "openWorldHint": false,
            },
        })
    }
}

/// The tools `profile` offers. The assistant's follow `sharing` (no data
/// tool without data sharing, no schema tool without schema sharing) and
/// `client_tools` (the dashboard tools), in [`Tool::ASSISTANT`]'s order.
/// MCP's are always all eight: it checks sharing per call and says why.
///
/// The MCP server doesn't call this with [`Profile::Mcp`]: rmcp lists its
/// tools from the `#[tool]` methods and their [`mcp`] argument types. The
/// MCP profile is here so `tests/registry.rs` can pin the registry to
/// `tests/fixtures/tool-schemas.json`, which `seaquel-mcp`'s
/// `tests/tool_schemas.rs` compares with what the server lists.
pub fn definitions(profile: Profile, sharing: Sharing, client_tools: bool) -> Vec<ToolDefinition> {
    match profile {
        Profile::Mcp => Tool::MCP.iter().map(|t| definition(profile, *t)).collect(),
        Profile::Assistant => Tool::ASSISTANT
            .iter()
            .filter(|t| match t.needs() {
                Needs::Data => sharing.data,
                Needs::Schema => sharing.schema,
                Needs::Client => client_tools,
                Needs::Nothing => true,
            })
            .map(|t| definition(profile, *t))
            .collect(),
    }
}

/// One tool's definition in `profile`. A tool the profile doesn't have
/// (`list_connections` in the assistant, a client tool in MCP) gets the
/// other profile's schema; [`definitions`] never asks for one.
fn definition(profile: Profile, tool: Tool) -> ToolDefinition {
    let (title, description) = text(profile, tool);
    ToolDefinition {
        tool,
        name: tool.name(),
        title,
        description,
        input_schema: schema(profile, tool),
    }
}

/// A tool's MCP title and its description for `profile`. MCP's are the doc
/// comments its `#[tool]` methods had, line breaks included.
fn text(profile: Profile, tool: Tool) -> (&'static str, &'static str) {
    match (profile, tool) {
        (_, Tool::ListConnections) => (
            "List connections",
            "List the database connections this server exposes, with their\nengine, project and whether schema and data may be shared.",
        ),
        (Profile::Mcp, Tool::ListSchemas) => ("List schemas", "List the schemas of a connection."),
        (Profile::Mcp, Tool::ListTables) => (
            "List tables",
            "List the tables and views of a connection, optionally of one schema.",
        ),
        (Profile::Mcp, Tool::DescribeTable) => (
            "Describe table",
            "Describe a table: its columns (name, type, nullable, default, primary\nkey), indexes and foreign keys.",
        ),
        (Profile::Mcp, Tool::RunQuery) => (
            "Run read-only query",
            "Run one read-only SQL query (SELECT and the like) and return its rows.\nWrites are refused. At most `max_rows` rows come back (default 100, at\nmost 1000); `truncated` says the query had more.",
        ),
        (Profile::Mcp, Tool::ExplainQuery) => (
            "Explain query",
            "Show the database's query plan for a read-only query, without running\nit.",
        ),
        (Profile::Mcp, Tool::ListSavedQueries) => (
            "List saved queries",
            "List the saved queries of the exposed connections' projects, with\ntheir `{{parameters}}`.",
        ),
        (Profile::Mcp, Tool::RunSavedQuery) => (
            "Run saved query",
            "Run a saved query on a connection of its project, with values for its\n`{{parameters}}`. Read-only, like run_query.",
        ),
        (Profile::Assistant, Tool::RunQuery) => (
            "Run read-only query",
            "Run one read-only SQL query (SELECT and the like) on the connected database and return its rows as JSON: the columns once, then each row as an array. Writes are refused, and the user may be asked to approve the query first. At most `max_rows` rows come back (default 100, at most 1000); `truncated` says the query had more.",
        ),
        (Profile::Assistant, Tool::ExplainQuery) => (
            "Explain query",
            "Show the database's query plan for a read-only query, without running it.",
        ),
        (Profile::Assistant, Tool::ListSchemas) => {
            ("List schemas", "List the schemas of the connected database.")
        }
        (Profile::Assistant, Tool::ListTables) => (
            "List tables",
            "List the tables and views of the connected database, optionally of one schema.",
        ),
        (Profile::Assistant, Tool::DescribeTable) => (
            "Describe table",
            "Describe a table: its columns (name, type, nullable, default, primary key), indexes and foreign keys.",
        ),
        (Profile::Assistant, Tool::ListSavedQueries) => (
            "List saved queries",
            "List the saved queries of the connection's project, with their `{{parameters}}`.",
        ),
        (Profile::Assistant, Tool::RunSavedQuery) => (
            "Run saved query",
            "Run a saved query of the connection's project, with values for its `{{parameters}}`, and return its rows like run_query. Read-only, like run_query.",
        ),
        (_, Tool::CreateDashboard) => (
            "Create dashboard",
            "Create a new empty dashboard. Returns the dashboard ID to use when adding widgets.",
        ),
        (_, Tool::AddWidget) => (
            "Add widget",
            "Add a widget to a dashboard. Provide position (x, y), size (width, height), widget type, and the relevant config for that type.",
        ),
        (_, Tool::GetDashboard) => (
            "Get dashboard",
            "Retrieve a dashboard and all its widgets. Use this to inspect the current state before making updates.",
        ),
        (_, Tool::UpdateWidget) => (
            "Update widget",
            "Update an existing widget on a dashboard. Only the fields you provide will be changed.",
        ),
        (_, Tool::RemoveWidget) => ("Remove widget", "Remove a dashboard widget."),
    }
}

fn schema(profile: Profile, tool: Tool) -> Json {
    use args::{assistant as a, mcp as m};
    match (profile, tool) {
        // rmcp's schema for a tool without parameters.
        (_, Tool::ListConnections) => json!({ "type": "object", "properties": {} }),
        (Profile::Mcp, Tool::ListSchemas) => input_schema::<m::ConnectionArgs>(profile),
        (Profile::Mcp, Tool::ListTables) => input_schema::<m::ListTablesArgs>(profile),
        (Profile::Mcp, Tool::DescribeTable) => input_schema::<m::DescribeTableArgs>(profile),
        (Profile::Mcp, Tool::RunQuery) => input_schema::<m::RunQueryArgs>(profile),
        (Profile::Mcp, Tool::ExplainQuery) => input_schema::<m::ExplainArgs>(profile),
        (Profile::Mcp, Tool::ListSavedQueries) => input_schema::<m::ListSavedQueriesArgs>(profile),
        (Profile::Mcp, Tool::RunSavedQuery) => input_schema::<m::RunSavedQueryArgs>(profile),
        (_, Tool::RunQuery) => input_schema::<a::RunQueryArgs>(profile),
        (_, Tool::ExplainQuery) => input_schema::<a::ExplainArgs>(profile),
        (_, Tool::ListSchemas | Tool::ListSavedQueries) => input_schema::<a::NoArgs>(profile),
        (_, Tool::ListTables) => input_schema::<a::ListTablesArgs>(profile),
        (_, Tool::DescribeTable) => input_schema::<a::DescribeTableArgs>(profile),
        (_, Tool::RunSavedQuery) => input_schema::<a::RunSavedQueryArgs>(profile),
        (_, Tool::CreateDashboard) => input_schema::<a::CreateDashboardArgs>(profile),
        (_, Tool::AddWidget) => input_schema::<a::AddWidgetArgs>(profile),
        (_, Tool::GetDashboard) => input_schema::<a::GetDashboardArgs>(profile),
        (_, Tool::UpdateWidget) => input_schema::<a::UpdateWidgetArgs>(profile),
        (_, Tool::RemoveWidget) => input_schema::<a::RemoveWidgetArgs>(profile),
    }
}

/// `T`'s schema as rmcp makes a tool's `inputSchema` (`schema_for_input`):
/// draft 2020-12, the root's `title` and `description` dropped. The
/// assistant's nested types are inlined, so the model reads one schema
/// with no `$ref`s.
fn input_schema<T: JsonSchema>(profile: Profile) -> Json {
    let mut settings = SchemaSettings::draft2020_12();
    settings.inline_subschemas = profile == Profile::Assistant;
    let schema = settings.into_generator().into_root_schema_for::<T>();
    let mut value = serde_json::to_value(schema).unwrap_or_else(|_| json!({}));
    if let Json::Object(map) = &mut value {
        map.remove("title");
        map.remove("description");
        // The model has no use for the dialect line; MCP's keeps it, as
        // rmcp sends it.
        if profile == Profile::Assistant {
            map.remove("$schema");
        }
    }
    value
}

/// A refused or failed tool call: what the model or MCP host gets as an
/// error result, `CODE: message`. The message can name a connection, a
/// table or a saved query, so `Debug` shows only the code.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

impl ToolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// A tool the profile doesn't have, or a client tool the page didn't
    /// offer.
    pub fn unknown_tool(name: &str) -> Self {
        Self::new(INVALID_ARGUMENT, format!("Unknown tool: {name}"))
    }

    /// MCP's refusal when the connection doesn't share its schema.
    pub fn schema_sharing_off(connection: &str) -> Self {
        Self::new(
            SCHEMA_SHARING_OFF,
            format!(
                "The connection {connection:?} doesn't share its schema with AI tools. The user \
                 can turn schema sharing on for it in the Seaquel app (the connection's AI \
                 settings, or Settings > AI for the default)."
            ),
        )
    }

    /// MCP's refusal when the connection doesn't share its data.
    pub fn data_sharing_off(connection: &str) -> Self {
        Self::new(
            DATA_SHARING_OFF,
            format!(
                "The connection {connection:?} doesn't share data with AI tools. The user can \
                 turn data sharing on for it in the Seaquel app (the connection's AI settings, \
                 or Settings > AI for the default)."
            ),
        )
    }

    /// The user denied the query (Decision 25).
    pub fn denied() -> Self {
        Self::new(DENIED, "User denied query execution")
    }

    /// Core's read-only refusal.
    pub fn read_only() -> Self {
        Self::new(READ_ONLY, seaquel_sql::read_only::READ_ONLY_MESSAGE)
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl fmt::Debug for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolError")
            .field("code", &self.code)
            .finish_non_exhaustive()
    }
}

impl std::error::Error for ToolError {}

/// An argument no call of the tool can use, or a tool that doesn't exist.
pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
/// The connection's schema sharing is off (after the global default).
pub const SCHEMA_SHARING_OFF: &str = "SCHEMA_SHARING_OFF";
/// The connection's data sharing is off (after the global default).
pub const DATA_SHARING_OFF: &str = "DATA_SHARING_OFF";
/// The SQL isn't read-only.
pub const READ_ONLY: &str = "READ_ONLY";
/// The user denied the query.
pub const DENIED: &str = "DENIED";

/// A tool's result as the model gets it: the text, and whether it is an
/// error (the wire marks it). Its `Debug` shows the size only.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    /// A result: its JSON, compact, keys sorted.
    pub fn ok(value: &Json) -> Self {
        Self {
            text: value.to_string(),
            is_error: false,
        }
    }

    /// A refusal or failure: `CODE: message`.
    pub fn error(e: &ToolError) -> Self {
        Self {
            text: e.to_string(),
            is_error: true,
        }
    }

    pub fn from_result(result: &Result<Json, ToolError>) -> Self {
        match result {
            Ok(v) => Self::ok(v),
            Err(e) => Self::error(e),
        }
    }
}

impl fmt::Debug for ToolOutput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolOutput")
            .field("bytes", &self.text.len())
            .field("is_error", &self.is_error)
            .finish()
    }
}

/// A call's parsed arguments.
#[derive(Clone, PartialEq)]
pub enum Args {
    ListConnections,
    ListSchemas,
    ListTables {
        schema: Option<String>,
    },
    DescribeTable {
        schema: Option<String>,
        table: String,
    },
    RunQuery {
        sql: String,
        max_rows: Option<u32>,
    },
    ExplainQuery {
        sql: String,
    },
    /// MCP's `project` filter; the assistant lists the chat's project.
    ListSavedQueries {
        project: Option<String>,
    },
    RunSavedQuery {
        saved_query: String,
        params: Map<String, Json>,
        max_rows: Option<u32>,
    },
    /// A dashboard tool: checked by its type, then handed to the page as
    /// the model sent it. `query` is a widget's SQL, for Core's read-only
    /// check.
    Client {
        input: Json,
        query: Option<String>,
    },
}

/// The variant only: arguments carry SQL, names and values.
impl fmt::Debug for Args {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Args::ListConnections => "ListConnections",
            Args::ListSchemas => "ListSchemas",
            Args::ListTables { .. } => "ListTables",
            Args::DescribeTable { .. } => "DescribeTable",
            Args::RunQuery { .. } => "RunQuery",
            Args::ExplainQuery { .. } => "ExplainQuery",
            Args::ListSavedQueries { .. } => "ListSavedQueries",
            Args::RunSavedQuery { .. } => "RunSavedQuery",
            Args::Client { .. } => "Client",
        })
    }
}

/// A parsed tool call.
#[derive(Clone, PartialEq)]
pub struct Call {
    pub tool: Tool,
    /// MCP's `connection` argument; `None` in the assistant (the chat's
    /// connection) and for MCP's `list_connections`.
    pub connection: Option<String>,
    pub args: Args,
}

impl fmt::Debug for Call {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call")
            .field("tool", &self.tool.name())
            .finish_non_exhaustive()
    }
}

impl Call {
    /// The rows to fetch: `max_rows`, or 100 when the call gives none.
    /// Call [`limits::max_rows`] first to refuse one out of range.
    pub fn max_rows(&self) -> usize {
        match &self.args {
            Args::RunQuery { max_rows, .. } | Args::RunSavedQuery { max_rows, .. } => {
                limits::max_rows(*max_rows).unwrap_or(limits::DEFAULT_MAX_ROWS as usize)
            }
            _ => limits::DEFAULT_MAX_ROWS as usize,
        }
    }
}

/// Parse `input` as `name`'s arguments in `profile`.
///
/// The assistant's are strict (Decision 24): unknown fields are refused,
/// and the message is serde's for the first problem, with its path
/// (`kpi_config.format: unknown variant …`). serde_json's map visits keys
/// in sorted order (this crate must never be built with `preserve_order`;
/// a test pins it), then missing fields are reported in declaration order.
/// MCP's are parsed as rmcp parses them: unknown fields ignored.
///
/// The MCP server doesn't parse through here: rmcp parses its [`mcp`]
/// argument types, and each becomes a [`Call`] through `From` (the same
/// conversion `parse_mcp` uses). [`Profile::Mcp`] is here for the tests
/// that pin the registry's MCP surface, and for Core's tests of
/// `ai::tools::call`.
pub fn parse(profile: Profile, name: &str, input: &Json) -> Result<Call, ToolError> {
    let tool = Tool::find(profile, name).ok_or_else(|| ToolError::unknown_tool(name))?;
    match profile {
        Profile::Assistant => parse_assistant(tool, input),
        Profile::Mcp => parse_mcp(tool, input),
    }
}

fn parse_assistant(tool: Tool, input: &Json) -> Result<Call, ToolError> {
    use args::assistant as a;
    let p = Profile::Assistant;
    let args = match tool {
        Tool::RunQuery => {
            let a: a::RunQueryArgs = parse_as(p, input)?;
            Args::RunQuery {
                sql: a.sql,
                max_rows: a.max_rows,
            }
        }
        Tool::ExplainQuery => {
            let a: a::ExplainArgs = parse_as(p, input)?;
            Args::ExplainQuery { sql: a.sql }
        }
        Tool::ListSchemas => {
            parse_as::<a::NoArgs>(p, input)?;
            Args::ListSchemas
        }
        Tool::ListTables => {
            let a: a::ListTablesArgs = parse_as(p, input)?;
            Args::ListTables { schema: a.schema }
        }
        Tool::DescribeTable => {
            let a: a::DescribeTableArgs = parse_as(p, input)?;
            Args::DescribeTable {
                schema: a.schema,
                table: a.table,
            }
        }
        Tool::ListSavedQueries => {
            parse_as::<a::NoArgs>(p, input)?;
            Args::ListSavedQueries { project: None }
        }
        Tool::RunSavedQuery => {
            let a: a::RunSavedQueryArgs = parse_as(p, input)?;
            Args::RunSavedQuery {
                saved_query: a.saved_query,
                params: a.params.unwrap_or_default(),
                max_rows: a.max_rows,
            }
        }
        Tool::CreateDashboard => {
            client(input, parse_as::<a::CreateDashboardArgs>(p, input)?, |_| {
                None
            })
        }
        Tool::AddWidget => client(input, parse_as::<a::AddWidgetArgs>(p, input)?, |a| a.query),
        Tool::GetDashboard => client(input, parse_as::<a::GetDashboardArgs>(p, input)?, |_| None),
        Tool::UpdateWidget => client(input, parse_as::<a::UpdateWidgetArgs>(p, input)?, |a| {
            a.query
        }),
        Tool::RemoveWidget => client(input, parse_as::<a::RemoveWidgetArgs>(p, input)?, |_| None),
        Tool::ListConnections => return Err(ToolError::unknown_tool(tool.name())),
    };
    Ok(Call {
        tool,
        connection: None,
        args,
    })
}

/// A client tool's call: its input as the model sent it (checked), and a
/// widget's query.
fn client<T>(input: &Json, parsed: T, query: impl FnOnce(T) -> Option<String>) -> Args {
    Args::Client {
        input: input.clone(),
        query: query(parsed),
    }
}

fn parse_mcp(tool: Tool, input: &Json) -> Result<Call, ToolError> {
    use args::mcp as m;
    let p = Profile::Mcp;
    Ok(match tool {
        Tool::ListConnections => Call {
            tool,
            connection: None,
            args: Args::ListConnections,
        },
        Tool::ListSchemas => parse_as::<m::ConnectionArgs>(p, input)?.into(),
        Tool::ListTables => parse_as::<m::ListTablesArgs>(p, input)?.into(),
        Tool::DescribeTable => parse_as::<m::DescribeTableArgs>(p, input)?.into(),
        Tool::RunQuery => parse_as::<m::RunQueryArgs>(p, input)?.into(),
        Tool::ExplainQuery => parse_as::<m::ExplainArgs>(p, input)?.into(),
        Tool::ListSavedQueries => parse_as::<m::ListSavedQueriesArgs>(p, input)?.into(),
        Tool::RunSavedQuery => parse_as::<m::RunSavedQueryArgs>(p, input)?.into(),
        _ => return Err(ToolError::unknown_tool(tool.name())),
    })
}

/// MCP's calls from the arguments rmcp parsed (the server's `#[tool]`
/// methods take these types as their `Parameters`).
mod mcp_calls {
    use super::args::mcp as m;
    use super::{Args, Call, Tool};

    /// `list_schemas`' arguments: the connection alone.
    impl From<m::ConnectionArgs> for Call {
        fn from(a: m::ConnectionArgs) -> Self {
            Call {
                tool: Tool::ListSchemas,
                connection: Some(a.connection),
                args: Args::ListSchemas,
            }
        }
    }

    impl From<m::ListTablesArgs> for Call {
        fn from(a: m::ListTablesArgs) -> Self {
            Call {
                tool: Tool::ListTables,
                connection: Some(a.connection),
                args: Args::ListTables { schema: a.schema },
            }
        }
    }

    impl From<m::DescribeTableArgs> for Call {
        fn from(a: m::DescribeTableArgs) -> Self {
            Call {
                tool: Tool::DescribeTable,
                connection: Some(a.connection),
                args: Args::DescribeTable {
                    schema: a.schema,
                    table: a.table,
                },
            }
        }
    }

    impl From<m::RunQueryArgs> for Call {
        fn from(a: m::RunQueryArgs) -> Self {
            Call {
                tool: Tool::RunQuery,
                connection: Some(a.connection),
                args: Args::RunQuery {
                    sql: a.sql,
                    max_rows: a.max_rows,
                },
            }
        }
    }

    impl From<m::ExplainArgs> for Call {
        fn from(a: m::ExplainArgs) -> Self {
            Call {
                tool: Tool::ExplainQuery,
                connection: Some(a.connection),
                args: Args::ExplainQuery { sql: a.sql },
            }
        }
    }

    impl From<m::ListSavedQueriesArgs> for Call {
        fn from(a: m::ListSavedQueriesArgs) -> Self {
            Call {
                tool: Tool::ListSavedQueries,
                connection: a.connection,
                args: Args::ListSavedQueries { project: a.project },
            }
        }
    }

    impl From<m::RunSavedQueryArgs> for Call {
        fn from(a: m::RunSavedQueryArgs) -> Self {
            Call {
                tool: Tool::RunSavedQuery,
                connection: Some(a.connection),
                args: Args::RunSavedQuery {
                    saved_query: a.saved_query,
                    params: a.params.unwrap_or_default(),
                    max_rows: a.max_rows,
                },
            }
        }
    }
}

/// What [`prepare`] checks a call against: the sharing flags as Core read
/// them just before this call, whether the page offered the client tools,
/// and the connection's name for the sharing refusals.
#[derive(Clone, Copy)]
pub struct Gate<'a> {
    pub sharing: Sharing,
    pub client_tools: bool,
    pub connection_name: &'a str,
}

impl fmt::Debug for Gate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gate")
            .field("sharing", &self.sharing)
            .field("client_tools", &self.client_tools)
            .finish_non_exhaustive()
    }
}

/// An assistant call's checks before Core does anything with it, in order:
/// the tool exists (`INVALID_ARGUMENT: Unknown tool: …`), its sharing flag
/// is on (`SCHEMA_SHARING_OFF`/`DATA_SHARING_OFF`, before the arguments are
/// read, Decision 22), its arguments parse ([`parse`]) and `max_rows` is in
/// range.
pub fn prepare(name: &str, input: &Json, gate: &Gate<'_>) -> Result<Call, ToolError> {
    let tool = Tool::find(Profile::Assistant, name)
        .filter(|t| !t.is_client() || gate.client_tools)
        .ok_or_else(|| ToolError::unknown_tool(name))?;
    match tool.needs() {
        Needs::Data if !gate.sharing.data => {
            return Err(ToolError::data_sharing_off(gate.connection_name))
        }
        Needs::Schema if !gate.sharing.schema => {
            return Err(ToolError::schema_sharing_off(gate.connection_name))
        }
        _ => {}
    }
    let call = parse_assistant(tool, input)?;
    if let Args::RunQuery { max_rows, .. } | Args::RunSavedQuery { max_rows, .. } = &call.args {
        limits::max_rows(*max_rows)?;
    }
    Ok(call)
}

/// Core's read-only check of the SQL a call carries (Decision 4: the same
/// token check `query_stream` runs): `run_query`'s and `explain_query`'s
/// `sql`, and a widget's non-blank `query`. `run_saved_query`'s SQL is
/// checked when it runs.
pub fn read_only_check(call: &Call, engine: SqlEngine) -> Result<(), ToolError> {
    let sql = match &call.args {
        Args::RunQuery { sql, .. } | Args::ExplainQuery { sql } => Some(sql.as_str()),
        Args::Client { query: Some(q), .. } if !q.trim_matches(saved::is_js_space).is_empty() => {
            Some(q.as_str())
        }
        _ => None,
    };
    sql.map_or(Ok(()), |sql| read_only_sql(sql, engine))
}

/// Core's read-only check of SQL Core is about to run for a call that
/// carries none of its own: a saved query's, after its parameters are
/// substituted and before the approval card shows it.
pub fn read_only_sql(sql: &str, engine: SqlEngine) -> Result<(), ToolError> {
    match seaquel_sql::read_only::read_only_error(sql, engine) {
        Some(message) => Err(ToolError::new(READ_ONLY, message)),
        None => Ok(()),
    }
}

/// Deserialize a tool's arguments. The assistant's errors carry the path
/// `serde_path_to_error` prints before serde's message, except for the root
/// object's own problems (a missing or unknown field); MCP's are serde's
/// message alone.
fn parse_as<T: DeserializeOwned>(profile: Profile, input: &Json) -> Result<T, ToolError> {
    match profile {
        Profile::Mcp => {
            T::deserialize(input).map_err(|e| ToolError::new(INVALID_ARGUMENT, e.to_string()))
        }
        Profile::Assistant => serde_path_to_error::deserialize(input).map_err(|e| {
            let message = e.inner().to_string();
            let mut segments: Vec<&serde_path_to_error::Segment> = e.path().iter().collect();
            // serde_path_to_error puts an unknown field's own key on the path;
            // the problem is its object's, as with a missing field.
            if message.starts_with("unknown field") {
                segments.pop();
            }
            let path = path_text(&segments);
            ToolError::new(
                INVALID_ARGUMENT,
                if path.is_empty() {
                    message
                } else {
                    format!("{path}: {message}")
                },
            )
        }),
    }
}

/// A path as `serde_path_to_error` prints it (`a.b[1].c`), `""` for the
/// root.
fn path_text(segments: &[&serde_path_to_error::Segment]) -> String {
    let mut out = String::new();
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 && !matches!(segment, serde_path_to_error::Segment::Seq { .. }) {
            out.push('.');
        }
        out.push_str(&segment.to_string());
    }
    out
}
