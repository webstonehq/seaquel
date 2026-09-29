/**
 * The WebSocket proxy for `/api/rpc/stream` (phase 5a): one socket per
 * browser session, carrying that user's query streams and connection
 * events. SvelteKit routes can't upgrade to WebSocket, so this runs on the
 * underlying HTTP server: `server.js` in production, a Vite plugin in
 * `dev:web`.
 *
 * For each upgrade to exactly `/api/rpc/stream` it:
 *   1. resolves the session through `/api/account/stream-access` with the
 *      browser's cookie and `Origin`, and the upgrade's `Host` in
 *      {@link UPGRADE_HOST_HEADER}. That route is behind `handleApiGate`
 *      (session, license and membership, as every other `/api` route) and
 *      refuses an untrusted `Origin` (`isOriginTrusted`: the configured
 *      list, or the install's own origin, an `Origin` naming that `Host`).
 *      Anything but 200 with a user id refuses the upgrade with that
 *      route's status (401, 403 or 503; a lookup that takes over
 *      {@link ACCESS_TIMEOUT_MS} is 503);
 *   2. opens a WebSocket to the Rust service's `/rpc/stream` (a fixed URL:
 *      nothing from the browser's URL is forwarded) with `X-Seaquel-User` set
 *      from the session. No other browser header reaches Rust: no cookie, and
 *      no client-sent `X-Seaquel-User`. The one value taken from the URL is
 *      the page's write origin (`?origin=`, phase 5d: a browser can't set
 *      headers on a WebSocket), sent as `X-Seaquel-Origin` only when it
 *      matches {@link WRITE_ORIGIN_PATTERN} ({@link streamOrigin}); a run's
 *      history event carries it;
 *   3. pipes frames both ways as they are, Text as Text and Binary as Binary,
 *      without reading them. Rust checks every frame and enforces ownership.
 *      Browser frames are capped at {@link MAX_FRAME_BYTES}; Rust's aren't
 *      (it's trusted, and splits large batches itself);
 *   4. re-checks the session every {@link RECHECK_MS}: a 401 or 403 (signed
 *      out, license suspended, member revoked) closes the socket with 1008
 *      (policy), and an unreachable or failing gate with 1013 (try again
 *      later, so the client reconnects and is checked afresh). After
 *      {@link MAX_LIFETIME_MS} the socket closes with 1000 anyway, and the
 *      client reconnects and re-authorises;
 *   5. applies backpressure both ways: when the socket it writes to holds
 *      more than {@link PAUSE_BUFFERED_BYTES} unsent, it stops reading the
 *      other side (`pause()`), and reads again once those writes flush. A
 *      slow browser then stalls Rust's sender (and Rust's bounded outbox
 *      stalls the query), instead of growing Node's memory without limit
 *      (phase 5b probe, I2). When more than {@link MAX_BUFFERED_BYTES}
 *      still arrives from a side after it was paused, it closes both with
 *      1013; one huge frame (a wide row) and the frames behind it pass. Browser frames queued while the Rust socket
 *      opens count too (at most {@link MAX_PENDING_FRAMES}, and that many
 *      bytes);
 *   6. closes each side when the other closes, which cancels the socket's
 *      streams in Rust. Rust's close code and reason reach the browser
 *      (`TOO_MANY_SOCKETS` is 1013).
 *
 * Any other upgrade path is left alone here (`server.js` destroys it). Nothing
 * here can reach Rust's `/internal/*` or any path but `/rpc/stream`.
 *
 * Plain ESM JS (no TS) so `server.js` can import it directly at runtime
 * without a build step. The Dockerfile copies this directory into the image.
 */

import { WebSocket, WebSocketServer } from "ws";

/** The browser-facing path. */
export const RPC_STREAM_PATH = "/api/rpc/stream";

/** The Rust service's path. */
export const RUST_STREAM_PATH = "/rpc/stream";

