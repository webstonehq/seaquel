//! Each tool's arguments, once per profile. The JSON schemas the
//! model or the MCP host sees are generated from these types, so a field's
//! doc comment is its description; `tests/fixtures/tool-schemas.json` holds
//! them frozen.
//!
//! - [`mcp`] is the MCP server's surface, byte for byte: every
//!   tool takes `connection`, and unknown fields are ignored, as rmcp's
//!   `Parameters` does.
//! - [`assistant`] is bound to the chat's connection: no `connection`, and
//!   every struct refuses unknown fields, the dashboard tools'
//!   included.

/// The MCP server's arguments, the only copy: `seaquel-mcp`'s `#[tool]`
/// methods take them as their `Parameters` (rmcp parses them and lists
/// their schemas), and each becomes a [`super::Call`] through `From`. Field
/// docs, serde and schemars attributes must stay exactly as they are: they
/// are the schemas MCP hosts already see (`tool-schemas.json`'s `mcp`).
pub mod mcp {
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::{Map, Value as Json};

    #[derive(Debug, Deserialize, JsonSchema)]
    pub struct ConnectionArgs {
        /// The connection's name or id, exactly as list_connections shows it.
        pub connection: String,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    pub struct ListTablesArgs {
        /// The connection's name or id, exactly as list_connections shows it.
        pub connection: String,
        /// Only the tables of this schema (as list_schemas shows it). All
        /// schemas when omitted.
        #[serde(default)]
        pub schema: Option<String>,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    pub struct DescribeTableArgs {
        /// The connection's name or id, exactly as list_connections shows it.
        pub connection: String,
        /// The table's schema, as list_tables shows it. When omitted, the table
        /// name must be unique across schemas.
        #[serde(default)]
        pub schema: Option<String>,
        /// The table or view name, as list_tables shows it.
        pub table: String,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    pub struct RunQueryArgs {
        /// The connection's name or id, exactly as list_connections shows it.
        pub connection: String,
        /// One read-only SQL statement in the connection's dialect.
        pub sql: String,
        /// The most rows to return: 1 to 1000, default 100.
        #[serde(default)]
        #[schemars(range(min = 1, max = 1000))]
        pub max_rows: Option<u32>,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    pub struct ExplainArgs {
        /// The connection's name or id, exactly as list_connections shows it.
        pub connection: String,
        /// One read-only SQL statement to explain.
        pub sql: String,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    pub struct ListSavedQueriesArgs {
        /// Only the saved queries of this connection's project.
        #[serde(default)]
        pub connection: Option<String>,
        /// Only the saved queries of this project (name or id), which must hold
        /// an exposed connection.
        #[serde(default)]
        pub project: Option<String>,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    pub struct RunSavedQueryArgs {
        /// The connection to run it on (name or id); the saved query must belong
        /// to its project.
        pub connection: String,
        /// The saved query's id or name, as list_saved_queries shows it.
        pub saved_query: String,
        /// A value for each of the query's parameters, by name: a string, number,
        /// boolean or null. A parameter with a default may be left out; every
        /// other one is required, and names the query doesn't take are refused.
        #[serde(default)]
        pub params: Option<Map<String, Json>>,
        /// The most rows to return: 1 to 1000, default 100.
        #[serde(default)]
        #[schemars(range(min = 1, max = 1000))]
        pub max_rows: Option<u32>,
    }
}

/// The assistant's arguments: the chat's connection is implied, and unknown
/// fields are refused.
pub mod assistant {
    use std::collections::BTreeMap;

    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::{Map, Value as Json};

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct NoArgs {}

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct RunQueryArgs {
        /// One read-only SQL statement in the connection's dialect.
        pub sql: String,
        /// The most rows to return: 1 to 1000, default 100.
        #[serde(default)]
        #[schemars(range(min = 1, max = 1000))]
        pub max_rows: Option<u32>,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct ExplainArgs {
        /// One read-only SQL statement to explain.
        pub sql: String,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct ListTablesArgs {
        /// Only the tables of this schema (as list_schemas shows it). All
        /// schemas when omitted.
        #[serde(default)]
        pub schema: Option<String>,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct DescribeTableArgs {
        /// The table's schema, as list_tables shows it. When omitted, the table
        /// name must be unique across schemas.
        #[serde(default)]
        pub schema: Option<String>,
        /// The table or view name, as list_tables shows it.
        pub table: String,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct RunSavedQueryArgs {
        /// The saved query's id or name, as list_saved_queries shows it.
        pub saved_query: String,
        /// A value for each of the query's parameters, by name: a string, number,
        /// boolean or null. A parameter with a default may be left out; every
        /// other one is required, and names the query doesn't take are refused.
        #[serde(default)]
        pub params: Option<Map<String, Json>>,
        /// The most rows to return: 1 to 1000, default 100.
        #[serde(default)]
        #[schemars(range(min = 1, max = 1000))]
        pub max_rows: Option<u32>,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct CreateDashboardArgs {
        /// Name for the new dashboard
        pub name: String,
    }

    /// Type of widget
    #[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
    #[serde(rename_all = "lowercase")]
    pub enum WidgetType {
        Chart,
        Kpi,
        Text,
    }

    /// Chart type
    #[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
    #[serde(rename_all = "lowercase")]
    pub enum ChartType {
        Bar,
        Line,
        Pie,
        Scatter,
        Area,
    }

    /// How to format the value
    #[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
    #[serde(rename_all = "lowercase")]
    pub enum KpiFormat {
        Number,
        Percentage,
    }

    /// Configuration for chart widgets
    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct ChartConfig {
        /// Chart type
        #[serde(default, rename = "type")]
        pub chart_type: Option<ChartType>,
        /// Column name for the X axis
        #[serde(default, rename = "xAxis")]
        pub x_axis: Option<String>,
        /// Column names for the Y axis values
        #[serde(default, rename = "yAxis")]
        pub y_axis: Option<Vec<String>>,
        /// Custom colors per Y-axis column (column name → hex color)
        #[serde(default)]
        pub colors: Option<BTreeMap<String, String>>,
    }

    /// Configuration for KPI widgets
    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct KpiConfig {
        /// Label for the KPI value
        pub label: String,
        /// Column name containing the KPI value
        #[serde(rename = "valueColumn")]
        pub value_column: String,
        /// How to format the value
        #[serde(default)]
        pub format: Option<KpiFormat>,
        /// Prefix to display before the value (e.g. $)
        #[serde(default)]
        pub prefix: Option<String>,
        /// Suffix to display after the value (e.g. %)
        #[serde(default)]
        pub suffix: Option<String>,
    }

    /// Configuration for text widgets
    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct TextConfig {
        /// Text content to display
        pub content: String,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct AddWidgetArgs {
        /// ID of the dashboard to add the widget to
        pub dashboard_id: String,
        /// Display title for the widget
        pub title: String,
        /// X position in pixels on the canvas
        pub x: f64,
        /// Y position in pixels on the canvas
        pub y: f64,
        /// Width in pixels
        pub width: f64,
        /// Height in pixels
        pub height: f64,
        /// Type of widget
        pub widget_type: WidgetType,
        /// SQL SELECT query that powers this widget (not needed for text widgets)
        #[serde(default)]
        pub query: Option<String>,
        /// Configuration for chart widgets
        #[serde(default)]
        pub chart_config: Option<ChartConfig>,
        /// Configuration for KPI widgets
        #[serde(default)]
        pub kpi_config: Option<KpiConfig>,
        /// Configuration for text widgets
        #[serde(default)]
        pub text_config: Option<TextConfig>,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct GetDashboardArgs {
        /// ID of the dashboard to retrieve
        pub dashboard_id: String,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct UpdateWidgetArgs {
        /// ID of the dashboard containing the widget
        pub dashboard_id: String,
        /// ID of the widget to update
        pub widget_id: String,
        /// New display title
        #[serde(default)]
        pub title: Option<String>,
        /// New X position in pixels
        #[serde(default)]
        pub x: Option<f64>,
        /// New Y position in pixels
        #[serde(default)]
        pub y: Option<f64>,
        /// New width in pixels
        #[serde(default)]
        pub width: Option<f64>,
        /// New height in pixels
        #[serde(default)]
        pub height: Option<f64>,
        /// New widget type
        #[serde(default)]
        pub widget_type: Option<WidgetType>,
        /// New SQL query
        #[serde(default)]
        pub query: Option<String>,
        /// New chart configuration
        #[serde(default)]
        pub chart_config: Option<ChartConfig>,
        /// New KPI configuration
        #[serde(default)]
        pub kpi_config: Option<KpiConfig>,
        /// New text configuration
        #[serde(default)]
        pub text_config: Option<TextConfig>,
    }

    #[derive(Debug, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub struct RemoveWidgetArgs {
        /// ID of the dashboard containing the widget
        pub dashboard_id: String,
        /// ID of the widget to remove
        pub widget_id: String,
    }
}
