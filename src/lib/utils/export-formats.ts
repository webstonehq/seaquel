import type { DatabaseType } from "$lib/types";
import { SqlDecimal, cellText, jsonReplacer, toHex } from "$lib/values";

export type ExportFormat = "csv" | "json" | "sql" | "markdown";

export const formatConfig: Record<ExportFormat, { extension: string; name: string }> = {
  csv: { extension: "csv", name: "CSV" },
  json: { extension: "json", name: "JSON" },
  sql: { extension: "sql", name: "SQL" },
  markdown: { extension: "md", name: "Markdown" },
};

function escapeCSVValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  const str = cellText(value);
  if (str.includes(",") || str.includes('"') || str.includes("\n")) {
    return `"${str.replace(/"/g, '""')}"`;
  }
  return str;
}

const DECIMAL_LITERAL = /^-?\d+(\.\d+)?([eE][-+]?\d+)?$/;

export function escapeSQLValue(value: unknown, dbType?: DatabaseType): string {
  if (value === null || value === undefined) return "NULL";
  if (typeof value === "number" || typeof value === "bigint") return String(value);
  if (value instanceof SqlDecimal) {
    if (DECIMAL_LITERAL.test(value.value)) return value.value;
    // NaN / Infinity / -Infinity are only valid as quoted strings.
    return dbType === "postgres" ? `'${value.value}'::numeric` : `'${value.value}'`;
  }
  if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
  // Postgres needs the cast to read hex text as bytea; other engines keep a
  // plain quoted string, as before.
  if (value instanceof Uint8Array)
    return dbType === "postgres" ? `'${toHex(value)}'::bytea` : `'${toHex(value)}'`;
  const str = cellText(value);
  return `'${str.replace(/'/g, "''")}'`;
}

export function escapeMarkdownValue(value: unknown): string {
  if (value === null || value === undefined) return "";
  return cellText(value).replace(/\|/g, "\\|").replace(/\n/g, " ");
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
  return JSON.stringify(objects, jsonReplacer, 2);
}

export function generateSQL(
  columns: string[],
  rows: unknown[][],
  tableName: string = "table_name",
  dbType?: DatabaseType,
): string {
  if (rows.length === 0) return "";

  const columnNames = columns.join(", ");
  const inserts = rows.map((row) => {
    const values = row.map((v) => escapeSQLValue(v, dbType)).join(", ");
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
  dbType?: DatabaseType,
): string {
  switch (format) {
    case "csv":
      return generateCSV(columns, rows);
    case "json":
      return generateJSON(columns, rows);
    case "sql":
      return generateSQL(columns, rows, tableName, dbType);
    case "markdown":
      return generateMarkdown(columns, rows);
  }
}