/** The header Rust takes the user from. */
export const USER_HEADER = "x-seaquel-user";

/**
 * The header carrying the upgrade's own `Host` to `/api/account/stream-access`,
 * which compares it with `Origin` (the install's own origin is trusted).
 * `fetch` can't send a `Host` of its own choosing, and the lookup goes to
 * loopback, so the route would otherwise see `127.0.0.1:<port>`. Always set
 * from the upgrade request, never copied from a browser header.
 */
export const UPGRADE_HOST_HEADER = "x-seaquel-upgrade-host";

/** The largest browser frame: `MAX_FRAME_BYTES` in `seaquel-server`. */
export const MAX_FRAME_BYTES = 8 * 1024 * 1024;

/** Browser frames held while the Rust socket opens; more closes both. */
export const MAX_PENDING_FRAMES = 64;

/**
 * Unsent bytes on one side past which the proxy stops reading the other.
 * Rust splits batches at about 4 MiB, so this is a few frames.
 */
export const PAUSE_BUFFERED_BYTES = 16 * 1024 * 1024;

/** Unsent bytes on one side past which the proxy closes both with 1013. */
export const MAX_BUFFERED_BYTES = 64 * 1024 * 1024;

/** How long the Rust socket may take to open before the browser gets 1013. */
export const RUST_CONNECT_TIMEOUT_MS = 10_000;

/** How long a session lookup may take before it counts as unavailable. */
export const ACCESS_TIMEOUT_MS = 5_000;

/** How often an open socket's session is checked again. */
export const RECHECK_MS = 60_000;

/** The longest a socket stays open before the client must reconnect. */
export const MAX_LIFETIME_MS = 12 * 60 * 60 * 1000;

/** Close codes. */
const NORMAL = 1000;
const POLICY_VIOLATION = 1008;
const TRY_AGAIN_LATER = 1013;

/** The header Rust reads the page's write origin from. */
export const WRITE_ORIGIN_HEADER = "x-seaquel-origin";

/** 1–64 of `[A-Za-z0-9_-]`, as `/api/rpc` and Rust check it. */
export const WRITE_ORIGIN_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;

/**
 * The upgrade URL's `origin` query parameter when it is exactly one
 * well-formed value, else `null`.
 *
 * @param {string | undefined} url
 * @returns {string | null}
 */
export function streamOrigin(url) {
  try {
    const values = new URL(url ?? "", "http://localhost").searchParams.getAll("origin");
    return values.length === 1 && WRITE_ORIGIN_PATTERN.test(values[0]) ? values[0] : null;
  } catch {
    return null;
  }
}

/**
 * Whether `url` (a request URL) is the stream path, exactly. The query
 * string doesn't count.
 *
 * @param {string | undefined} url
 * @returns {boolean}
 */
export function isRpcStreamPath(url) {
  try {
    return new URL(url ?? "", "http://localhost").pathname === RPC_STREAM_PATH;
  } catch {
    return false;
  }
}

/**
 * The session behind a request's cookie and origin: `{ userId }`, or
 * `{ status }` (401 without a session, or the gate's own 401/403/503; a
 * lookup that fails or times out is 503).
 *
 * @param {{ cookie?: string, origin?: string, host?: string }} credentials
 * @param {string} accessUrl `http://…/api/account/stream-access` on this server.
 * @param {typeof fetch} [fetchImpl]
 * @returns {Promise<{ userId: string } | { status: number }>}
 */
export async function resolveSession(credentials, accessUrl, fetchImpl = fetch) {
  const { cookie, origin, host } = credentials;
  if (!cookie) return { status: 401 };
  /** @type {Record<string, string>} */
  const headers = { cookie };
  if (origin) headers.origin = origin;
  if (host) headers[UPGRADE_HOST_HEADER] = host;
  try {
    const res = await fetchImpl(accessUrl, {
      headers,
      signal: AbortSignal.timeout(ACCESS_TIMEOUT_MS),
    });
    if (!res.ok) {
      return { status: [401, 403, 503].includes(res.status) ? res.status : 401 };
    }
    const data = await res.json();
    const userId = data?.userId;
    // A partial answer never becomes a user.
    if (typeof userId !== "string" || userId.length === 0) return { status: 401 };
    return { userId };
  } catch {
    // Timed out or unreachable: not the user's fault, but no access either.
    return { status: 503 };
  }
}

