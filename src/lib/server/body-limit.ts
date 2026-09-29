/**
 * Per-route request body limits (phase 5c probe, I2); the rules are in
 * `shared/body-limit.js`. `hooks.server.ts` refuses a body whose
 * `Content-Length` is past its route's limit before anything reads it, and
 * counts the bytes of one sent without it. `/api/rpc` reads its body with
 * `readLimitedBody` and answers `rpcBodyTooLarge`, an `RpcError` the GUI
 * shows through `errorText`.
 *
 * `/api/rpc` also holds each user's bodies in flight to `RPC_USER_BUDGET`
 * (`reserveRpcBody`), since Node keeps every body it reads until Rust
 * answers: past it, 429 `TOO_MANY_REQUESTS` before reading.
 */

import { error } from "@sveltejs/kit";
import { bodyLimitFor, RPC_BODY_LIMIT, RPC_PATH, rpcBodyLimit } from "$shared/body-limit.js";

export { RPC_BODY_LIMIT, RPC_PATH, rpcBodyLimit };

const MIB = 1024 * 1024;

function limitText(limit: number): string {
  return limit % MIB === 0 ? `${limit / MIB} MiB (${limit} bytes)` : `${limit} bytes`;
}

/** How many bytes of bodies one user's `/api/rpc` calls in flight may hold (probe review, M4). */
export const RPC_USER_BUDGET = 40 * MIB;

/** Each user's bytes of `/api/rpc` bodies in flight. */
const inFlight = new Map<string, number>();

/** A call's share of its user's budget; release it on every path. */
export interface RpcReservation {
  /** Take `bytes` more (a body counted as it arrives); `false` past the budget. */
  grow(bytes: number): boolean;
  release(): void;
}

/**
 * A body under this always gets in (it still counts toward the budget), so
 * a user's own large applies never make their storage saves and history
 * appends fail with 429.
 */
export const SMALL_CALL_BYTES = 64 * 1024;

function admits(held: number, bytes: number): boolean {
  // A lone call always runs, so a body up to the route's limit still fits.
  return bytes < SMALL_CALL_BYTES || held === 0 || held + bytes <= RPC_USER_BUDGET;
}

/**
 * Reserve `bytes` (the declared `Content-Length`, or 0 for a body counted as
 * it arrives) of `userId`'s budget, or `null` when their calls in flight
 * already hold too much.
 */
export function reserveRpcBody(userId: string, bytes: number): RpcReservation | null {
  const held = inFlight.get(userId) ?? 0;
  if (!admits(held, bytes)) return null;
  let mine = 0;
  let released = false;
  const add = (n: number) => {
    mine += n;
    inFlight.set(userId, (inFlight.get(userId) ?? 0) + n);
  };
  add(bytes);
  return {
    grow(n) {
      if (released) return false;
      const others = (inFlight.get(userId) ?? 0) - mine;
      const small = mine + n < SMALL_CALL_BYTES;
      if (!small && others > 0 && others + mine + n > RPC_USER_BUDGET) return false;
      add(n);
      return true;
    },
    release() {
      if (released) return;
      released = true;
      const left = (inFlight.get(userId) ?? 0) - mine;
      if (left > 0) inFlight.set(userId, left);
      else inFlight.delete(userId);
    },
  };
}

/** `userId`'s bytes of `/api/rpc` bodies in flight (tests). */
export function rpcBytesInFlight(userId: string): number {
  return inFlight.get(userId) ?? 0;
}

/** `/api/rpc`'s answer past `RPC_USER_BUDGET`: 429 with an `RpcError`; nothing ran. */
export function rpcTooManyRequests(): Response {
  return new Response(
    JSON.stringify({
      code: "TOO_MANY_REQUESTS",
      message:
        "Too many large requests are running at once. Wait for them to finish and try again.",
    }),
    { status: 429, headers: { "content-type": "application/json" } },
  );
}

