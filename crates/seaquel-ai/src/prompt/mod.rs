//! What the model is told: the system prompt with the schema context
//! (Decision 13), `@mention` expansion ([`mentions`]) and the chat's
//! history ([`history`]).
//!
//! The prompt is the TypeScript's (`services/ai/context.ts` before phase 6)
//! apart from what `changes.json` lists: schema-qualified table names (bug
//! 7), the schema context cut at 128 KB with a line naming what was left
//! out, and a paragraph naming the tools the model is offered.

pub mod history;
pub mod mentions;

use std::fmt::Write as _;

use seaquel_sql::SqlEngine;
use seaquel_types::SchemaTable;

use crate::sharing::Sharing;

pub use mentions::{mentions, MentionDashboard, MentionQuery, MentionWidget};

/// The engine's name as the prompt says it.
pub fn engine_label(engine: SqlEngine) -> &'static str {
    match engine {
        SqlEngine::Postgres => "PostgreSQL",
        SqlEngine::Mysql => "MySQL",
        SqlEngine::Mariadb => "MariaDB",
        SqlEngine::Sqlite => "SQLite",
        SqlEngine::Mssql => "SQL Server",
        SqlEngine::Duckdb => "DuckDB",
    }
}

/// The paragraph after the schema context, when the schema tools are offered.
pub const GUIDE_SCHEMA: &str = "Use list_schemas, list_tables and describe_table to look up schemas, tables and columns, and list_saved_queries to see the project's saved queries.";
/// The same, for the data tools.
pub const GUIDE_DATA: &str = "Use run_query to run one read-only query and read its rows as JSON (100 rows unless you pass max_rows, at most 1000), explain_query to see a query's plan, and run_saved_query to run a saved query. Anything that writes or changes the database is refused.";
/// Today's closing line.
pub const PROVIDE_CLEAR: &str = "Provide clear, concise SQL queries and explanations. When writing SQL, wrap it in a markdown code block.";
/// Today's dashboard guidelines, when the dashboard tools are offered.
pub const DASHBOARD_GUIDELINES: &str = "Dashboard creation guidelines:
- Use create_dashboard first, then add_widget for each widget.
- Layout conventions (pixel units): KPI widgets are 220×140, chart widgets are 460×340, text widgets vary. Use 20px gaps between widgets. The canvas is roughly 980px wide.
- Widget types: \"kpi\" requires kpi_config (label, valueColumn, optional format/prefix/suffix). \"chart\" requires chart_config (type, xAxis, yAxis array; chart type should match the data). \"text\" requires text_config (content).
- Chart config: xAxis is the category column, yAxis is an array of value columns, type should be \"bar\", \"line\", \"pie\", \"scatter\", or \"area\" depending on data.
- Always provide a SQL query for kpi and chart widgets. Text widgets do not need a query.
- Use get_dashboard to inspect the current state before updating or removing widgets.";

/// The schema context: each table as `Table: schema.name`, its columns and
/// its indexes, cut at whole tables.
#[derive(Clone, PartialEq, Eq)]
pub struct SchemaContext {
    /// `""` when there are no tables.
    pub text: String,
    /// Tables in `text`.
    pub kept: usize,
    /// Tables left out for the cap, named in `text`'s last line.
    pub left: usize,
}

impl std::fmt::Debug for SchemaContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SchemaContext")
            .field("bytes", &self.text.len())
            .field("kept", &self.kept)
            .field("left", &self.left)
            .finish()
    }
}

/// The schema context of `tables`, at most `max_bytes` before the closing
/// note ([`crate::limits::SCHEMA_CONTEXT_BYTES`] for a turn).
pub fn schema_context(tables: &[SchemaTable], max_bytes: usize) -> SchemaContext {
    if tables.is_empty() {
        return SchemaContext {
            text: String::new(),
            kept: 0,
            left: 0,
        };
    }
    let mut text = String::from("Database schema:");
    let mut kept = 0;
    let mut block = String::new();
    for table in tables {
        block.clear();
        let _ = write!(block, "\n\nTable: {}.{}", table.schema, table.name);
        for c in &table.columns {
            let not_null = if c.nullable { "" } else { " NOT NULL" };
            let _ = write!(block, "\n  {} {}{not_null}", c.name, c.ty);
        }
        for idx in &table.indexes {
            let _ = write!(block, "\n  INDEX {} ({})", idx.name, idx.columns.join(", "));
        }
        if text.len() + block.len() > max_bytes {
            break;
        }
        text.push_str(&block);
        kept += 1;
    }
    let left = tables.len() - kept;
    if left > 0 {
        let _ = write!(
            text,
            "\n\n({left} more tables not shown; use list_tables and describe_table to see them.)"
        );
    }
    SchemaContext { text, kept, left }
}

/// The system prompt: one line naming the engine, the schema context (left
/// out unless `sharing.schema`), the guidance for the tools a turn offers
/// (`tools`; the inline prompt offers none), following `sharing` as the
/// turn started with it, the closing line, and the dashboard guidelines
/// when the dashboard tools are offered.
pub fn system(
    engine: SqlEngine,
    schema: Option<&SchemaContext>,
    sharing: Sharing,
    tools: bool,
    dashboards: bool,
) -> String {
    let tools = tools.then_some(sharing);
    let schema = schema.filter(|_| sharing.schema);
    let label = engine_label(engine);
    let mut parts = vec![format!(
        "You are a helpful SQL assistant for a {label} database. Always use {label}-compatible syntax."
    )];
    if let Some(ctx) = schema.filter(|c| !c.text.is_empty()) {
        parts.push(ctx.text.clone());
    }
    if let Some(s) = tools {
        let guide: Vec<&str> = [(s.schema, GUIDE_SCHEMA), (s.data, GUIDE_DATA)]
            .into_iter()
            .filter_map(|(on, text)| on.then_some(text))
            .collect();
        if !guide.is_empty() {
            parts.push(guide.join(" "));
        }
    }
    parts.push(PROVIDE_CLEAR.to_string());
    if dashboards {
        parts.push(DASHBOARD_GUIDELINES.to_string());
    }
    parts.join("\n\n")
}
