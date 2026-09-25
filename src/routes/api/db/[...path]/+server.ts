/**
 * HTTP proxy for /api/db/*.
 *
 * Beyond plain forwarding, this layer enforces tenant isolation on
 * `connection_id`. The Rust loopback service is oblivious to users — it keys
 * every connection under a single map by UUID, so without a gate here, any
 * authenticated caller could query any other caller's connection by guessing
 * (or leaking) its UUID.
 *
 * Scheme: the browser-facing connection_id is `${userId}:${rustId}`. On
 * `/api/db/connect`, we rewrite the Rust response to prepend the prefix. On
 * every other path carrying a connection_id in its body (disconnect, query,
 * execute, transaction, engine), we verify the prefix matches the caller and
 * strip it before forwarding. WebSocket stream has the same guarantee in server.js.
 *
 * The WebSocket upgrade for /api/db/stream is NOT handled here — SvelteKit
 * routes can't handle WS upgrades. It's intercepted by server.js before
 * SvelteKit's request handler sees it.
 */

import { error } from "@sveltejs/kit";
import { scopeConnectionId, unscopeConnectionId } from "$shared/connection-scope.js";
import type { RequestHandler } from "./$types";

const RUST_BASE_URL = process.env.SEAQUEL_RUST_URL ?? "http://127.0.0.1:8788";

// Paths whose request body carries a connection_id we must validate + strip.
const VALIDATED_PATHS = new Set(["disconnect", "query", "execute", "transaction", "engine"]);

const forward: RequestHandler = async ({ locals, params, request, url }) => {
  if (!locals.user) throw error(401, "unauthorized");

  const userId = locals.user.id;
  const upstreamPath = params.path ?? "";
  const upstreamUrl = `${RUST_BASE_URL}/api/db/${upstreamPath}${url.search}`;
  const method = request.method;

  const validatesIncoming = VALIDATED_PATHS.has(upstreamPath);
  const rewritesResponse = upstreamPath === "connect";

  // Forward only content-type; host/cookie/auth headers must not leak to the
  // loopback Rust service (it has no concept of a user).
  const headers: Record<string, string> = {
    "content-type": request.headers.get("content-type") ?? "application/json",
  };

  let upstreamBody: BodyInit | undefined;
  if (method !== "GET" && method !== "HEAD") {
    if (validatesIncoming) {
      let body: Record<string, unknown>;
      try {
        body = (await request.json()) as Record<string, unknown>;
      } catch {
        throw error(400, "invalid json body");
      }
      const cid = body.connection_id;
      if (typeof cid !== "string") {
        throw error(400, "missing connection_id");
      }
      const rustId = unscopeConnectionId(userId, cid);
      if (rustId === null) {
        throw error(403, "connection does not belong to this user");
      }
      body.connection_id = rustId;
      upstreamBody = JSON.stringify(body);
    } else {
      upstreamBody = await request.arrayBuffer();
    }
  }

  let upstream: Response;
  try {
    upstream = await fetch(upstreamUrl, { method, headers, body: upstreamBody });
  } catch (e) {
    // Log the raw cause server-side; return a generic message so we don't
    // leak loopback addresses or node-level errors to the browser.
    console.error("[seaquel] upstream unreachable:", e);
    throw error(502, "upstream service unavailable");
  }

  if (rewritesResponse && upstream.ok) {
    const text = await upstream.text();
    try {
      const parsed = JSON.parse(text) as Record<string, unknown>;
      if (typeof parsed.connection_id === "string") {
        parsed.connection_id = scopeConnectionId(userId, parsed.connection_id);
      }
      return new Response(JSON.stringify(parsed), {
        status: upstream.status,
        headers: { "content-type": "application/json" },
      });
    } catch {
      // Shouldn't happen (Rust always returns JSON on success) — forward raw.
      return new Response(text, {
        status: upstream.status,
        headers: { "content-type": "application/json" },
      });
    }
  }

  return new Response(upstream.body, {
    status: upstream.status,
    headers: {
      "content-type": upstream.headers.get("content-type") ?? "application/json",
    },
  });
};

export const GET = forward;
export const POST = forward;
export const PUT = forward;
export const DELETE = forward;
export const PATCH = forward;
