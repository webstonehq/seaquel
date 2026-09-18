/**
 * DELETE /api/team/[containerUserId] — owner-only member removal.
 *
 * Calls the control plane to soft-remove the `tenant_members` row,
 * then deletes the local Better Auth user (cascades to session +
 * member_license) so the removed member is signed out the next time
 * they hit any route. Owner rows can't be removed — the upstream
 * endpoint returns 409 if attempted.
 */
import { error, json } from "@sveltejs/kit";
import type { RequestHandler } from "./$types";
import { listMembers, unbindMember } from "$lib/server/licensing";
import { openAuthDb } from "$lib/server/auth";
import { deleteByUserId } from "$lib/server/member-license";

export const DELETE: RequestHandler = async ({ locals, params }) => {
  if (!locals.user) throw error(401, "unauthorized");

  const targetId = params.containerUserId!;
  if (targetId === locals.user.id) {
    throw error(400, "use leave/sign-out to remove yourself");
  }

  const members = await listMembers();
  const me = members.find((m) => m.containerUserId === locals.user!.id);
  if (me?.role !== "owner") throw error(403, "owner-only");

  await unbindMember(targetId);

  // Local cleanup so the removed user is signed out and unbound. Cascade
  // through `account` / `session` / `member_license` foreign keys.
  try {
    deleteByUserId(targetId);
    openAuthDb().prepare(`DELETE FROM "user" WHERE id = ?`).run(targetId);
  } catch (e) {
    console.error("[team:DELETE] local cleanup failed (upstream still removed):", e);
  }

  return json({ ok: true });
};
