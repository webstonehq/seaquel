/**
 * POST /api/storage/transaction
 *
 * Runs a list of statements atomically against the authenticated user's
 * `meta.db`. Mirrors `TauriSqliteDatabase.transaction()`'s wire shape.
 */

import { error, json } from "@sveltejs/kit";
import { userStorage } from "$lib/server/storage";
import { assertAllowedStorageSql } from "$lib/server/storage-guard";
import type { RequestHandler } from "./$types";

interface TransactionBody {
  statements: Array<{ sql: string; params?: unknown[] }>;
}

export const POST: RequestHandler = async ({ locals, request }) => {
  if (!locals.user) throw error(401, "unauthorized");
  const body = (await request.json()) as TransactionBody;
  if (!Array.isArray(body.statements)) {
    throw error(400, "statements must be an array of {sql, params?}");
  }
  for (const stmt of body.statements) assertAllowedStorageSql(stmt?.sql);

  const db = await userStorage(locals.user.id);
  await db.transaction(body.statements);
  return json(null);
};
