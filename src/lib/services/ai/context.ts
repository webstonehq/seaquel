import type { DatabaseType } from "$lib/types/database";
import { READ_ONLY_REFUSAL, validateReadOnlyQuery } from "$lib/sql";

/**
 * The page's read-only check for SQL an AI or a dashboard runs
 * (`executeReadOnly`, and the dashboard tools' widget queries, which Core
 * checked already): the refusal message, or `null` when the query may
 * run. It reads the query with the connection's quoting (`$lib/sql`), so
 * without a known connection type it refuses: it fails
 * closed, as `validateReadOnlyQuery` does when the module fails or the type
 * isn't an engine it knows.
 */
export function readOnlyError(query: string, dbType: DatabaseType | undefined): string | null {
  if (!dbType) return READ_ONLY_REFUSAL;
  return validateReadOnlyQuery(query, dbType);
}
