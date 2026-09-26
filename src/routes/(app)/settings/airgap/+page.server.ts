/**
 * Load the current air-gap bundle status for the in-app settings page.
 *
 * Reads the status from the license service (`airgapStatus`) rather
 * than calling /api/airgap/bundle — same data, no extra HTTP hop, and a
 * richer shape. The actual POST / DELETE buttons still hit the API
 * endpoint so the revocation walk + mode flip happen in one trusted
 * place.
 *
 * Ownership matters for which controls render: the API will already
 * reject non-owner POST/DELETE with 401/403, but hiding the buttons
 * keeps the UI honest — a member shouldn't see "Replace bundle" only to
 * get told no on submit.
 */
import { redirect } from "@sveltejs/kit";
import type { PageServerLoad } from "./$types";
import { airgapStatus } from "$lib/server/license-client";

export const prerender = false;

export const load: PageServerLoad = async ({ locals, url }) => {
  // (app)/+layout.server.ts already enforces auth + license-state gating,
  // but it short-circuits when the build isn't `web`. Repeat the auth
  // check here defensively so a desktop/demo build can't accidentally
  // render this page with a null `locals.user`.
  if (!locals.user) {
    throw redirect(302, `/login?redirect=${encodeURIComponent(url.pathname)}`);
  }

  const status = await airgapStatus(locals.user.id);
  const active = status.bundle;

  return {
    isOwner: status.member?.isOwner ?? false,
    mode: status.mode,
    bundle: active ? { present: true as const, ...active } : { present: false as const },
  };
};
