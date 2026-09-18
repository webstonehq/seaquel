/**
 * GET /api/team — list active members of this tenant.
 *
 * Server-side proxy to the control plane's `/api/cloud/members`. Lives
 * here so the install's owner license key never leaves the container.
 *
 * Returns the same `MemberView[]` shape the upstream endpoint emits,
 * plus a `currentUserRole` field so the UI can decide whether to show
 * the "Remove" buttons without inferring it from a contains-check on
 * the email.
 */
import { error, json } from "@sveltejs/kit";
import type { RequestHandler } from "./$types";
import { listMembers } from "$lib/server/licensing";

export const GET: RequestHandler = async ({ locals }) => {
  if (!locals.user) throw error(401, "unauthorized");

  const members = await listMembers();
  const me = members.find((m) => m.containerUserId === locals.user!.id);
  return json({
    members,
    currentUserRole: me?.role ?? "member",
  });
};
