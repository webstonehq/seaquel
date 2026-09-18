/**
 * GET /api/account/tenant — slim tenant view for the current user.
 *
 * Returns just enough for the License section UI: tier, subscription
 * status, seat limit, owner email, billing cycle end. Sourced from
 * `event.locals.tenant`, which `hooks.server.ts` resolves through the
 * persisted install_cache + grace-period ladder.
 *
 * Returns a null-stub when no install has been registered yet (first-
 * run before the first owner signs up) or when grace has expired and
 * the control plane is unreachable. The UI hides the section in those
 * cases.
 */
import { error, json } from "@sveltejs/kit";
import type { RequestHandler } from "./$types";

export const GET: RequestHandler = ({ locals }) => {
  if (!locals.user) throw error(401, "unauthorized");

  if (!locals.tenant) {
    return json({
      tier: null,
      status: null,
      ownerEmail: "",
      seatLimit: 0,
      currentPeriodEnd: null,
    });
  }

  return json({
    tier: locals.tenant.tier,
    status: locals.tenant.status,
    ownerEmail: locals.tenant.ownerEmail,
    seatLimit: locals.tenant.seatLimit,
    currentPeriodEnd: locals.tenant.currentPeriodEnd,
  });
};
