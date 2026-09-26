/**
 * The `/api/*` gate `hooks.server.ts` applies (`handleApiGate`), as a pure
 * decision so its exemptions can be tested.
 */

import type { GateAnswer } from "./license-client";

/** Paths the gate passes through without a session, license or membership. */
export function isApiGateExempt(path: string): boolean {
  // Not an API call: pages and SvelteKit internals (static assets never
  // reach the hooks).
  if (!path.startsWith("/api/")) return true;
  // Better Auth's session / sign-in / sign-up endpoints.
  if (path.startsWith("/api/auth/")) return true;
  // The signup endpoint: it's how a user becomes bound in the first place.
  if (path.startsWith("/api/signup")) return true;
  // Air-gap bundle endpoints handle their own auth policy (loose during
  // first-run, strict on established installs), so a bundle can be
  // imported before any session exists.
  if (path.startsWith("/api/airgap/")) return true;
  // The license section needs to render in the unregistered and suspended
  // states so users see why they're blocked.
  if (path === "/api/account/tenant") return true;
  return false;
}

/** Whether the gate needs the license answer for this request. */
export function needsLicenseGate(path: string, userId: string | null): boolean {
  return !isApiGateExempt(path) && userId !== null;
}

/**
 * The response that blocks a gated `/api/*` request, or `null` to let it
 * through. `gate` is the user's answer (required when `needsLicenseGate`).
 */
export function apiGateResponse(
  path: string,
  userId: string | null,
  licenseState: GateAnswer["state"],
  gate: GateAnswer | null,
): Response | null {
  if (isApiGateExempt(path)) return null;

  // Data-plane and admin API: an active session, a usable license state and
  // a bound member_license row.
  if (!userId) return new Response("unauthorized", { status: 401 });
  if (licenseState !== "ok") {
    return new Response(`license ${licenseState}`, { status: 403 });
  }
  // A revoked row counts as not bound: the bundle-import revocation walk
  // stamps `revoked_at` but leaves the row for auditing. With the session
  // purge during that walk, this completes the revoke flow.
  const member = gate?.member ?? null;
  if (!member || member.revoked) {
    return new Response("not a bound member of this install", { status: 403 });
  }
  return null;
}

/**
 * The answer while the Rust license service doesn't respond: JSON for the
 * API, a plain page for everything else. English only, like the other
 * server-rendered operator messages (the page can't load the app's i18n
 * without the app).
 */
export function licenseServiceUnavailable(path: string): Response {
  if (path.startsWith("/api/")) {
    return new Response(JSON.stringify({ code: "license_service_unavailable" }), {
      status: 503,
      headers: { "content-type": "application/json", "retry-after": "10" },
    });
  }
  const body = `<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Seaquel is unavailable</title></head>
<body style="font-family: system-ui, sans-serif; max-width: 40rem; margin: 4rem auto; padding: 0 1rem; line-height: 1.5">
<h1>Seaquel is unavailable</h1>
<p>The license service isn't responding, so this server can't check its license.</p>
<p>If you run this server, check the container logs for <code>seaquel-server</code> errors, then restart it.</p>
</body>
</html>
`;
  return new Response(body, {
    status: 503,
    headers: { "content-type": "text/html; charset=utf-8", "retry-after": "10" },
  });
}
