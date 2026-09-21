/**
 * Browser-side Better Auth client.
 *
 * Wraps `better-auth/client` with our app's defaults (same-origin fetches).
 * Used by /login, /signup and anywhere else the UI needs to inspect or
 * mutate auth state.
 *
 * Tenant membership and the team roster are NOT Better Auth concerns —
 * they live on the seaquel-app control plane and are surfaced by the
 * server-side `cloud.ts` wrapper. The browser only sees the slim Better
 * Auth user/session.
 *
 * Not used on Tauri (the desktop build doesn't have an auth layer) or on
 * the demo build (no server). `isWeb()` callers are the intended consumers.
 */

import { createAuthClient } from "better-auth/client";

let client: ReturnType<typeof createAuthClient> | null = null;

/**
 * Build the client on first use rather than at module scope.
 *
 * `createAuthClient` validates `baseURL` eagerly and throws on any non-HTTP
 * origin ("Invalid base URL: tauri://localhost"). The desktop app is served
 * from `tauri://localhost`, and this module is statically imported by the app
 * shell — so constructing the client at module scope threw while the layout
 * chunk was still evaluating. The layout's exports were left uninitialized,
 * and every route rendered SvelteKit's bare "500 Internal Error" page. The
 * `isWeb()` guards around the call sites never got a chance to run, because
 * the failure happened at import time.
 *
 * Deferring construction keeps that call on the web build's auth paths, which
 * are its only callers.
 */
export function getAuthClient() {
  client ??= createAuthClient({
    baseURL: typeof window !== "undefined" ? window.location.origin : undefined,
  });
  return client;
}
