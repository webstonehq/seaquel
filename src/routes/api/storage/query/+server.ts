/**
 * POST /api/storage/query
 *
 * Runs a read statement on the authenticated user's `meta.db`. Returns the
 * row objects (keyed by column name) as a JSON array — same shape the
 * desktop's `TauriSqliteDatabase.query()` returns.
 */

import { error, json } from "@sveltejs/kit";
import { userStorage } from "$lib/server/storage";
import { assertAllowedStorageSql } from "$lib/server/storage-guard";
import type { RequestHandler } from "./$types";

interface QueryBody {
  sql: string;
  params?: unknown[];
}

export const POST: RequestHandler = async ({ locals, request }) => {
  if (!locals.user) throw error(401, "unauthorized");
  const body = (await request.json()) as QueryBody;
  if (!body.sql) throw error(400, "sql is required");
  assertAllowedStorageSql(body.sql);

  const db = await userStorage(locals.user.id);
  const rows = await db.query(body.sql, body.params ?? []);
  return json(rows);
};
