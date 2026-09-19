/**
 * Custom Node entrypoint for the tenant container.
 *
 * Responsibilities:
 *   1. Serve SvelteKit's adapter-node `handler` on the public port.
 *   2. Spawn `seaquel-server` (the Rust DB service) as a subprocess bound to
 *      loopback. Fate-share its lifecycle with this process.
 *   3. Intercept WebSocket upgrades for `/api/db/stream` and proxy them to
 *      the Rust service. SvelteKit +server.ts handlers can't upgrade to WS,
 *      so this has to happen in the underlying http server.
 *
 * All run in one Node process, in one container, behind one public port.
 */

import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { WebSocketServer, WebSocket } from "ws";
import { handler } from "./build-web/handler.js";
import { unscopeConnectionId } from "./shared/connection-scope.js";

const PORT = Number(process.env.PORT ?? 8787);
const RUST_BIN = process.env.SEAQUEL_RUST_BIN ?? "./seaquel-server";
const RUST_URL = process.env.SEAQUEL_RUST_URL ?? "http://127.0.0.1:8788";
const RUST_WS_URL = RUST_URL.replace(/^http/, "ws") + "/api/db/stream";

// ---------------------------------------------------------------------------
// 1. Spawn the Rust subprocess.
// ---------------------------------------------------------------------------

