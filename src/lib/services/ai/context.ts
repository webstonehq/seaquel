import type { SchemaTable } from "$lib/types";
import type { DatabaseType } from "$lib/types/database";
import { READ_ONLY_REFUSAL, validateReadOnlyQuery } from "$lib/sql";

/**
 * The AI tools' read-only check (`run_query`, and the dashboard tools'
 * widget queries): the refusal message, or `null` when the query may run. It reads the query with the connection's quoting
 * (`$lib/sql`), so without a known connection type it refuses: it fails
 * closed, as `validateReadOnlyQuery` does when the module fails or the type
 * isn't an engine it knows.
 */
export function readOnlyError(query: string, dbType: DatabaseType | undefined): string | null {
  if (!dbType) return READ_ONLY_REFUSAL;
  return validateReadOnlyQuery(query, dbType);
}

export async function runAndFormat(
  query: string,
  executeQuery: (q: string) => Promise<Record<string, unknown>[]>,
): Promise<string> {
  try {
    const rows = await executeQuery(query);
    if (rows.length === 0) return "Query returned no rows.";
    const columns = Object.keys(rows[0]);
    return buildDataContext(rows, columns);
  } catch (err) {
    return `Query error: ${err instanceof Error ? err.message : String(err)}`;
  }
}

export function buildSchemaContext(tables: SchemaTable[]): string {
  if (tables.length === 0) return "";
  const lines: string[] = ["Database schema:"];
  for (const table of tables) {
    lines.push(`\nTable: ${table.name}`);
    for (const col of table.columns) {
      const nullable = col.nullable ? "" : " NOT NULL";
      lines.push(`  ${col.name} ${col.type}${nullable}`);
    }
    if (table.indexes && table.indexes.length > 0) {
      for (const idx of table.indexes) {
        lines.push(`  INDEX ${idx.name} (${idx.columns.join(", ")})`);
      }
    }
  }
  return lines.join("\n");
}

/** A sample cell for the prompt. Bytes are summarised; the model can't use them. */
function sampleText(v: unknown): string {
  if (v === null || v === undefined) return "NULL";
  if (v instanceof Uint8Array) return `<${v.byteLength} bytes>`;
  // oxlint-disable-next-line typescript/no-base-to-string
  return String(v);
}

export function buildDataContext(rows: Record<string, unknown>[], columns: string[]): string {
  if (rows.length === 0 || columns.length === 0) return "";
  const sample = rows.slice(0, 5);
  const header = `| ${columns.join(" | ")} |`;
  const separator = `| ${columns.map(() => "---").join(" | ")} |`;
  const dataRows = sample.map((row) => `| ${columns.map((c) => sampleText(row[c])).join(" | ")} |`);
  return ["Sample data:", header, separator, ...dataRows].join("\n");
}

const DATABASE_LABELS: Record<DatabaseType, string> = {
  postgres: "PostgreSQL",
  mysql: "MySQL",
  sqlite: "SQLite",
  mariadb: "MariaDB",
  mssql: "SQL Server",
  duckdb: "DuckDB",
};

export function buildSystemPrompt(
  schemaCtx: string,
  dbType?: DatabaseType,
  hasDashboardTools?: boolean,
): string {
  const dbLabel = dbType ? DATABASE_LABELS[dbType] : "SQL";
  const parts = [
    `You are a helpful SQL assistant for a ${dbLabel} database. Always use ${dbLabel}-compatible syntax.`,
  ];
  if (schemaCtx) parts.push(schemaCtx);
  parts.push(
    "Provide clear, concise SQL queries and explanations. When writing SQL, wrap it in a markdown code block.",
  );
  if (hasDashboardTools) {
    parts.push(
      `Dashboard creation guidelines:
- Use create_dashboard first, then add_widget for each widget.
- Layout conventions (pixel units): KPI widgets are 220×140, chart widgets are 460×340, text widgets vary. Use 20px gaps between widgets. The canvas is roughly 980px wide.
- Widget types: "kpi" requires kpi_config (label, valueColumn, optional format/prefix/suffix). "chart" requires chart_config (type, xAxis, yAxis array; chart type should match the data). "text" requires text_config (content).
- Chart config: xAxis is the category column, yAxis is an array of value columns, type should be "bar", "line", "pie", "scatter", or "area" depending on data.
- Always provide a SQL query for kpi and chart widgets. Text widgets do not need a query.
- Use get_dashboard to inspect the current state before updating or removing widgets.`,
    );
  }
  return parts.join("\n\n");
}
