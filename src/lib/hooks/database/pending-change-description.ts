import type { DatabaseType } from "$lib/types";
import type { PendingChangeOrigin } from "$lib/types/pending-changes";
import { changeSummary, detectQueryType, type ChangeSummary, type QueryType } from "$lib/sql";

/**
 * A pending change's description in the sheet, from what the statement does
 * (`ChangeSummary`, read by Core's scanner: phase 5c). The
 * English and the origin fallback stay here.
 *
 * Without a summary, a statement that reads as an INSERT, UPDATE or DELETE
 * (one whose table couldn't be read) and anything typed in the editor show
 * their SQL, cut at 80 characters; any other origin names itself.
 */
export function describeChange(
  summary: ChangeSummary | null | undefined,
  sql: string,
  origin: PendingChangeOrigin,
  queryType: QueryType,
): string {
  if (summary) {
    const { table, column } = summary;
    switch (summary.verb) {
      case "insert":
        return `Insert row into ${table}`;
      case "update":
        return column ? `Update ${table}.${column}` : `Update ${table}`;
      case "delete":
        return `Delete row from ${table}`;
      case "createTable":
        return `Create table ${table}`;
      case "createIndex":
        return `Create index ${table}`;
      case "dropTable":
        return `Drop table ${table}`;
      case "dropIndex":
        return `Drop index ${table}`;
      case "dropView":
        return `Drop view ${table}`;
      case "truncate":
        return `Truncate table ${table}`;
      case "alterTable":
        return `Alter table ${table}`;
    }
  }

  const trimmed = sql.replace(/\s+/g, " ").trim();
  if (queryType === "insert" || queryType === "update" || queryType === "delete") {
    return truncate(trimmed);
  }
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
    case "drop-view":
      return "Drop view";
    case "truncate-table":
      return "Truncate table";
    default:
      return truncate(trimmed);
  }
}

/**
 * `describeChange` for SQL whose summary nobody planned (a statement the
 * editor deferred, one the table editor generated): read with the
 * connection's quoting through `$lib/sql`.
 */
export function describePendingChange(
  sql: string,
  origin: PendingChangeOrigin,
  engine: DatabaseType,
): string {
  return describeChange(changeSummary(sql, engine), sql, origin, detectQueryType(sql, engine));
}

function truncate(sql: string, maxLength = 80): string {
  if (sql.length <= maxLength) return sql;
  return sql.slice(0, maxLength) + "…";
}
