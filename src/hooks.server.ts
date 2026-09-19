import type { Handle } from "@sveltejs/kit";
import { sequence } from "@sveltejs/kit/hooks";
import { building } from "$app/environment";
import { paraglideMiddleware } from "$lib/paraglide/server";
import { auth } from "$lib/server/auth";
import { resolveLicenseState } from "$lib/server/licensing";
import { findByUserId } from "$lib/server/member-license";

// No `init` hook needed: each server-side data store is now lazily
// bootstrapped on first access:
//   - Better Auth's `auth.db` schema is applied inside `auth.ts` on the
//     first auth API call (gated by `if (building)` to avoid build-time DB
//     creation).
//   - Per-user `meta.db` schemas are applied by `src/lib/server/storage.ts`
//     on first request from that user.

const RTL_LOCALES = ["ar", "he", "fa", "ur"];

const handleParaglide: Handle = ({ event, resolve }) =>
  paraglideMiddleware(event.request, ({ request, locale }) => {
    event.request = request;
    const dir = RTL_LOCALES.includes(locale) ? "rtl" : "ltr";

    return resolve(event, {
      transformPageChunk: ({ html }) =>
        html.replace("%paraglide.lang%", locale).replace("%paraglide.dir%", dir),
    });
  });

const handleAuth: Handle = async ({ event, resolve }) => {
  // Only the `web` build has an auth layer. Desktop (Tauri) and demo ship
  // adapter-static, so there's no runtime server — but `vite dev` still runs
  // hooks for `tauri dev`, and without this gate Better Auth initializes
  // (and warns about the missing baseURL) on every request.
  //
  // `building` catches the prerender pass during `vite build`, where opening
  // meta.db as a build artifact would happen before `init` runs migrations.
  if (import.meta.env.VITE_BUILD_TARGET !== "web" || building) {
    event.locals.user = null;
    event.locals.session = null;
    event.locals.tenant = null;
    event.locals.licenseState = "unregistered";
    return resolve(event);
  }

  // Better Auth accepts a Request-like thing with `.headers`. SvelteKit's
  // `event.request` is a standard Request, so this works directly.
  let result: Awaited<ReturnType<typeof auth.api.getSession>> | null = null;
  try {
    result = await auth.api.getSession({ headers: event.request.headers });
  } catch (e) {
    // "No such table" means the auth schema migration hasn't run yet — this
    // is the only error we treat as "effectively unauthenticated". Anything
    // else (driver panic, disk full, corrupt DB) should surface in logs so
    // it doesn't masquerade as every user silently logging out.
    const msg = e instanceof Error ? e.message : String(e);
    if (!/no such table/i.test(msg)) {
      console.error("[seaquel] auth.getSession failed:", e);
    }
    result = null;
  }

  if (result?.user && result.session) {
    event.locals.user = {
      id: result.user.id,
      email: result.user.email,
      name: result.user.name ?? null,
    };
    event.locals.session = {
      id: result.session.id,
      userId: result.session.userId,
      expiresAt: Math.floor(new Date(result.session.expiresAt).getTime() / 1000),
    };
  } else {
    event.locals.user = null;
    event.locals.session = null;
  }

  // License/tenant context: always resolved through the persisted cache
  // + grace-period ladder. No deployment-mode branch. When no owner has
  // registered an install yet, state is "unregistered" and locals.tenant
  // stays null — the signup flow handles that case.
  const state = await resolveLicenseState();
  event.locals.tenant = state.kind === "ok" || state.kind === "suspended" ? state.tenant : null;
  event.locals.licenseState = state.kind;

  return resolve(event);
};

const handleApiGate: Handle = async ({ event, resolve }) => {
  if (import.meta.env.VITE_BUILD_TARGET !== "web" || building) {
    return resolve(event);
  }

  const path = event.url.pathname;

  // Paths that must NOT be gated by license/membership:
  //   - Better Auth's session/sign-in/sign-up endpoints
  //   - The signup endpoint (it's how a user becomes bound in the first place)
  //   - The account/tenant endpoint (license-section UI needs to render in
  //     both the unregistered and suspended states so users see why they're
  //     blocked)
  //   - Static assets and SvelteKit internals are not under /api so this
  //     path filter is sufficient.
  if (!path.startsWith("/api/")) return resolve(event);
  if (path.startsWith("/api/auth/")) return resolve(event);
  if (path.startsWith("/api/signup")) return resolve(event);
  // Air-gap bundle endpoints handle their own auth policy (loose during
  // first-run, strict on established installs). Pass through here so the
  // frontend can import a bundle before any session exists.
  if (path.startsWith("/api/airgap/")) return resolve(event);
  if (path === "/api/account/tenant") return resolve(event);

  // Beyond this point: data-plane and admin API. Require an active session,
  // a usable license state, AND a bound member_license row.
  if (!event.locals.user) {
    return new Response("unauthorized", { status: 401 });
  }
  if (event.locals.licenseState !== "ok") {
    return new Response(`license ${event.locals.licenseState}`, { status: 403 });
  }
  // A revoked member_license row counts as "not bound": the bundle-import
  // revocation walk stamps `revoked_at` but leaves the row in place so we
  // can audit later. Treating revoked rows as missing membership forces a
  // 403 on the next request, which combined with the session purge during
  // the revocation walk completes the revoke flow.
  const ml = findByUserId(event.locals.user.id);
  if (!ml || ml.revokedAt != null) {
    return new Response("not a bound member of this install", { status: 403 });
  }

  return resolve(event);
};

export const handle: Handle = sequence(handleAuth, handleApiGate, handleParaglide);
