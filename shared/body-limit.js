/**
 * Request body limits for the web build (phase 5c probe, I2).
 *
 * adapter-node applies one `BODY_SIZE_LIMIT` to every route, 512 KB unless
 * the operator sets it. `/api/rpc` needs more: an apply at the web edit
 * limits (`WEB_EDIT_LIMITS`: 16 MiB of values and 2 MiB of SQL) is about
 * 18 MiB of JSON. So `server.js` moves the operator's value (or the 512K
 * default) to `SEAQUEL_BODY_SIZE_LIMIT`, sets adapter-node's to `Infinity`,
 * and `hooks.server.ts` enforces both: `RPC_BODY_LIMIT` on `/api/rpc`, the
 * operator's limit everywhere else, refusing by `Content-Length` before
 * anything is read and counting the bytes of a body sent without one.
 *
 * Plain ESM JS (no TS) so `server.js` can import it without a build step.
 */

/** The one route with its own limit. */
export const RPC_PATH = "/api/rpc";

/**
 * `/api/rpc`'s limit: 16 MiB of values, 2 MiB of SQL and room for JSON. An
 * operator's higher `BODY_SIZE_LIMIT` raises it, up to 64 MiB.
 */
export const RPC_BODY_LIMIT = 20 * 1024 * 1024;

/** adapter-node's default, kept for every other route. */
export const DEFAULT_BODY_SIZE_LIMIT = "512K";

/** Where `server.js` puts the operator's `BODY_SIZE_LIMIT` for the hook. */
export const BODY_LIMIT_ENV = "SEAQUEL_BODY_SIZE_LIMIT";

/**
 * A limit as adapter-node's `parse_as_bytes` reads `BODY_SIZE_LIMIT`,
 * exactly: `Number()` of the value, times 1024, 1024² or 1024³ for a `K`,
 * `M` or `G` suffix (either case). So `1e6` and `Infinity` work, and a value
 * that isn't a number is `NaN`.
 * @param {string} value
 * @returns {number}
 */
export function parseBodyLimit(value) {
  const multiplier =
    /** @type {Record<string, number>} */ ({ K: 1024, M: 1024 * 1024, G: 1024 * 1024 * 1024 })[
      value[value.length - 1]?.toUpperCase()
    ] ?? 1;
  return Number(multiplier != 1 ? value.substring(0, value.length - 1) : value) * multiplier;
}

/**
 * `server.js`'s step before it loads adapter-node's handler: the operator's
 * `BODY_SIZE_LIMIT` (or the default) moves to `SEAQUEL_BODY_SIZE_LIMIT`,
 * and adapter-node's becomes `Infinity`, so the hook decides per route.
 * Throws for a value that isn't a limit, as adapter-node does.
 * @param {Record<string, string | undefined>} env
 * @returns {number} the limit for every route but `/api/rpc`
 */
export function moveBodyLimit(env) {
  const value = env.BODY_SIZE_LIMIT ?? DEFAULT_BODY_SIZE_LIMIT;
  const limit = parseBodyLimit(value);
  if (Number.isNaN(limit)) {
    throw new Error(`Invalid BODY_SIZE_LIMIT: '${value}'. Please provide a numeric value.`);
  }
  env[BODY_LIMIT_ENV] = value;
  env.BODY_SIZE_LIMIT = "Infinity";
  return limit;
}

/** The most `/api/rpc` takes whatever the operator sets: Rust's own limit. */
export const RPC_BODY_LIMIT_MAX = 64 * 1024 * 1024;

/**
 * The operator's limit for every route but `/api/rpc`:
 * `SEAQUEL_BODY_SIZE_LIMIT`, or the 512K default when it's unset (as in the
 * Vite dev server) or not a limit.
 * @param {Record<string, string | undefined>} env
 * @returns {number}
 */
function operatorLimit(env) {
  const limit = parseBodyLimit(env[BODY_LIMIT_ENV] ?? DEFAULT_BODY_SIZE_LIMIT);
  return Number.isNaN(limit) ? parseBodyLimit(DEFAULT_BODY_SIZE_LIMIT) : limit;
}

/**
 * `/api/rpc`'s limit: `RPC_BODY_LIMIT`, or the operator's when that is
 * higher, up to Rust's 64 MiB.
 * @param {Record<string, string | undefined>} env
 * @returns {number}
 */
export function rpcBodyLimit(env) {
  return Math.max(RPC_BODY_LIMIT, Math.min(operatorLimit(env), RPC_BODY_LIMIT_MAX));
}

/**
 * The limit for a request to `pathname`: `rpcBodyLimit` on `/api/rpc`, the
 * operator's everywhere else.
 * @param {string} pathname
 * @param {Record<string, string | undefined>} env
 * @returns {number}
 */
export function bodyLimitFor(pathname, env) {
  return pathname === RPC_PATH ? rpcBodyLimit(env) : operatorLimit(env);
}
