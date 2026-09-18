import { redirect } from "@sveltejs/kit";
import { building } from "$app/environment";
import type { LayoutServerLoad } from "./$types";
import { findByUserId } from "$lib/server/member-license";
import { isBundleDriven } from "$lib/server/airgap/bundle-store";
import { readInstallCache } from "$lib/server/license-cache";

// Server-side gate for the main app shell. One path for Cloud and
// self-hosted alike: anyone authenticated must also be bound to a
// license seat for the install, and the install must be in a usable
// license state (ok / not suspended / not past the grace window).
export const load: LayoutServerLoad = ({ locals, url }) => {
  if (building) return {};
  // `BUILD_TARGET` is set as an npm-script env at build time. Read it via
  // `import.meta.env.VITE_BUILD_TARGET` (inlined by vite.config.js) so it
  // resolves at *runtime* inside adapter-node too — `process.env` is not
  // exported into the running container. Matches `hooks.server.ts`.
  if (import.meta.env.VITE_BUILD_TARGET !== "web") return {};

  if (!locals.user) {
    const here = url.pathname + url.search;
    throw redirect(302, `/login?redirect=${encodeURIComponent(here)}`);
  }

  // License state ladder. "unregistered" → signup will handle the
  // first-owner self-register; "revalidate" → grace expired offline;
  // "suspended" → explicit suspension from the control plane.
  if (locals.licenseState === "unregistered") {
    // Best-effort detour for offline-first installs: if the install
    // hasn't been bound to a tenant yet AND there's no bundle on disk,
    // the operator is probably bootstrapping a self-hosted instance with
    // no control-plane connectivity. Sending them to /signup would just
    // 503 on submit (no tenant + no transport). Land them on
    // /airgap-setup instead so they can import a bundle first. From
    // there a CTA link takes them to /signup. This redirect is purely a
    // UX nicety — /signup itself still has to handle the 503 case for
    // operators who land there via a bookmark.
    const here = url.pathname + url.search;
    const cache = readInstallCache();
    const hasTenant = !!(cache && cache.tenantId);
    const hasBundle = isBundleDriven();
    if (
      !hasTenant &&
      !hasBundle &&
      !url.pathname.startsWith("/signup") &&
      !url.pathname.startsWith("/airgap-setup")
    ) {
      throw redirect(302, `/airgap-setup?redirect=${encodeURIComponent(here)}`);
    }
    if (!url.pathname.startsWith("/signup") && !url.pathname.startsWith("/airgap-setup")) {
      throw redirect(302, `/signup?redirect=${encodeURIComponent(here)}`);
    }
  } else if (locals.licenseState === "suspended") {
    if (!url.pathname.startsWith("/suspended")) {
      throw redirect(302, "/suspended");
    }
  } else if (locals.licenseState === "revalidate") {
    if (!url.pathname.startsWith("/revalidate")) {
      throw redirect(302, "/revalidate");
    }
  } else {
    // ok — enforce per-user membership binding.
    const bound = findByUserId(locals.user.id);
    if (!bound) {
      const here = url.pathname + url.search;
      throw redirect(302, `/signup?reason=membership&redirect=${encodeURIComponent(here)}`);
    }
  }

  return { tenant: locals.tenant };
};
