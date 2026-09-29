import { m } from "$lib/paraglide/messages.js";
import { extractErrorMessage } from "$lib/errors";
import { CoreCallError } from "$lib/storage/rust-client";
import { TRANSACTION_OPEN } from "./edit-service/types";
import { CONFIRM_REQUIRED } from "./query-runner/types";

/** Core's code for a statement the database refused: its message is the database's own. */
export const QUERY_ERROR = "QUERY_ERROR";
/** The TS runner's code for the SQL module failing on the run path. */
export const SQL_CHECK_FAILED = "SQL_CHECK_FAILED";
/** The web server's code for a user's large calls past its cap: nothing ran. */
export const TOO_MANY_REQUESTS = "TOO_MANY_REQUESTS";

/**
 * An error as the grid and toasts show it: the database's message alone for
 * `QUERY_ERROR`, a translated sentence when the SQL module failed, a
 * transaction opened by hand is in the way, an edit needs confirming, or the web server has too
 * many of the user's large calls running, and `CODE: message` otherwise
 * (the code says what went wrong around the query: `CONNECTION_CLOSED`,
 * `WS_CLOSED`, `RESULT_TOO_LARGE`, …).
 */
export function errorText(code: string, message: string): string {
  if (code === QUERY_ERROR) return message;
  if (code === SQL_CHECK_FAILED) return m.statement_at_cursor_failed({ error: message });
  if (code === TRANSACTION_OPEN) return m.edit_transaction_open();
  if (code === CONFIRM_REQUIRED) return m.edit_confirm_required();
  if (code === TOO_MANY_REQUESTS) return m.rpc_too_many_requests();
  return `${code}: ${message}`;
}

/** A rejected call's `{code, message}`: a `CoreCallError`'s, else `UNKNOWN` and its text. */
export function callError(error: unknown): { code: string; message: string } {
  if (error instanceof CoreCallError) {
    return { code: error.code, message: error.message.replace(`${error.code}: `, "") };
  }
  return { code: "UNKNOWN", message: extractErrorMessage(error) };
}

/** A rejected call as `errorText` shows it. */
export function callErrorText(error: unknown): string {
  if (!(error instanceof CoreCallError)) return extractErrorMessage(error);
  const { code, message } = callError(error);
  return errorText(code, message);
}
