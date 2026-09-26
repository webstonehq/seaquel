import type { QueryTab, ParameterValue } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import {
  getStatementAtOffsetOrThrow,
  ParameterSubstitutionError,
  substituteParameters,
} from "$lib/sql";
import { extractErrorMessage } from "$lib/errors";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";

type Resolved = { tab: QueryTab; query: string; bindValues?: unknown[] };

/**
 * Resolve query text from a query tab, optionally extracting the statement at cursor
 * and substituting parameters. Shared across explain-tabs, visualize-tabs, and query-execution.
 *
 * `null` when there's nothing to run, or after an error toast when it can't
 * be resolved safely: a parameter value that can't be substituted, or the
 * SQL module failing to find the statement at the cursor (running the whole
 * buffer instead could run a statement the user didn't pick).
 */
export function resolveQuery(
  state: DatabaseState,
  tabId: string,
  cursorOffset?: number,
  parameterValues?: ParameterValue[],
  forceInline?: boolean,
): Resolved | null {
  try {
    return resolveQueryOrThrow(state, tabId, cursorOffset, parameterValues, forceInline);
  } catch (error) {
    errorToast(m.statement_at_cursor_failed({ error: extractErrorMessage(error) }));
    return null;
  }
}

/**
 * `resolveQuery`, but a failure of the SQL module to find the statement at
 * the cursor throws, so the caller can report it its own way (the query
 * runner shows it as the result). Parameter errors still toast and give `null`.
 */
export function resolveQueryOrThrow(
  state: DatabaseState,
  tabId: string,
  cursorOffset?: number,
  parameterValues?: ParameterValue[],
  forceInline?: boolean,
): Resolved | null {
  const projectId = state.activeProjectId;
  if (!projectId) return null;

  const tabs = state.queryTabsByProject[projectId] ?? [];
  const tab = tabs.find((t) => t.id === tabId);
  if (!tab || !tab.query.trim()) return null;

  const dbType = state.activeConnection?.type ?? "postgres";
  let query = tab.query;

  if (cursorOffset !== undefined) {
    const statement = getStatementAtOffsetOrThrow(tab.query, cursorOffset, dbType);
    if (statement) query = statement.sql;
  }

  if (!query.trim()) return null;

  if (parameterValues) {
    try {
      const { sql, bindValues } = substituteParameters(query, parameterValues, dbType, forceInline);
      return { tab, query: sql, bindValues };
    } catch (error) {
      // A value that can't be substituted safely: shown, nothing runs.
      if (!(error instanceof ParameterSubstitutionError)) throw error;
      errorToast(error.message);
      return null;
    }
  }
  return { tab, query };
}
