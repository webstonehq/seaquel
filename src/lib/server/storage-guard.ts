/**
 * Statement allowlist for the `/api/storage/*` endpoints.
 *
 * Those endpoints execute SQL sent by the browser against the caller's
 * `meta.db`. The browser only ever sends DML from `src/lib/storage/repos/*`
 * (schema bootstrap and PRAGMAs run server-side in `storage.ts`), so anything
 * else is refused. In particular this blocks ATTACH (which would expose
 * `auth.db` or other users' files through the pooled handle), VACUUM INTO
 * (arbitrary file writes) and PRAGMA.
 *
 * Checking the leading keyword is sufficient because better-sqlite3's
 * `prepare()` rejects strings containing more than one statement.
 */

import { error } from "@sveltejs/kit";

const ALLOWED_LEADING_KEYWORDS = new Set([
  "SELECT",
  "INSERT",
  "UPDATE",
  "DELETE",
  "REPLACE",
  "WITH",
]);

/** Returns the SQL with leading whitespace, comments and `(` removed, or null on an unterminated comment. */
function stripLeadingTrivia(sql: string): string | null {
  let i = 0;
  while (i < sql.length) {
    const ch = sql[i];
    if (/\s/.test(ch) || ch === "(") {
      i++;
    } else if (sql.startsWith("--", i)) {
      const end = sql.indexOf("\n", i);
      if (end === -1) return "";
      i = end + 1;
    } else if (sql.startsWith("/*", i)) {
      const end = sql.indexOf("*/", i + 2);
      if (end === -1) return null;
      i = end + 2;
    } else {
      break;
    }
  }
  return sql.slice(i);
}

export function assertAllowedStorageSql(sql: unknown): asserts sql is string {
  if (typeof sql !== "string") throw error(400, "sql must be a string");
  const rest = stripLeadingTrivia(sql);
  const keyword = rest?.match(/^[A-Za-z]+/)?.[0].toUpperCase();
  if (!keyword || !ALLOWED_LEADING_KEYWORDS.has(keyword)) {
    throw error(400, "statement not allowed");
  }
}
