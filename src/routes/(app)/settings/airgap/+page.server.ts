/**
 * Load the current air-gap bundle status for the in-app settings page.
 *
 * Reads directly from the bundle-store + license-cache helpers rather
 * than calling /api/airgap/bundle — same data, but no extra HTTP hop
 * and we get to surface a richer shape (already-decoded numbers, not a
 * JSON envelope). The actual POST / DELETE buttons still hit the API
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
import { readActiveBundle } from "$lib/server/airgap/bundle-store";
import { readInstallCache } from "$lib/server/license-cache";
import { findByUserId } from "$lib/server/member-license";

export const prerender = false;

export const load: PageServerLoad = async ({ locals, url }) => {
  // (app)/+layout.server.ts already enforces auth + license-state gating,
  // but it short-circuits when the build isn't `web`. Repeat the auth
  // check here defensively so a desktop/demo build can't accidentally
  // render this page with a null `locals.user`.
  if (!locals.user) {
    throw redirect(302, `/login?redirect=${encodeURIComponent(url.pathname)}`);
  }

  const ml = findByUserId(locals.user.id);
  const active = await readActiveBundle();
  const cache = readInstallCache();

  return {
    isOwner: ml?.isOwner ?? false,
    mode: cache?.mode ?? "online",
    bundle: active
      ? {
          present: true as const,
          tier: active.payload.tier,
          seats: active.payload.seats,
          notAfter: active.payload.not_after,
          issuedAt: active.payload.issued_at,
          importedAt: active.importedAt,
          pubkeyFingerprint: active.pubkeyFingerprint,
          payloadSha256: active.payloadSha256,
          revokedKeyCount: active.payload.revoked_keys.length,
          expired: Math.floor(Date.now() / 1000) > active.payload.not_after,
        }
      : { present: false as const },
  };
};
