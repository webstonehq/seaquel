import type { PendingChangeOrigin } from "$lib/types/pending-changes";

/**
 * Extract a table name from SQL, handling quoted and unqualified names.
 * Matches patterns like: "schema"."table", schema.table, "table", table
 */
function extractTableName(sql: string, keyword: string): string | null {
  const pattern = new RegExp(
    `${keyword}\\s+(?:"?([a-zA-Z_][a-zA-Z0-9_]*)"?\\.)?"?([a-zA-Z_][a-zA-Z0-9_]*)"?`,
    "i",
  );
  const match = sql.match(pattern);
  if (!match) return null;
  return match[2] || null;
}

/**
 * Generate a human-readable description for a pending change.
 */
export function describePendingChange(sql: string, origin: PendingChangeOrigin): string {
  const trimmed = sql.replace(/\s+/g, " ").trim();
  const upper = trimmed.toUpperCase();

  // INSERT
  if (upper.startsWith("INSERT")) {
    const table = extractTableName(trimmed, "INTO");
    if (table) return `Insert row into ${table}`;
    return truncate(trimmed);
  }

  // UPDATE
  if (upper.startsWith("UPDATE")) {
    const table = extractTableName(trimmed, "UPDATE");
    const setMatch = trimmed.match(/SET\s+"?([a-z_][a-z0-9_]*)"?\s*=/i);
    const column = setMatch?.[1];
    if (table && column) return `Update ${table}.${column}`;
    if (table) return `Update ${table}`;
    return truncate(trimmed);
  }

  // DELETE
  if (upper.startsWith("DELETE")) {
    const table = extractTableName(trimmed, "FROM");
    if (table) return `Delete row from ${table}`;
    return truncate(trimmed);
  }

  // CREATE TABLE
  if (upper.startsWith("CREATE TABLE")) {
    const table = extractTableName(trimmed, "TABLE");
    if (table) return `Create table ${table}`;
    return truncate(trimmed);
  }

  // CREATE INDEX
  if (upper.startsWith("CREATE INDEX") || upper.startsWith("CREATE UNIQUE INDEX")) {
    const match = trimmed.match(/INDEX\s+(?:IF\s+NOT\s+EXISTS\s+)?"?([a-z_][a-z0-9_]*)"?/i);
    if (match?.[1]) return `Create index ${match[1]}`;
    return truncate(trimmed);
  }

  // DROP TABLE
  if (upper.startsWith("DROP TABLE")) {
    const table = extractTableName(trimmed, "TABLE");
    if (table) return `Drop table ${table}`;
    return truncate(trimmed);
  }

  // DROP INDEX
  if (upper.startsWith("DROP INDEX")) {
    const match = trimmed.match(/INDEX\s+(?:IF\s+EXISTS\s+)?"?([a-z_][a-z0-9_]*)"?/i);
    if (match?.[1]) return `Drop index ${match[1]}`;
    return truncate(trimmed);
  }

  // DROP VIEW
  if (upper.startsWith("DROP VIEW")) {
    const match = trimmed.match(
      /VIEW\s+(?:IF\s+EXISTS\s+)?(?:"?[a-z_][a-z0-9_]*"?\.)?"?([a-z_][a-z0-9_]*)"?/i,
    );
    if (match?.[1]) return `Drop view ${match[1]}`;
    return truncate(trimmed);
  }

  // TRUNCATE
  if (upper.startsWith("TRUNCATE")) {
    const table = extractTableName(trimmed, "TABLE") ?? extractTableName(trimmed, "TRUNCATE");
    if (table) return `Truncate table ${table}`;
    return truncate(trimmed);
  }

  // ALTER TABLE
  if (upper.startsWith("ALTER TABLE")) {
    const table = extractTableName(trimmed, "TABLE");
    if (table) return `Alter table ${table}`;
    return truncate(trimmed);
  }

  // Fallback by origin
  switch (origin) {
    case "inline-edit":
      return "Update cell";
    case "insert-row":
      return "Insert row";
    case "delete-row":
      return "Delete row";
    case "set-default":
      return "Set column default";
    case "create-table":
      return "Create table";
    case "alter-table":
      return "Alter table";
    case "drop-table":
      return "Drop table";
    case "truncate-table":
      return "Truncate table";
    default:
      return truncate(trimmed);
  }
}

function truncate(sql: string, maxLength = 80): string {
  if (sql.length <= maxLength) return sql;
  return sql.slice(0, maxLength) + "…";
}
