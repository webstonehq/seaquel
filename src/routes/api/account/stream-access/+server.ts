/**
 * GET /api/account/stream-access — authorizes an `/api/rpc/stream` WebSocket.
 *
 * The upgrade is handled outside SvelteKit (`shared/rpc-stream-proxy.js`,
 * from `server.js` or the dev server's Vite plugin), so the proxy calls this
 * endpoint with the browser's cookie and takes the user id it answers.
 * Being under `/api/`, the request passes through `handleApiGate` in
 * `hooks.server.ts`, which enforces the same session + license +
 * bound-membership checks as every other data-plane route. Reaching this
 * handler therefore means the session may stream.
 *
 * The proxy passes the upgrade's `Origin` along, and it must be one this
 * install trusts (`isOriginTrusted`): a configured origin, or the install's
 * own, an `Origin` naming the upgrade's `Host`. The proxy's lookup arrives
 * over loopback, so it sends that `Host` in `x-seaquel-upgrade-host` (always
 * set from the upgrade, never from a browser header); without it the
 * request's own `Host` counts. A WebSocket isn't
 * covered by CORS, so without this another site could open a socket with
 * the user's cookie (cross-site WebSocket hijacking). Browsers always send
 * `Origin` on a WebSocket handshake.
 */
import { error, json } from "@sveltejs/kit";
import { isOriginTrusted } from "$lib/server/auth";
import type { RequestHandler } from "./$types";

/** `UPGRADE_HOST_HEADER` in `shared/rpc-stream-proxy.js` (not imported: it pulls in `ws`). */
const UPGRADE_HOST_HEADER = "x-seaquel-upgrade-host";

export const GET: RequestHandler = ({ locals, request }) => {
  if (!locals.user) throw error(401, "unauthorized");
  const host = request.headers.get(UPGRADE_HOST_HEADER) ?? request.headers.get("host");
  if (!isOriginTrusted(request.headers.get("origin"), host)) {
    throw error(403, "untrusted origin");
  }
  return json({ userId: locals.user.id });
};
