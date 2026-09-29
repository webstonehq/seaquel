/**
 * POST /api/rpc: forwards one workspace call to the Rust service's `/rpc`.
 *
 * Rust takes the user from `X-Seaquel-User` and trusts it, so this route
 * sets it from the session (`locals.user.id`) and never forwards one the
 * browser sent. `handleApiGate` in `hooks.server.ts` has already checked the
 * session, license and membership before this runs, and the request's
 * `Origin` must be one this install trusts (the page's own, or configured).
 *
 * `X-Seaquel-Origin`, the calling tab's id for its writes' `storageChanged`
 * events, is forwarded only when it is one value matching
 * `^[A-Za-z0-9_-]{1,64}$` (`$lib/server/write-origin`); otherwise it's
 * dropped and the call goes on without one.
 *
 * The body goes through as the bytes that arrived. It is never parsed and
 * re-stringified: `method` must stay before `params`, and stored JSON
 * columns must stay byte-identical. Rust's status and body (a `Response`, or
 * `RpcError` JSON) come back unchanged.
 *
 * The body may be up to `rpcBodyLimit` (20 MiB, or the operator's higher
 * `BODY_SIZE_LIMIT` up to 64 MiB; every other route keeps the operator's
 * limit, `shared/body-limit.js`). Past it the answer is 413 with
 * `INVALID_ARGUMENT`. A user's calls in flight hold at most
 * `RPC_USER_BUDGET` (40 MiB) of bodies; past it the answer is 429
 * `TOO_MANY_REQUESTS`, before the body is read.
 *
 * When the client's connection closes (the browser aborted the request, or
 * went away), the call to Rust is aborted, which drops it there: an atomic
 * apply rolls back, an in-order one stops at the statement in flight.
 * SvelteKit's `request.signal` doesn't say so once the body has been read,
 * so this watches the socket (`shared/client-request.js`).
 */

import { error } from "@sveltejs/kit";
import { originNotAllowed } from "$lib/server/api-gate";
import {
  BodyBudgetError,
  BodyTooLargeError,
  declaredLength,
  readLimitedBody,
  reserveRpcBody,
  rpcBodyLimit,
  rpcBodyTooLarge,
  rpcTooManyRequests,
} from "$lib/server/body-limit";
import { isOriginTrusted } from "$lib/server/origin";
import { WRITE_ORIGIN_HEADER, writeOrigin } from "$lib/server/write-origin";
import { currentClientRequest, onClientClose } from "$shared/client-request.js";
import type { RequestHandler } from "./$types";

/** What the route answers when the client left first: nobody reads it. */
const CLIENT_CLOSED = 499;

function clientClosed(): Response {
  return new Response(
    JSON.stringify({ code: "CANCELLED", message: "the client closed the request" }),
    { status: CLIENT_CLOSED, headers: { "content-type": "application/json" } },
  );
}

const RUST_BASE_URL = process.env.SEAQUEL_RUST_URL ?? "http://127.0.0.1:8788";

const USER_HEADER = "x-seaquel-user";

export const POST: RequestHandler = async ({ locals, request, platform }) => {
  if (!locals.user) throw error(401, "unauthorized");
  // CSRF: only the app's own pages (or a configured origin) may call. The
  // hook's Origin gate checks this for every /api mutation too; this keeps
  // the route safe on its own.
  if (!isOriginTrusted(request.headers.get("origin"), request.headers.get("host"))) {
    return originNotAllowed();
  }

  // Only these headers reach Rust: no cookies, no auth headers and no
  // client-sent X-Seaquel-User. The tab's origin goes only when it's
  // well-formed.
  const headers: Record<string, string> = {
    "content-type": "application/json",
    [USER_HEADER]: locals.user.id,
  };
  const origin = writeOrigin(request.headers);
  if (origin !== null) headers[WRITE_ORIGIN_HEADER] = origin;
  // The body's limit first (nothing read), then the user's budget: Node
  // keeps every body it reads until Rust answers. A slow sender holds its
  // reservation (its declared length, or what it has sent so far) until the
  // body arrives or Node's request timeout ends the request
  // (`server.requestTimeout`, 300 s by default), so one user can keep their
  // own budget full that long; bodies under `SMALL_CALL_BYTES` still get in.
  const limit = rpcBodyLimit(process.env);
  const declared = declaredLength(request);
  if (declared !== null && declared > limit) {
    await request.body?.cancel().catch(() => {});
    return rpcBodyTooLarge(limit);
  }
  const reservation = reserveRpcBody(locals.user.id, declared ?? 0);
  if (!reservation) {
    await request.body?.cancel().catch(() => {});
    return rpcTooManyRequests();
  }
  try {
    let body: Uint8Array<ArrayBuffer>;
    try {
      body = (await readLimitedBody(
        request,
        limit,
        declared === null ? (n) => reservation.grow(n) : undefined,
      )) as Uint8Array<ArrayBuffer>;
    } catch (e) {
      if (e instanceof BodyTooLargeError) return rpcBodyTooLarge(limit);
      if (e instanceof BodyBudgetError) return rpcTooManyRequests();
      throw e;
    }
    return await forward(headers, body, platform);
  } finally {
    reservation.release();
  }
};

/** Send `body` to Rust, aborting when the client's connection closes. */
async function forward(
  headers: Record<string, string>,
  body: Uint8Array<ArrayBuffer>,
  platform: App.Platform | undefined,
): Promise<Response> {
  // adapter-node's request, or the Vite dev plugin's.
  const client = (platform as { req?: Parameters<typeof onClientClose>[0] } | undefined)?.req;
  const aborted = new AbortController();
  const stopWatching = onClientClose(client ?? currentClientRequest(), () => aborted.abort());
  if (aborted.signal.aborted) return clientClosed();

  let upstream: Response;
  try {
    upstream = await fetch(`${RUST_BASE_URL}/rpc`, {
      method: "POST",
      headers,
      body,
      signal: aborted.signal,
    });
  } catch (e) {
    if (aborted.signal.aborted) return clientClosed();
    // Log the cause here; don't leak the loopback address to the browser.
    console.error("[seaquel] upstream unreachable:", e);
    return new Response(
      JSON.stringify({ code: "UPSTREAM_UNAVAILABLE", message: "upstream service unavailable" }),
      { status: 502, headers: { "content-type": "application/json" } },
    );
  } finally {
    // Rust has answered (headers arrive when the call is done). A client
    // that leaves while the body streams back cancels the body itself.
    stopWatching();
  }

  return new Response(upstream.body, {
    status: upstream.status,
    headers: {
      "content-type": upstream.headers.get("content-type") ?? "application/json",
    },
  });
}
