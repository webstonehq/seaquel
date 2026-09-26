import type { Handle } from "@sveltejs/kit";
import { sequence } from "@sveltejs/kit/hooks";
import { building } from "$app/environment";
import { paraglideMiddleware } from "$lib/paraglide/server";
import { apiGateResponse, licenseServiceUnavailable, needsLicenseGate } from "$lib/server/api-gate";
import { auth } from "$lib/server/auth";
import { gate as licenseGate, LicenseClientError } from "$lib/server/license-client";

// No `init` hook needed: each server-side data store is now lazily
// bootstrapped on first access:
//   - Better Auth's `auth.db` schema is applied inside `auth.ts` on the
//     first auth API call (gated by `if (building)` to avoid build-time DB
//     creation).
//   - Per-user `meta.db` files are opened (and their schema applied) by the
//     Rust service on that user's first `/api/rpc` call.
//   - Licensing lives in the Rust service too (`/internal/license/*`, via
//     `license-client.ts`), which answers NOT_READY until `auth.db` exists;
//     `license-client` opens it before its first call.

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
  // `building` catches the prerender pass during `vite build`, where a
  // session lookup would create `auth.db` (and apply its schema) as a build
  // artifact. Node never opens a user's `meta.db`; the Rust service does.
  if (import.meta.env.VITE_BUILD_TARGET !== "web" || building) {
    event.locals.user = null;
    event.locals.session = null;
    event.locals.tenant = null;
    event.locals.licenseState = "unregistered";
    return resolve(event);
  }

  // The liveness probe (Docker HEALTHCHECK) answers without the session or
  // the license service, so a Rust-only problem doesn't restart the
  // container.
  if (event.url.pathname === "/health") {
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
  // + grace-period ladder (in Rust). No deployment-mode branch. When no
  // owner has registered an install yet, state is "unregistered" and
  // locals.tenant stays null — the signup flow handles that case. The
  // answer is cached for 5 s per user (license-client).
  let state: Awaited<ReturnType<typeof licenseGate>>;
  try {
    state = await licenseGate(event.locals.user?.id ?? null);
  } catch (e) {
    // Fail closed, but say why: an unreachable license service is an
    // operator problem, not the user's license.
    if (e instanceof LicenseClientError && e.code === "UPSTREAM_UNAVAILABLE") {
      return licenseServiceUnavailable(event.url.pathname);
    }
    throw e;
  }
  event.locals.tenant = state.state === "ok" || state.state === "suspended" ? state.tenant : null;
  event.locals.licenseState = state.state;

  return resolve(event);
};

const handleApiGate: Handle = async ({ event, resolve }) => {
  if (import.meta.env.VITE_BUILD_TARGET !== "web" || building) {
    return resolve(event);
  }

  // Exemptions and rules: `api-gate.ts`. The membership row comes from the
  // same (cached) gate answer handleAuth read.
  const path = event.url.pathname;
  const userId = event.locals.user?.id ?? null;
  const answer = needsLicenseGate(path, userId) ? await licenseGate(userId) : null;
  const blocked = apiGateResponse(path, userId, event.locals.licenseState, answer);
  return blocked ?? resolve(event);
};

export const handle: Handle = sequence(handleAuth, handleApiGate, handleParaglide);
