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

export const authClient = createAuthClient({
  baseURL: typeof window !== "undefined" ? window.location.origin : undefined,
});
