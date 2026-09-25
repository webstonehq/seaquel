import type { PendingChange } from "$lib/types";
import { m } from "$lib/paraglide/messages.js";
import { cellText } from "$lib/values";

/**
 * A grid edit, set-to-default or delete targets one row by its primary key.
 * When the key no longer matches (the row was deleted or its key changed
 * since it was loaded, or the key didn't compare as sent), the statement
 * affects 0 rows and the edit is lost. These helpers turn that into an error.
 *
 * Engines count differently. MySQL/MariaDB count matched rows, not changed
 * ones (sqlx connects with CLIENT_FOUND_ROWS), so setting a cell to its
 * current value still counts. MSSQL connections turn NOCOUNT off when they
 * open, in case the server's `user options` turn it on.
 *
 * A 0 doesn't always mean nothing happened: a Postgres rule (DO INSTEAD) or
 * a SQLite INSTEAD OF trigger on a view may have applied the change and
 * still report 0 rows, so the "no row matched" error can be wrong there.
 * Such targets rarely have a primary key the grid can edit by.
 */

/** Pending-change origins that are a keyed UPDATE or DELETE of one row. */
const KEYED_ORIGINS = new Set<PendingChange["origin"]>([
  "inline-edit",
  "set-default",
  "delete-row",
]);

/** Whether `change` is a keyed single-row edit that must affect a row. */
export function expectsRow(change: PendingChange): boolean {
  return KEYED_ORIGINS.has(change.origin) && !!change.target?.primaryKeyValues;
}

/** `id = 5` or `a = 1, b = 'x'`, for messages. */
export function describeKey(primaryKeyValues: Record<string, unknown>): string {
  return Object.entries(primaryKeyValues)
    .map(([k, v]) => {
      if (v === null || v === undefined) return `${k} = NULL`;
      return typeof v === "string"
        ? `${k} = '${v.replaceAll("'", "''")}'`
        : `${k} = ${cellText(v)}`;
    })
    .join(", ");
}

/** The error for a keyed edit of `schema.table` that matched no row. */
export function noRowMatchedMessage(
  schema: string,
  table: string,
  primaryKeyValues: Record<string, unknown>,
): string {
  return m.edit_no_row_matched({
    table: schema ? `${schema}.${table}` : table,
    key: describeKey(primaryKeyValues),
  });
}
