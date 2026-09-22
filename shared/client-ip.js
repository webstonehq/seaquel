/**
 * Client IP resolution for the tenant container.
 *
 * Better Auth's rate limiter and adapter-node's `getClientAddress()` only see
 * request headers, and `X-Forwarded-For` is attacker-controlled unless it was
 * written by a proxy we trust. `server.js` owns the socket, so it resolves the
 * real client IP here and overwrites `CLIENT_IP_HEADER` on every request.
 * Everything downstream keys on that header.
 *
 * Without `SEAQUEL_TRUSTED_PROXIES`, the socket address is used and
 * `X-Forwarded-For` is ignored. With it, `X-Forwarded-For` is walked from the
 * right, skipping trusted hops, and the first untrusted hop is the client.
 *
 * Plain ESM JS (no TS) so `server.js` can import it directly at runtime
 * without a build step. The Dockerfile copies this directory into the image.
 */

import { BlockList, isIP } from "node:net";

export const CLIENT_IP_HEADER = "x-seaquel-client-ip";

/** @param {string} ip */
function normalize(ip) {
  return ip.startsWith("::ffff:") && isIP(ip.slice(7)) === 4 ? ip.slice(7) : ip;
}

/** @param {string} ip */
function family(ip) {
  return isIP(ip) === 6 ? "ipv6" : "ipv4";
}

/**
 * Parses a comma-separated list of IPs and CIDR ranges.
 * @param {string | undefined} value
 * @returns {BlockList}
 */
export function parseTrustedProxies(value) {
  const list = new BlockList();
  for (const entry of (value ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean)) {
    const [addr, prefix] = entry.split("/");
    if (!isIP(addr)) throw new Error(`invalid trusted proxy: ${entry}`);
    if (prefix === undefined) {
      list.addAddress(addr, family(addr));
    } else {
      const bits = Number(prefix);
      const max = isIP(addr) === 6 ? 128 : 32;
      if (!Number.isInteger(bits) || bits < 0 || bits > max) {
        throw new Error(`invalid trusted proxy: ${entry}`);
      }
      list.addSubnet(addr, bits, family(addr));
    }
  }
  return list;
}

/**
 * @param {{ socket: { remoteAddress?: string }, headers: Record<string, string | string[] | undefined> }} req
 * @param {BlockList} trusted
 * @returns {string}
 */
export function resolveClientIp(req, trusted) {
  const isTrusted = (/** @type {string} */ ip) => isIP(ip) !== 0 && trusted.check(ip, family(ip));

  let ip = normalize(req.socket.remoteAddress ?? "");
  if (!isTrusted(ip)) return ip;

  const header = req.headers["x-forwarded-for"];
  const hops = (Array.isArray(header) ? header.join(",") : (header ?? ""))
    .split(",")
    .map((s) => normalize(s.trim()))
    .filter(Boolean);
  for (let i = hops.length - 1; i >= 0; i--) {
    if (!isIP(hops[i])) break;
    ip = hops[i];
    if (!isTrusted(ip)) break;
  }
  return ip;
}