/** A body past its user's in-flight budget while it arrived, from `readLimitedBody`. */
export class BodyBudgetError extends Error {
  constructor() {
    super("Too many large requests are running at once.");
    this.name = "BodyBudgetError";
  }
}

/** A body past its limit, from `readLimitedBody`. */
export class BodyTooLargeError extends Error {
  constructor(readonly limit: number) {
    super(`The request body is larger than ${limitText(limit)}.`);
    this.name = "BodyTooLargeError";
  }
}

/** `request`'s `Content-Length`, or `null` when it has none. */
export function declaredLength(request: Request): number | null {
  const header = request.headers.get("content-length");
  if (header === null) return null;
  const n = Number(header);
  return Number.isFinite(n) ? n : null;
}

/**
 * `request`'s body, at most `limit` bytes: refused by its `Content-Length`
 * before reading, else counted while it streams in and cancelled at the
 * first byte past the limit. Throws `BodyTooLargeError`. With `onChunk`,
 * each chunk's size is offered to it first, and a `false` cancels the read
 * with `BodyBudgetError`.
 */
export async function readLimitedBody(
  request: Request,
  limit: number,
  onChunk?: (bytes: number) => boolean,
): Promise<Uint8Array> {
  const declared = declaredLength(request);
  if (declared !== null && declared > limit) {
    await request.body?.cancel().catch(() => {});
    throw new BodyTooLargeError(limit);
  }
  if (!request.body) return new Uint8Array(0);
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > limit) {
      await reader.cancel().catch(() => {});
      throw new BodyTooLargeError(limit);
    }
    if (onChunk && !onChunk(value.byteLength)) {
      await reader.cancel().catch(() => {});
      throw new BodyBudgetError();
    }
    chunks.push(value);
  }
  const body = new Uint8Array(size);
  let at = 0;
  for (const chunk of chunks) {
    body.set(chunk, at);
    at += chunk.byteLength;
  }
  return body;
}

/** `/api/rpc`'s answer to a body past its `limit` (`rpcBodyLimit`): 413 with an `RpcError`. */
export function rpcBodyTooLarge(limit: number): Response {
  return new Response(
    JSON.stringify({
      code: "INVALID_ARGUMENT",
      message: `One request can send at most ${limitText(limit)}. Apply the changes in parts.`,
    }),
    { status: 413, headers: { "content-type": "application/json" } },
  );
}

/** Any other route's answer, shaped like SvelteKit's own. */
function tooLarge(limit: number): Response {
  return new Response(
    JSON.stringify({ message: `Payload Too Large: the limit is ${limitText(limit)}.` }),
    { status: 413, headers: { "content-type": "application/json" } },
  );
}

/** SvelteKit's `HttpError`, which a route's failed body read turns into its status. */
function httpError(status: number, message: string): unknown {
  try {
    error(status, message);
  } catch (e) {
    return e;
  }
}

/**
 * The hook's step: a 413 `Response` for a body declared past the route's
 * limit, else the request to go on with. Past `/api/rpc`'s limit that is
 * `rpcBodyTooLarge`. Elsewhere a body sent without `Content-Length` is
 * counted as it streams, and a read past the limit fails with a 413
 * `HttpError`. `/api/rpc` counts its own (`readLimitedBody`).
 */
export function limitRequestBody(
  request: Request,
  pathname: string,
  env: Record<string, string | undefined>,
): Response | Request {
  if (!request.body) return request;
  const limit = bodyLimitFor(pathname, env);
  const declared = declaredLength(request);
  if (declared !== null && declared > limit) {
    return pathname === RPC_PATH ? rpcBodyTooLarge(limit) : tooLarge(limit);
  }
  if (pathname === RPC_PATH || declared !== null || limit === Infinity) return request;
  let size = 0;
  const counted = request.body.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        size += chunk.byteLength;
        if (size > limit) {
          controller.error(httpError(413, `Payload Too Large: the limit is ${limitText(limit)}.`));
          return;
        }
        controller.enqueue(chunk);
      },
    }),
  );
  return new Request(request, { body: counted, duplex: "half" } as RequestInit);
}
