/**
 * POST /api/storage/exec
 *
 * Runs a write statement on the authenticated user's `meta.db`. Returns the
 * number of rows affected. Mirrors the desktop's
 * `TauriSqliteDatabase.execute()` wire shape so the browser-side
 * `HttpSqliteProvider` and `TauriSqliteProvider` are interchangeable.
 *
 * The SQL on this endpoint is expected to come from our own repo code (see
 * `src/lib/storage/repos/*`), but a client can send anything, so
 * `assertAllowedStorageSql` restricts it to DML. User-supplied SQL goes
 * through the separate `/api/db/*` path which targets the customer's
 * database, not `meta.db`.
 */

import { error, json } from "@sveltejs/kit";
import { userStorage } from "$lib/server/storage";
import { assertAllowedStorageSql } from "$lib/server/storage-guard";
import type { RequestHandler } from "./$types";

interface ExecBody {
  sql: string;
  params?: unknown[];
}

export const POST: RequestHandler = async ({ locals, request }) => {
  if (!locals.user) throw error(401, "unauthorized");
  const body = (await request.json()) as ExecBody;
  if (!body.sql) throw error(400, "sql is required");
  assertAllowedStorageSql(body.sql);

  const db = await userStorage(locals.user.id);
  const rowsAffected = await db.execute(body.sql, body.params ?? []);
  return json(rowsAffected);
};
