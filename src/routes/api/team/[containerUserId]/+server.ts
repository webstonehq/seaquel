/**
 * DELETE /api/team/[containerUserId] — owner-only member removal.
 *
 * The Rust service soft-removes the upstream `tenant_members` row (a no-op
 * in air-gap mode) and deletes the local `member_license` row; then the
 * Better Auth user is deleted here (cascading to `session` and `account`)
 * so the removed member is signed out the next time they hit any route.
 * Owner rows can't be removed — the upstream endpoint returns 409 if
 * attempted.
 */
import { error, json } from "@sveltejs/kit";
import type { RequestHandler } from "./$types";
import { listMembers, unbindMember } from "$lib/server/license-client";
import { openAuthDb } from "$lib/server/auth";

export const DELETE: RequestHandler = async ({ locals, params }) => {
  if (!locals.user) throw error(401, "unauthorized");

  const targetId = params.containerUserId!;
  if (targetId === locals.user.id) {
    throw error(400, "use leave/sign-out to remove yourself");
  }

  const members = await listMembers();
  const me = members.find((m) => m.containerUserId === locals.user!.id);
  if (me?.role !== "owner") throw error(403, "owner-only");

  const { localCleanupFailed } = await unbindMember(targetId);
  if (localCleanupFailed) {
    console.error("[team:DELETE] local cleanup failed (upstream still removed)");
  }

  // Local cleanup so the removed user is signed out. Cascades through the
  // `account` / `session` / `member_license` foreign keys.
  try {
    openAuthDb().prepare(`DELETE FROM "user" WHERE id = ?`).run(targetId);
  } catch (e) {
    console.error("[team:DELETE] local cleanup failed (upstream still removed):", e);
  }

  return json({ ok: true });
};
