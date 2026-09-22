export type ExportFormat = "csv" | "json" | "sql" | "markdown";

export const formatConfig: Record<ExportFormat, { extension: string; name: string }> = {
  csv: { extension: "csv", name: "CSV" },
  json: { extension: "json", name: "JSON" },
  sql: { extension: "sql", name: "SQL" },
  markdown: { extension: "md", name: "Markdown" },
};

function escapeCSVValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  // oxlint-disable-next-line typescript/no-base-to-string
  const str = String(value);
  if (str.includes(",") || str.includes('"') || str.includes("\n")) {
    return `"${str.replace(/"/g, '""')}"`;
  }
  return str;
}

export function escapeSQLValue(value: unknown): string {
  if (value === null || value === undefined) return "NULL";
  if (typeof value === "number") return String(value);
  if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
  // oxlint-disable-next-line typescript/no-base-to-string
  const str = String(value);
  return `'${str.replace(/'/g, "''")}'`;
}

export function escapeMarkdownValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  // oxlint-disable-next-line typescript/no-base-to-string
  return String(value).replace(/\|/g, "\\|").replace(/\n/g, " ");
}

function generateCSV(columns: string[], rows: unknown[][]): string {
  const header = columns.map(escapeCSVValue).join(",");
  const dataRows = rows.map((row) => row.map((v) => escapeCSVValue(v)).join(","));
  return [header, ...dataRows].join("\n");
}

export function generateJSON(columns: string[], rows: unknown[][]): string {
  // JSON export materializes rows into `{col: value}` objects so downstream
  // consumers get the familiar shape. Only runs on user-initiated export,
  // so the allocation cost is a non-issue.
  const objects = rows.map((row) => {
    const obj: Record<string, unknown> = {};
    for (let i = 0; i < columns.length; i++) obj[columns[i]] = row[i];
    return obj;
  });
  return JSON.stringify(objects, null, 2);
}

export function generateSQL(
  columns: string[],
  rows: unknown[][],
  tableName: string = "table_name",
): string {
  if (rows.length === 0) return "";

  const columnNames = columns.join(", ");
  const inserts = rows.map((row) => {
    const values = row.map((v) => escapeSQLValue(v)).join(", ");
    return `INSERT INTO ${tableName} (${columnNames}) VALUES (${values});`;
  });

  return inserts.join("\n");
}

export function generateMarkdown(columns: string[], rows: unknown[][]): string {
  if (rows.length === 0) return "";

  const header = `| ${columns.join(" | ")} |`;
  const separator = `| ${columns.map(() => "---").join(" | ")} |`;
  const dataRows = rows.map((row) => `| ${row.map((v) => escapeMarkdownValue(v)).join(" | ")} |`);

  return [header, separator, ...dataRows].join("\n");
}

export function getExportContent(
  format: ExportFormat,
  columns: string[],
  rows: unknown[][],
  tableName?: string,
): string {
  switch (format) {
    case "csv":
      return generateCSV(columns, rows);
    case "json":
      return generateJSON(columns, rows);
    case "sql":
      return generateSQL(columns, rows, tableName);
    case "markdown":
      return generateMarkdown(columns, rows);
  }
}
