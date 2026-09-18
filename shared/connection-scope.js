/**
 * Tenant-scoping for browser-facing `connection_id` values.
 *
 * The Rust loopback service keys connections under a single map by UUID and
 * has no concept of users. Without a gate in front of it, any authenticated
 * caller could query any other caller's connection by guessing (or leaking)
 * its UUID. We solve this by prefixing every browser-visible connection id
 * with `${userId}:` and stripping that prefix before forwarding to Rust.
 *
 * Two transports rely on this scheme — the HTTP route handler
 * (`src/routes/api/db/[...path]/+server.ts`) and the WebSocket upgrade proxy
 * in `server.js`. Both must agree on the format byte-for-byte, so this lives
 * in a single shared module.
 *
 * Plain ESM JS (no TS) so `server.js` can import it directly at runtime
 * without a build step. The Dockerfile copies this directory into the image.
 */

/**
 * @param {string} userId
 * @param {string} rustId
 * @returns {string}
 */
export function scopeConnectionId(userId, rustId) {
  return `${userId}:${rustId}`;
}

/**
 * Verifies `scoped` belongs to `userId` and returns the underlying Rust id.
 * Returns `null` when the prefix is missing or wrong — the caller is expected
 * to translate that into a 403 / WS close.
 *
 * @param {string} userId
 * @param {string} scoped
 * @returns {string | null}
 */
export function unscopeConnectionId(userId, scoped) {
  const prefix = `${userId}:`;
  if (!scoped.startsWith(prefix)) return null;
  return scoped.slice(prefix.length);
}