const rust = spawn(RUST_BIN, [], {
  stdio: "inherit",
  env: {
    ...process.env,
    BIND_ADDR: "127.0.0.1:8788",
  },
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

const server = createServer(handler);

// ---------------------------------------------------------------------------
// 3. WebSocket proxy for /api/db/stream.
// ---------------------------------------------------------------------------
//
// Contract: the browser connects to `wss://{tenant}.seaquel.app/api/db/stream`,
// sends its request as the first Text frame, and the Rust service streams
// batches back. Our job here is just to:
//   - check the session cookie (Phase 3c — stubbed accept-all in 3b)
//   - open a second WebSocket to the Rust loopback endpoint
//   - pipe bytes in both directions
//   - close both sockets when either side closes.

const wss = new WebSocketServer({ noServer: true });

// Resolve the session cookie to `{ userId }` via Better Auth. server.js
// lives outside the SvelteKit bundle, so we can't call auth.api.getSession()
// directly — instead, do a loopback fetch to the app's own
// /api/auth/get-session endpoint. Two hops in the same process (tens of
// microseconds) is an acceptable tax for avoiding a fragile cross-bundle
// import.
//
// Returns `null` if the session is missing, invalid, or incomplete.
async function resolveSession(req) {
  const cookie = req.headers.cookie ?? "";
  if (!cookie) return null;

  try {
    const res = await fetch(`http://127.0.0.1:${PORT}/api/auth/get-session`, {
      headers: { cookie },
    });
    if (!res.ok) return null;
    const data = await res.json();
    const userId = data?.user?.id;
    // Reject partial session objects — `userId` is what the tenant-scope
    // check below depends on. Without it, we'd let a malformed session
    // through and silently cross tenant boundaries.
    if (typeof userId !== "string" || userId.length === 0) return null;
    return { userId };
  } catch {
    return null;
  }
}

server.on("upgrade", async (req, socket, head) => {
  if (!req.url || !req.url.startsWith("/api/db/stream")) {
    // Not ours — let SvelteKit's default handling (usually "refuse") run.
    return;
  }
  const session = await resolveSession(req);
  if (!session) {
    socket.write("HTTP/1.1 401 Unauthorized\r\n\r\n");
    socket.destroy();
    return;
  }

  wss.handleUpgrade(req, socket, head, (browserWs) => {
    const rustWs = new WebSocket(RUST_WS_URL);

    // Buffer browser→Rust messages that arrive before the Rust socket opens.
    // In practice the Rust service is on loopback and opens in <1ms, but
    // we handle the race cleanly anyway.
    const pending = [];
    let rustReady = false;
    // `socketClosed` is the synchronous gate. Once any frame triggers a
    // close (validation failure, or a second frame after the first), it's
    // set to true *before* any further work, and every subsequent frame
    // handler short-circuits. The previous `firstFrameValidated` flag only
    // distinguished "first vs rest" — it let any frame past the first
    // through unvalidated, which meant a second frame bearing a different
    // user's connection_id would be forwarded to Rust as-is.
    let socketClosed = false;

    rustWs.on("open", () => {
      rustReady = true;
      for (const [data, binary] of pending) rustWs.send(data, { binary });
      pending.length = 0;
    });

    // Close the browser side with a policy-violation code, informing the
    // caller that the frame was bad. 1008 is the WebSocket spec's "policy
    // violation" — right fit for auth/tenancy issues.
    const rejectAndClose = (reason) => {
      socketClosed = true;
      pending.length = 0;
      try {
        browserWs.close(1008, reason);
      } catch {
        /* ignore */
      }
      try {
        rustWs.close();
      } catch {
        /* ignore */
      }
    };

    // The protocol (see routes/db/stream.rs) is a single client→server JSON
    // frame followed by server→client batches. We intercept that one frame
    // to enforce tenant isolation by stripping the `${userId}:` prefix from
    // its `connection_id`. Any further client→server frame is unexpected:
    // closing on it prevents an attacker from following a valid first frame
    // with one bearing a different user's connection_id (which Rust would
    // happily execute, since it has no concept of users).
    //
    // Preserve frame type (Text vs Binary). `ws` delivers incoming data as
    // a Buffer regardless of frame type — `isBinary` is the authoritative
    // indicator. If we always sent as default, JSON control frames the
    // browser sends as Text would arrive at Rust as Binary, and the Axum
    // handler (which pattern-matches on Message::Text) would reject them.
    let firstFrameSeen = false;
    browserWs.on("message", (data, isBinary) => {
      if (socketClosed) return;
      if (firstFrameSeen) {
        rejectAndClose("only one client frame is permitted per stream");
        return;
      }
      firstFrameSeen = true;

      if (isBinary) {
        rejectAndClose("first frame must be a JSON text frame");
        return;
      }
      let parsed;
      try {
        const buf = Array.isArray(data) ? Buffer.concat(data) : Buffer.from(data);
        parsed = JSON.parse(buf.toString("utf8"));
      } catch {
        rejectAndClose("first frame is not valid JSON");
        return;
      }
      const cid = parsed?.connection_id;
      if (typeof cid !== "string") {
        rejectAndClose("missing connection_id");
        return;
      }
      const rustId = unscopeConnectionId(session.userId, cid);
      if (rustId === null) {
        rejectAndClose("connection does not belong to this user");
        return;
      }
      parsed.connection_id = rustId;
      const forwardData = Buffer.from(JSON.stringify(parsed), "utf8");

      if (rustReady) rustWs.send(forwardData, { binary: isBinary });
      else pending.push([forwardData, isBinary]);
    });
    rustWs.on("message", (data, isBinary) => {
      if (browserWs.readyState === WebSocket.OPEN) {
        browserWs.send(data, { binary: isBinary });
      }
    });

    const closeBoth = () => {
      if (browserWs.readyState === WebSocket.OPEN) browserWs.close();
      if (rustWs.readyState === WebSocket.OPEN) rustWs.close();
    };
    browserWs.on("close", closeBoth);
    rustWs.on("close", closeBoth);
    browserWs.on("error", closeBoth);
    rustWs.on("error", (e) => {
      console.error("[seaquel] rustWs error:", e);
      closeBoth();
    });
  });
});

server.listen(PORT, () => {
  console.log(`[seaquel] seaquel-app listening on http://0.0.0.0:${PORT}`);
  console.log(`[seaquel] proxying /api/db/* and /api/db/stream to ${RUST_URL}`);
});
