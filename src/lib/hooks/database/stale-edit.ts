import { m } from "$lib/paraglide/messages.js";
import { cellText } from "$lib/values";

/**
 * A grid edit, set-to-default or delete targets one row by its primary key.
 * When the key no longer matches (the row was deleted or its key changed
 * since it was loaded, or the key didn't compare as sent), the statement
 * affects 0 rows and the edit is lost. Core fails it with
 * `NO_ROWS_AFFECTED` (phase 5c, Decision 4), whose message holds no key;
 * these helpers word it for the user from the change's table and key.
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