/** @type {Record<number, string>} */
const REASONS = { 401: "Unauthorized", 403: "Forbidden", 503: "Service Unavailable" };

/**
 * An `http://host:port` base for reaching `server` from the same process.
 *
 * @param {import("node:net").Server} server An HTTP server (Vite's may be HTTP/2).
 * @returns {string}
 */
export function localBaseUrl(server) {
  const address = server.address();
  if (!address || typeof address === "string") {
    throw new Error("the server isn't listening on a TCP port");
  }
  let host = address.address;
  if (host === "::" || host === "0.0.0.0" || host === "") host = "127.0.0.1";
  else if (host.includes(":")) host = `[${host}]`;
  return `http://${host}:${address.port}`;
}

/**
 * Proxy `/api/rpc/stream` upgrades on `server` to Rust.
 *
 * @param {import("node:net").Server} server An HTTP server (Vite's may be HTTP/2).
 * @param {object} options
 * @param {string} options.rustUrl The Rust service's base URL (`http://127.0.0.1:8788`).
 * @param {() => string} [options.accessUrl] Where to resolve sessions; by
 *   default `/api/account/stream-access` on `server` itself.
 * @param {typeof fetch} [options.fetch]
 * @param {number} [options.recheckMs] Tests only: {@link RECHECK_MS}.
 * @param {number} [options.maxLifetimeMs] Tests only: {@link MAX_LIFETIME_MS}.
 * @param {number} [options.pauseBufferedBytes] Tests only: {@link PAUSE_BUFFERED_BYTES}.
 * @param {number} [options.maxBufferedBytes] Tests only: {@link MAX_BUFFERED_BYTES}.
 * @param {number} [options.rustConnectTimeoutMs] Tests only: {@link RUST_CONNECT_TIMEOUT_MS}.
 * @returns {WebSocketServer}
 */
export function attachRpcStreamProxy(server, options) {
  const rustWsUrl = options.rustUrl.replace(/^http/, "ws").replace(/\/+$/, "") + RUST_STREAM_PATH;
  const accessUrl =
    options.accessUrl ?? (() => `${localBaseUrl(server)}/api/account/stream-access`);
  const fetchImpl = options.fetch ?? fetch;
  const recheckMs = options.recheckMs ?? RECHECK_MS;
  const maxLifetimeMs = options.maxLifetimeMs ?? MAX_LIFETIME_MS;
  const limits = {
    pauseAt: options.pauseBufferedBytes ?? PAUSE_BUFFERED_BYTES,
    maxBuffered: options.maxBufferedBytes ?? MAX_BUFFERED_BYTES,
    connectTimeoutMs: options.rustConnectTimeoutMs ?? RUST_CONNECT_TIMEOUT_MS,
  };
  const wss = new WebSocketServer({ noServer: true, maxPayload: MAX_FRAME_BYTES });

  server.on("upgrade", async (req, socket, head) => {
    if (!isRpcStreamPath(req.url)) return; // Not ours: leave it to others.

    const credentials = {
      cookie: req.headers.cookie,
      origin: req.headers.origin,
      host: req.headers.host,
    };
    const session = await resolveSession(credentials, accessUrl(), fetchImpl);
    if (!("userId" in session)) {
      const status = session.status;
      socket.write(`HTTP/1.1 ${status} ${REASONS[status] ?? "Unauthorized"}\r\n\r\n`);
      socket.destroy();
      return;
    }
    const userId = session.userId;

    const origin = streamOrigin(req.url);
    wss.handleUpgrade(req, socket, head, (browserWs) => {
      const closeBoth = pipe(browserWs, rustWsUrl, userId, origin, limits);

      // Still allowed? Checked again on a timer, with the same credentials.
      const recheck = setInterval(async () => {
        const again = await resolveSession(credentials, accessUrl(), fetchImpl);
        if ("userId" in again && again.userId === userId) return;
        const status = "status" in again ? again.status : 403;
        if (status === 503) closeBoth(TRY_AGAIN_LATER, "access check unavailable");
        else closeBoth(POLICY_VIOLATION, "access revoked");
      }, recheckMs);
      const lifetime = setTimeout(
        () => closeBoth(NORMAL, "reconnect to re-authorise"),
        maxLifetimeMs,
      );
      browserWs.once("close", () => {
        clearInterval(recheck);
        clearTimeout(lifetime);
      });
    });
  });
  return wss;
}

