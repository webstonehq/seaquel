/**
 * POST /api/rpc: forwards one workspace call to the Rust service's `/rpc`.
 *
 * Rust takes the user from `X-Seaquel-User` and trusts it, so this route
 * sets it from the session (`locals.user.id`) and never forwards one the
 * browser sent. `handleApiGate` in `hooks.server.ts` has already checked the
 * session, license and membership before this runs, and the request's
 * `Origin` must be one this install trusts (the page's own, or configured).
 *
 * The body goes through as the bytes that arrived. It is never parsed and
 * re-stringified: `method` must stay before `params`, and stored JSON
 * columns must stay byte-identical. Rust's status and body (a `Response`, or
 * `RpcError` JSON) come back unchanged.
 */

import { error } from "@sveltejs/kit";
import { originNotAllowed } from "$lib/server/api-gate";
import { isOriginTrusted } from "$lib/server/origin";
import type { RequestHandler } from "./$types";

const RUST_BASE_URL = process.env.SEAQUEL_RUST_URL ?? "http://127.0.0.1:8788";

const USER_HEADER = "x-seaquel-user";

export const POST: RequestHandler = async ({ locals, request }) => {
  if (!locals.user) throw error(401, "unauthorized");
  // CSRF: only the app's own pages (or a configured origin) may call. The
  // hook's Origin gate checks this for every /api mutation too; this keeps
  // the route safe on its own.
  if (!isOriginTrusted(request.headers.get("origin"), request.headers.get("host"))) {
    return originNotAllowed();
  }

  // Only these two headers reach Rust: no cookies, no auth headers and no
  // client-sent X-Seaquel-User.
  const headers: Record<string, string> = {
    "content-type": "application/json",
    [USER_HEADER]: locals.user.id,
  };
  const body = await request.arrayBuffer();

  let upstream: Response;
  try {
    upstream = await fetch(`${RUST_BASE_URL}/rpc`, { method: "POST", headers, body });
  } catch (e) {
    // Log the cause here; don't leak the loopback address to the browser.
    console.error("[seaquel] upstream unreachable:", e);
    return new Response(
      JSON.stringify({ code: "UPSTREAM_UNAVAILABLE", message: "upstream service unavailable" }),
      { status: 502, headers: { "content-type": "application/json" } },
    );
  }

  return new Response(upstream.body, {
    status: upstream.status,
    headers: {
      "content-type": upstream.headers.get("content-type") ?? "application/json",
    },
  });
};
