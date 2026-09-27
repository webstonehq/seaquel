/**
 * Which browser origins this install accepts state-changing and
 * cookie-authenticated requests from (CSRF and cross-site WebSocket
 * protection). Better Auth takes the same list (`trustedOrigins` in
 * `auth.ts`); `/api/signup`, `/api/airgap/bundle`, the `/api/*` Origin gate
 * in `hooks.server.ts` and `/api/account/stream-access` check it with
 * {@link isOriginTrusted}.
 *
 * An origin is trusted when it is:
 *   - listed in `SEAQUEL_TRUSTED_ORIGINS` (comma-separated), or is the origin
 *     of `BETTER_AUTH_URL` or of adapter-node's `ORIGIN`;
 *   - the install's own origin, only while neither `BETTER_AUTH_URL` nor
 *     `ORIGIN` is set and only for a loopback or IP-address host: the
 *     `Origin` names the host the request was sent to (its `Host` header,
 *     see {@link hostFallbackOrigin}). It is what makes a `docker run`
 *     reached at `http://localhost:8787` or `http://192.168.1.20:8787` work
 *     with nothing configured. A domain name is never trusted this way: a
 *     DNS-rebinding page on `evil.example` reaches the install with
 *     `evil.example` in both headers, and would otherwise pass (and reach
 *     the first-run `/api/signup` and `/api/airgap/bundle`, which need no
 *     session). An install on a domain sets `BETTER_AUTH_URL`, `ORIGIN` or
 *     `SEAQUEL_TRUSTED_ORIGINS`;
 *   - in a dev build only (`npm run dev:web`), one of {@link DEV_ORIGINS}.
 *     A production build never trusts them, so another local service on one
 *     of those ports can't act for a signed-in user.
 */

import { dev } from "$app/environment";

/** The Vite dev server's and a local `start:web`'s origins, dev builds only. */
export const DEV_ORIGINS: readonly string[] = [
  "http://localhost:5173",
  "http://localhost:8787",
  "http://127.0.0.1:5173",
  "http://127.0.0.1:8787",
];

/** The origin of an http(s) URL, or `null`. */
function httpOrigin(value: string | undefined): string | null {
  if (!value) return null;
  try {
    const url = new URL(value);
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    return url.origin;
  } catch {
    return null;
  }
}

/** The configured origins, plus {@link DEV_ORIGINS} in a dev build. */
export function getTrustedOrigins(): string[] {
  const configured =
    process.env.SEAQUEL_TRUSTED_ORIGINS?.split(",")
      .map((s) => s.trim())
      .filter(Boolean) ?? [];
  const own = [httpOrigin(process.env.BETTER_AUTH_URL), httpOrigin(process.env.ORIGIN)].filter(
    (o): o is string => o !== null,
  );
  return [...configured, ...own, ...(dev ? DEV_ORIGINS : [])];
}

/**
 * `origin` when it is an http(s) origin whose host (name and port) is
 * `host`, the `Host` the request was sent to; otherwise `null`.
 *
 * The scheme isn't compared: behind a TLS-terminating proxy the server
 * can't tell http from https, and a page served over plain http on our own
 * host already needs control of that host or the network.
 */
export function sameHostOrigin(
  origin: string | null | undefined,
  host: string | null | undefined,
): string | null {
  if (!origin || !host) return null;
  let url: URL;
  try {
    url = new URL(origin);
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  // An Origin header is exactly scheme://host[:port], nothing more.
  if (url.origin !== origin) return null;
  let requestHost: URL;
  try {
    requestHost = new URL(`${url.protocol}//${host}`);
  } catch {
    return null;
  }
  if (requestHost.pathname !== "/" || requestHost.username || requestHost.search) return null;
  return requestHost.host === url.host ? origin : null;
}

/**
 * Whether `host` (a `Host` header: name and optional port) is `localhost`
 * or an IP address, which DNS rebinding can't put in a browser's `Host`.
 */
export function isIpOrLoopbackHost(host: string | null | undefined): boolean {
  if (!host) return false;
  let hostname: string;
  try {
    hostname = new URL(`http://${host}`).hostname;
  } catch {
    return false;
  }
  if (hostname === "localhost") return true;
  // The URL parser has normalized IPv4 forms to dotted decimal.
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(hostname)) return true;
  return hostname.startsWith("[") && hostname.endsWith("]");
}

/**
 * {@link sameHostOrigin}, as a trusted origin: only while neither
 * `BETTER_AUTH_URL` nor `ORIGIN` names the install (then that is its
 * origin), and only for a loopback or IP-address `host`
 * ({@link isIpOrLoopbackHost}), never a domain name, which DNS rebinding
 * could supply.
 */
export function hostFallbackOrigin(
  origin: string | null | undefined,
  host: string | null | undefined,
): string | null {
  if (httpOrigin(process.env.BETTER_AUTH_URL) || httpOrigin(process.env.ORIGIN)) return null;
  if (!isIpOrLoopbackHost(host)) return null;
  return sameHostOrigin(origin, host);
}

/**
 * Whether a request's `Origin` is trusted: configured (see
 * {@link getTrustedOrigins}), or the install's own origin when `host` (the
 * request's `Host`) is given and {@link hostFallbackOrigin} allows it. A missing, empty or `"null"` Origin is never
 * trusted: browsers send one on every cross-site request and on every POST,
 * PUT, PATCH and DELETE, so a mutation without one isn't from a page.
 */
export function isOriginTrusted(origin: string | null | undefined, host?: string | null): boolean {
  if (!origin) return false;
  if (getTrustedOrigins().includes(origin)) return true;
  return hostFallbackOrigin(origin, host) !== null;
}

/**
 * Better Auth's `trustedOrigins`: {@link getTrustedOrigins}, plus the
 * request's own origin when {@link hostFallbackOrigin} allows it. Better Auth
 * calls this with no request at startup and with the request on every call
 * whose Origin it checks.
 */
export function betterAuthTrustedOrigins(request?: Request): string[] {
  const origins = getTrustedOrigins();
  const own = request
    ? hostFallbackOrigin(request.headers.get("origin"), request.headers.get("host"))
    : null;
  return own ? [...origins, own] : origins;
}