/**
 * The bytes in a frame `ws` handed over.
 *
 * @param {import("ws").RawData} data
 * @returns {number}
 */
function byteLength(data) {
  if (Array.isArray(data)) return data.reduce((n, b) => n + b.length, 0);
  return data.byteLength;
}

/**
 * Whether a close code may be sent in a close frame (1005, 1006 and 1015
 * are only reported, never sent).
 *
 * @param {number} code
 */
function sendable(code) {
  return (
    [1000, 1001, 1002, 1003, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 1014].includes(code) ||
    (code >= 3000 && code <= 4999)
  );
}

/**
 * Pipe `browserWs` to a new Rust socket for `userId`.
 *
 * @param {WebSocket} browserWs
 * @param {string} rustWsUrl
 * @param {string} userId
 * @param {string | null} origin The page's checked write origin, if any.
 * @param {{ pauseAt: number, maxBuffered: number, connectTimeoutMs: number }} limits
 * @returns {(code?: number, reason?: string) => void} Closes both sides; the
 *   browser gets `code` and `reason`.
 */
function pipe(browserWs, rustWsUrl, userId, origin, limits) {
  // No `maxPayload`: Rust is trusted, and a result frame may pass the
  // browser's limit (Rust splits batches at about 4 MiB, but one wide row
  // can be larger). 0 is no limit in `ws`.
  /** @type {Record<string, string>} */
  const headers = { [USER_HEADER]: userId };
  if (origin !== null) headers[WRITE_ORIGIN_HEADER] = origin;
  const rustWs = new WebSocket(rustWsUrl, {
    headers,
    maxPayload: 0,
  });

  // Browser frames that arrive before the Rust socket opens, and their bytes.
  /** @type {Array<[import("ws").RawData, boolean]>} */
  const pending = [];
  let pendingBytes = 0;
  let closed = false;
  /** @type {ReturnType<typeof setTimeout> | undefined} */
  let connectTimer;
  /**
   * Bytes forwarded from each side since it was paused.
   * @type {Map<WebSocket, number>}
   */
  const late = new Map();

  /**
   * @param {number} [code]
   * @param {string} [reason]
   */
  const closeBoth = (code, reason) => {
    if (closed) return;
    closed = true;
    clearTimeout(connectTimer);
    pending.length = 0;
    pendingBytes = 0;
    const browserCode = code !== undefined && sendable(code) ? code : undefined;
    // `terminate` for a socket still connecting, which can't `close`.
    try {
      if (browserWs.readyState === WebSocket.OPEN) browserWs.close(browserCode, reason);
      else if (browserWs.readyState === WebSocket.CONNECTING) browserWs.terminate();
    } catch {
      /* ignore */
    }
    try {
      if (rustWs.readyState === WebSocket.OPEN) rustWs.close();
      else if (rustWs.readyState === WebSocket.CONNECTING) rustWs.terminate();
    } catch {
      /* ignore */
    }
  };

  /**
   * Send a frame read from `from` on to `to`, with backpressure: past
   * `limits.pauseAt` unsent bytes on `to`, stop reading `from` until a write
   * to `to` completes with the buffer back under half of it. The last
   * write's callback always comes, so a paused side is always resumed once
   * `to` drains.
   *
   * The hard cap is on what arrives after the pause, not on one frame: a
   * wide row (a 70 MB bytea) and the frames right behind it pass, as before
   * backpressure, while frames that keep arriving from a paused side past
   * `limits.maxBuffered` (read-ahead the pause didn't stop) close both. So
   * does a backlog past it when pausing never starts (`pauseAt` above it).
   *
   * @param {WebSocket} from
   * @param {WebSocket} to
   * @param {import("ws").RawData} data
   * @param {boolean} isBinary
   */
  const forward = (from, to, data, isBinary) => {
    const size = byteLength(data);
    const wasPaused = from.isPaused;
    to.send(data, { binary: isBinary }, () => {
      if (!closed && from.isPaused && to.bufferedAmount <= limits.pauseAt / 2) {
        late.set(from, 0);
        from.resume();
      }
    });
    const buffered = to.bufferedAmount;
    if (wasPaused) {
      const after = (late.get(from) ?? 0) + size;
      late.set(from, after);
      if (after > limits.maxBuffered) closeBoth(TRY_AGAIN_LATER, "too far behind");
    } else if (buffered - size > limits.maxBuffered) {
      closeBoth(TRY_AGAIN_LATER, "too far behind");
    } else if (buffered > limits.pauseAt) {
      late.set(from, 0);
      from.pause();
    }
  };

  // A Rust socket that never opens would hold the browser (and its queued
  // frames) forever: give up with 1013 so the client reconnects.
  connectTimer = setTimeout(() => {
    if (rustWs.readyState !== WebSocket.OPEN) closeBoth(TRY_AGAIN_LATER, "upstream unavailable");
  }, limits.connectTimeoutMs);

  rustWs.on("open", () => {
    clearTimeout(connectTimer);
    if (closed) {
      rustWs.close();
      return;
    }
    // Through `forward`, so the queued bytes count toward backpressure;
    // its send callbacks resume the browser if the queue paused it.
    const queued = pending.splice(0);
    pendingBytes = 0;
    for (const [data, isBinary] of queued) {
      if (closed) return;
      forward(browserWs, rustWs, data, isBinary);
    }
  });

  // `isBinary` is authoritative: `ws` hands over a Buffer either way, and
  // Rust answers a Binary frame with an error.
  browserWs.on("message", (data, isBinary) => {
    if (closed) return;
    if (rustWs.readyState === WebSocket.OPEN) forward(browserWs, rustWs, data, isBinary);
    else if (
      pending.length < MAX_PENDING_FRAMES &&
      pendingBytes + byteLength(data) <= limits.maxBuffered
    ) {
      pending.push([data, isBinary]);
      pendingBytes += byteLength(data);
      // Stop reading the browser until Rust opens and takes the queue.
      if (pendingBytes > limits.pauseAt && !browserWs.isPaused) {
        late.set(browserWs, 0);
        browserWs.pause();
      }
    } else closeBoth(TRY_AGAIN_LATER, "too far behind");
  });
  rustWs.on("message", (data, isBinary) => {
    if (!closed && browserWs.readyState === WebSocket.OPEN) {
      forward(rustWs, browserWs, data, isBinary);
    }
  });

  browserWs.on("close", () => closeBoth());
  // Rust's code and reason (e.g. 1013 TOO_MANY_SOCKETS) go to the browser.
  rustWs.on("close", (code, reason) => closeBoth(code, reason.toString("utf8")));
  browserWs.on("error", () => closeBoth());
  rustWs.on("error", (e) => {
    // An error after the browser left (a socket still connecting is
    // terminated) is expected.
    if (!closed) console.error("[seaquel] rpc stream upstream error:", e);
    closeBoth(1011, "upstream error");
  });
  return closeBoth;
}
