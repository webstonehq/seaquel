/**
 * Custom Node entrypoint for the tenant container.
 *
 * Responsibilities:
 *   1. Serve SvelteKit's adapter-node `handler` on the public port.
 *   2. Spawn `seaquel-server` (the Rust DB service) as a subprocess bound to
 *      loopback. Fate-share its lifecycle with this process.
 *   3. Proxy WebSocket upgrades for `/api/rpc/stream` to the Rust service's
 *      `/rpc/stream` (shared/rpc-stream-proxy.js). SvelteKit +server.ts
 *      handlers can't upgrade to WS, so this has to happen in the underlying
 *      http server.
 *
 * All run in one Node process, in one container, behind one public port.
 *
 * Nothing here forwards `/internal/*` (the Rust service's licensing routes,
 * which trust whatever user id they're given): the WebSocket proxy matches
 * `/api/rpc/stream` exactly and always dials `/rpc/stream`, and the
 * SvelteKit proxy (`/api/rpc`) forwards one fixed path.
 */

import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { moveBodyLimit } from "./shared/body-limit.js";
import { attachRpcStreamProxy, isRpcStreamPath } from "./shared/rpc-stream-proxy.js";
import { rustEnv } from "./shared/rust-env.js";
import { CLIENT_IP_HEADER, parseTrustedProxies, resolveClientIp } from "./shared/client-ip.js";

// Body limits (shared/body-limit.js): adapter-node applies one
// BODY_SIZE_LIMIT to every route, and /api/rpc needs 20 MiB (an apply at
// the web edit limits). So the operator's BODY_SIZE_LIMIT (512K by default)
// moves to SEAQUEL_BODY_SIZE_LIMIT, adapter-node's becomes Infinity, and
// hooks.server.ts enforces both per route. adapter-node reads the variable
// when its handler loads, hence the dynamic import after this.
moveBodyLimit(process.env);
const { handler } = await import("./build-web/handler.js");

const PORT = Number(process.env.PORT ?? 8787);
const RUST_BIN = process.env.SEAQUEL_RUST_BIN ?? "./seaquel-server";
const RUST_URL = process.env.SEAQUEL_RUST_URL ?? "http://127.0.0.1:8788";

// ---------------------------------------------------------------------------
// 1. Spawn the Rust subprocess.
// ---------------------------------------------------------------------------

// The per-boot secret the Rust service requires on /internal/* (licensing),
// as defense in depth on top of the loopback-only check: loopback alone
// doesn't prove the caller is this process (anything else running on the
// host, or a database feature that makes HTTP requests from inside the
// service, would also connect from loopback). A new one every start; the
// license client (src/lib/server/license-client.ts) reads it from this
// process's environment. Rust removes it from its own.
const INTERNAL_SECRET = randomBytes(32).toString("hex");
process.env.SEAQUEL_INTERNAL_SECRET = INTERNAL_SECRET;

// An allow-listed environment, never all of Node's: sqlx would take a
// self-hoster's PGPASSWORD, PG* defaults or ~/.pgpass for web users'
// connections (see shared/rust-env.js).
const rust = spawn(RUST_BIN, [], {
  stdio: "inherit",
  env: rustEnv(process.env, {
    BIND_ADDR: "127.0.0.1:8788",
    SEAQUEL_INTERNAL_SECRET: INTERNAL_SECRET,
  }),
});

rust.on("exit", (code) => {
  console.error(`[seaquel] seaquel-server exited with code ${code}; shutting down.`);
  process.exit(code ?? 1);
});

// Forward shutdown signals so the Rust process dies with us rather than
// becoming an orphan. Node's default behavior exits on these; we intercept
// so we can cleanly kill the child first.
for (const sig of ["SIGTERM", "SIGINT"]) {
  process.on(sig, () => {
    console.log(`[seaquel] received ${sig}, forwarding to seaquel-server.`);
    rust.kill(sig);
    // The child's `exit` handler above will then call process.exit().
  });
}

// ---------------------------------------------------------------------------
// 2. HTTP server with SvelteKit's handler.
// ---------------------------------------------------------------------------

// Overwrite CLIENT_IP_HEADER with the socket-derived client IP on every
// request, so a client can't choose its own rate-limit key. Better Auth
// (`advanced.ipAddress` in src/lib/server/auth.ts) and adapter-node's
// getClientAddress() (ADDRESS_HEADER in the Dockerfile) both read it.
// Behind a reverse proxy, list the proxy IPs/CIDRs in SEAQUEL_TRUSTED_PROXIES.
const trustedProxies = parseTrustedProxies(process.env.SEAQUEL_TRUSTED_PROXIES);

const server = createServer((req, res) => {
  req.headers[CLIENT_IP_HEADER] = resolveClientIp(req, trustedProxies);
  handler(req, res);
});

// ---------------------------------------------------------------------------
// 3. WebSocket proxy for /api/rpc/stream.
// ---------------------------------------------------------------------------
//
// One socket per browser session. The proxy resolves the session with a
// loopback fetch to /api/account/stream-access (server.js lives outside the
// SvelteKit bundle, so it can't call the auth API or the API gate itself;
// that route is gated by `handleApiGate` like every other /api route), sets
// X-Seaquel-User from it, drops anything the browser sent, and pipes frames
// both ways untouched. Rust checks the frames and owns the tenancy.

attachRpcStreamProxy(server, {
  rustUrl: RUST_URL,
  accessUrl: () => `http://127.0.0.1:${PORT}/api/account/stream-access`,
});

// Nothing else here upgrades: end any other upgrade at once instead of
// leaving the socket open.
server.on("upgrade", (req, socket) => {
  if (!isRpcStreamPath(req.url)) socket.destroy();
});

server.listen(PORT, () => {
  console.log(`[seaquel] seaquel-app listening on http://0.0.0.0:${PORT}`);
  console.log(`[seaquel] proxying /api/rpc and /api/rpc/stream to ${RUST_URL}`);
});
