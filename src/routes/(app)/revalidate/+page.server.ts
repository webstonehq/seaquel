import type { PageServerLoad } from "./$types";
import { airgapStatus } from "$lib/server/license-client";

export const prerender = false;

export const load: PageServerLoad = async ({ locals }) => {
  const status = await airgapStatus(null);
  const bundle = status.bundle;
  const bundlePresent = !!bundle;
  const bundleExpired = !!bundle?.expired;
  const bundleNotAfter = bundle ? new Date(bundle.notAfter * 1000).toISOString() : null;
  return {
    lastValidatedAt: status.lastValidatedAt,
    graceUntil: status.graceUntil,
    mode: status.mode,
    bundlePresent,
    bundleExpired,
    bundleNotAfter,
    signedIn: !!locals.user,
  };
};
