import type { PageServerLoad } from "./$types";
import { readInstallCache } from "$lib/server/license-cache";
import { readActiveBundle } from "$lib/server/airgap/bundle-store";

export const prerender = false;

export const load: PageServerLoad = async ({ locals }) => {
  const cache = readInstallCache();
  const bundle = await readActiveBundle();
  const nowSec = Math.floor(Date.now() / 1000);
  const bundlePresent = !!bundle;
  const bundleExpired = !!(bundle && nowSec > bundle.payload.not_after);
  const bundleNotAfter = bundle ? new Date(bundle.payload.not_after * 1000).toISOString() : null;
  return {
    lastValidatedAt: cache?.lastValidatedAt ?? null,
    graceUntil: cache?.graceUntil ?? null,
    mode: cache?.mode ?? "online",
    bundlePresent,
    bundleExpired,
    bundleNotAfter,
    signedIn: !!locals.user,
  };
};
