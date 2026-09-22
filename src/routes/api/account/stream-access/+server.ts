/**
 * GET /api/account/stream-access — authorizes a `/api/db/stream` WebSocket.
 *
 * `server.js` handles the WebSocket upgrade outside SvelteKit, so it calls
 * this endpoint with the browser's cookie. Being under `/api/`, the request
 * passes through `handleApiGate` in `hooks.server.ts`, which enforces the
 * same session + license + bound-membership checks as every other data-plane
 * route. Reaching this handler therefore means the stream is allowed.
 */
import { error, json } from "@sveltejs/kit";
import type { RequestHandler } from "./$types";

export const GET: RequestHandler = ({ locals }) => {
  if (!locals.user) throw error(401, "unauthorized");
  return json({ userId: locals.user.id });
};
