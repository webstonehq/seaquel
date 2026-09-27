/**
 * The /api/rpc/stream upgrade proxy: the user header comes from the session
 * only, the gate's refusals refuse the upgrade, frames pass through
 * untouched, and no browser path reaches anything in Rust but /rpc/stream.
 */

import { createServer, request, type IncomingHttpHeaders, type Server } from "node:http";
import type { AddressInfo, Socket } from "node:net";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WebSocket, WebSocketServer } from "ws";
import { attachRpcStreamProxy, RPC_STREAM_PATH, UPGRADE_HOST_HEADER } from "./rpc-stream-proxy.js";

/** What the fake Rust service saw. */
type Seen = { url: string | undefined; headers: IncomingHttpHeaders };

let rust: Server;
let rustWss: WebSocketServer;
let seen: Seen[];
let rustRequests: string[];
let rustSockets: WebSocket[];
let fetchMock: ReturnType<typeof vi.fn>;

const ACCESS_URL = "http://gate.test/api/account/stream-access";

/** The only Origin the fake gate trusts (the route's `isOriginTrusted`). */
const ORIGIN = "https://app.example.com";

/** Cookies whose answer a test changes while a socket is open. */
let overrides: Map<string, () => Response>;

/**
 * The gate's answers, by cookie and origin: `handleApiGate` plus
 * stream-access (its tests are in `src/routes/api/account/stream-access`).
 */
function gateAnswer(cookie: string, origin: string | null): Response {
  const override = overrides.get(cookie);
  if (override) return override();
  if (cookie === "session=alice" && origin !== ORIGIN) {
    return new Response("untrusted origin", { status: 403 });
  }
  switch (cookie) {
    case "session=alice":
      return Response.json({ userId: "alice" });
    case "session=unlicensed":
      return new Response("license suspended", { status: 403 });
    case "session=down":
      return new Response("{}", { status: 503 });
    case "session=partial":
      return Response.json({});
    default:
      return new Response("unauthorized", { status: 401 });
  }
}

/** Every TCP socket the servers accepted, upgraded ones too, to end them. */
let sockets: Set<Socket>;

function listen(server: Server): Promise<number> {
  server.on("connection", (socket: Socket) => sockets.add(socket));
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve((server.address() as AddressInfo).port));
  });
}

let proxyPort: number;
let rustPort: number;

/** A proxy server over the fake Rust and gate, with these options. */
async function startProxy(extra: { recheckMs?: number; maxLifetimeMs?: number } = {}) {
  const server = createServer((_req, res) => {
    res.statusCode = 404;
    res.end();
  });
  attachRpcStreamProxy(server, {
    rustUrl: `http://127.0.0.1:${rustPort}`,
    accessUrl: () => ACCESS_URL,
    fetch: fetchMock as unknown as typeof fetch,
    ...extra,
  });
  const port = await listen(server);
  extraServers.push(server);
  return port;
}

let extraServers: Server[];

beforeEach(async () => {
  sockets = new Set();
  overrides = new Map();
  extraServers = [];
  seen = [];
  rustRequests = [];
  rustSockets = [];
  rust = createServer((req, res) => {
    rustRequests.push(req.url ?? "");
    res.statusCode = 404;
    res.end();
  });
  rustWss = new WebSocketServer({ noServer: true });
  rust.on("upgrade", (req, socket, head) => {
    seen.push({ url: req.url, headers: req.headers });
    rustWss.handleUpgrade(req, socket, head, (ws) => {
      rustSockets.push(ws);
      ws.on("message", (data, isBinary) => {
        const text = isBinary ? "" : (data as Buffer).toString("utf8");
        // Commands for the tests; anything else is echoed as it came.
        if (text === "send-big") ws.send("y".repeat(9 * 1024 * 1024));
        else if (text === "close-1013") ws.close(1013, "TOO_MANY_SOCKETS: at most 8 open at once");
        else ws.send(data, { binary: isBinary });
      });
    });
  });
  rustPort = await listen(rust);

  fetchMock = vi.fn(async (_url: string, init?: RequestInit) => {
    const headers = new Headers(init?.headers);
    return gateAnswer(headers.get("cookie") ?? "", headers.get("origin"));
  });
  proxyPort = await startProxy();
});

afterEach(async () => {
  for (const ws of rustSockets) ws.terminate();
  rustWss.close();
  for (const socket of sockets) socket.destroy();
  await Promise.all([
    ...extraServers.map((server) => new Promise((r) => server.close(r))),
    new Promise((r) => rust.close(r)),
  ]);
});

/** Open the browser side; resolves when open, rejects with the refusal status. */
function openBrowser(
  path: string,
  headers: Record<string, string>,
  port = proxyPort,
): Promise<WebSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}${path}`, {
      headers: { origin: ORIGIN, ...headers },
    });
    ws.on("open", () => resolve(ws));
    ws.on("unexpected-response", (_req, res) => reject(new Error(`status ${res.statusCode}`)));
    ws.on("error", reject);
  });
}

function nextMessage(ws: WebSocket): Promise<{ data: Buffer; isBinary: boolean }> {
  return new Promise((resolve) => {
    ws.once("message", (data, isBinary) => resolve({ data: data as Buffer, isBinary }));
  });
}

/** Send a raw upgrade for `path` and report whether anything answered in `ms`. */
function rawUpgrade(path: string, ms = 150): Promise<string> {
  return new Promise((resolve) => {
    const req = request({
      host: "127.0.0.1",
      port: proxyPort,
      path,
      headers: {
        connection: "Upgrade",
        upgrade: "websocket",
        "sec-websocket-version": "13",
        "sec-websocket-key": "dGhlIHNhbXBsZSBub25jZQ==",
        cookie: "session=alice",
        origin: ORIGIN,
      },
    });
    const timer = setTimeout(() => {
      req.destroy();
      resolve("no answer");
    }, ms);
    req.on("upgrade", (res, socket) => {
      clearTimeout(timer);
      socket.destroy();
      resolve(`upgraded ${res.statusCode}`);
    });
    req.on("response", (res) => {
      clearTimeout(timer);
      resolve(`status ${res.statusCode}`);
    });
    req.on("error", () => undefined);
    req.end();
  });
}

describe("/api/rpc/stream proxy", () => {
  it("sets X-Seaquel-User from the session and drops the browser's headers", async () => {
    const ws = await openBrowser(RPC_STREAM_PATH, {
      cookie: "session=alice",
      "x-seaquel-user": "mallory",
      authorization: "Bearer x",
    });
    await vi.waitFor(() => expect(seen).toHaveLength(1));
    expect(seen[0].url).toBe("/rpc/stream");
    expect(seen[0].headers["x-seaquel-user"]).toBe("alice");
    expect(seen[0].headers.cookie).toBeUndefined();
    expect(seen[0].headers.authorization).toBeUndefined();
    expect(fetchMock).toHaveBeenCalledOnce();
    expect(fetchMock.mock.calls[0][0]).toBe(ACCESS_URL);
    expect(new Headers(fetchMock.mock.calls[0][1].headers).get("origin")).toBe(ORIGIN);
    ws.close();
  });

  it("passes the upgrade's Host to the gate, never a browser-sent copy", async () => {
    // The ws client sets Host to 127.0.0.1:<port>; a browser can't choose it.
    const ws = await openBrowser(RPC_STREAM_PATH, {
      cookie: "session=alice",
      [UPGRADE_HOST_HEADER]: "evil.example",
    });
    const sent = new Headers(fetchMock.mock.calls[0][1].headers);
    expect(sent.get(UPGRADE_HOST_HEADER)).toBe(`127.0.0.1:${proxyPort}`);
    ws.close();
  });

  it("pipes frames both ways untouched, Text as Text and Binary as Binary", async () => {
    const ws = await openBrowser(RPC_STREAM_PATH, { cookie: "session=alice" });
    // Sent at once: frames that arrive before the Rust socket opens are held.
    const text =
      '{"streamId":"s1","op":"start" ,"request":{"method":"db","params":{"n":1.50,"big":12345678901234567890,"t":"é\\u00e9"}}}';
    const first = nextMessage(ws);
    ws.send(text);
    const echoed = await first;
    expect(echoed.isBinary).toBe(false);
    expect(echoed.data.toString("utf8")).toBe(text);

    const bytes = Buffer.from([0, 1, 2, 255, 254]);
    const second = nextMessage(ws);
    ws.send(bytes);
    const echoedBytes = await second;
    expect(echoedBytes.isBinary).toBe(true);
    expect(Buffer.compare(echoedBytes.data, bytes)).toBe(0);
    ws.close();
  });

  it("closing the browser socket closes the Rust one", async () => {
    const ws = await openBrowser(RPC_STREAM_PATH, { cookie: "session=alice" });
    await vi.waitFor(() => expect(rustSockets).toHaveLength(1));
    const closed = new Promise((r) => rustSockets[0].once("close", r));
    ws.close();
    await closed;
  });

  it.each([
    [{}, 401],
    [{ cookie: "session=nobody" }, 401],
    [{ cookie: "session=partial" }, 401],
    [{ cookie: "session=unlicensed" }, 403],
    [{ cookie: "session=down" }, 503],
  ])("refuses %o with %i and never reaches Rust", async (headers, status) => {
    await expect(
      openBrowser(RPC_STREAM_PATH, { ...headers, "x-seaquel-user": "alice" }),
    ).rejects.toThrow(`status ${status}`);
    expect(seen).toHaveLength(0);
    expect(rustRequests).toHaveLength(0);
  });

  it("only upgrades exactly /api/rpc/stream, and always to /rpc/stream", async () => {
    for (const path of [
      "/internal/license/gate",
      "/rpc/stream",
      "/api/rpc/streamx",
      "/api/rpc/stream/",
      "/api/rpc/stream/x",
      "/api/rpc/stream%2F..%2F..%2F..%2Finternal%2Flicense%2Fgate",
      "/api/rpc/stream/../../../internal/license/gate",
      "/api/db/stream",
      "/api/rpc",
    ]) {
      expect(await rawUpgrade(path), path).toBe("no answer");
    }
    expect(fetchMock).not.toHaveBeenCalled();
    expect(seen).toHaveLength(0);

    // A query string is dropped, not forwarded.
    expect(await rawUpgrade(`${RPC_STREAM_PATH}?next=/internal/license/gate`)).toBe("upgraded 101");
    await vi.waitFor(() => expect(seen).toHaveLength(1));
    expect(seen[0].url).toBe("/rpc/stream");
    expect(rustRequests).toHaveLength(0);
  });

  it("refuses an untrusted or missing Origin with 403", async () => {
    await expect(
      openBrowser(RPC_STREAM_PATH, { cookie: "session=alice", origin: "https://evil.example" }),
    ).rejects.toThrow("status 403");
    expect(seen).toHaveLength(0);
  });

  it("passes a result frame over the browser's limit through intact", async () => {
    const ws = await openBrowser(RPC_STREAM_PATH, { cookie: "session=alice" });
    const got = nextMessage(ws);
    ws.send("send-big");
    const { data, isBinary } = await got;
    expect(isBinary).toBe(false);
    expect(data.length).toBe(9 * 1024 * 1024);
    expect(data.every((b) => b === 0x79)).toBe(true);
    // The socket is still open and piping.
    const next = nextMessage(ws);
    ws.send("ping");
    expect((await next).data.toString()).toBe("ping");
    ws.close();
  });

  it("passes Rust's close code and reason to the browser", async () => {
    const ws = await openBrowser(RPC_STREAM_PATH, { cookie: "session=alice" });
    const closed = new Promise<[number, string]>((r) =>
      ws.once("close", (code, reason) => r([code, reason.toString()])),
    );
    ws.send("close-1013");
    expect(await closed).toEqual([1013, "TOO_MANY_SOCKETS: at most 8 open at once"]);
  });

  it.each([
    [403, 1008],
    [401, 1008],
    [503, 1013],
  ])("re-checks the session: a %i closes the socket with %i", async (status, code) => {
    const port = await startProxy({ recheckMs: 50 });
    const ws = await openBrowser(RPC_STREAM_PATH, { cookie: "session=alice" }, port);
    await vi.waitFor(() => expect(rustSockets).toHaveLength(1));
    // Still allowed: a few re-checks pass and the socket stays open.
    await vi.waitFor(() => expect(fetchMock.mock.calls.length).toBeGreaterThanOrEqual(3));
    expect(ws.readyState).toBe(WebSocket.OPEN);

    const closed = new Promise<number>((r) => ws.once("close", (c) => r(c)));
    const rustClosed = new Promise((r) => rustSockets[0].once("close", r));
    overrides.set("session=alice", () => new Response("no", { status }));
    expect(await closed).toBe(code);
    await rustClosed;
  });

  it("a re-check that answers another user closes with 1008", async () => {
    const port = await startProxy({ recheckMs: 50 });
    const ws = await openBrowser(RPC_STREAM_PATH, { cookie: "session=alice" }, port);
    const closed = new Promise<number>((r) => ws.once("close", (c) => r(c)));
    overrides.set("session=alice", () => Response.json({ userId: "bob" }));
    expect(await closed).toBe(1008);
  });

  it("closes a socket at the end of its lifetime so the client re-authorises", async () => {
    const port = await startProxy({ maxLifetimeMs: 100 });
    const ws = await openBrowser(RPC_STREAM_PATH, { cookie: "session=alice" }, port);
    const closed = new Promise<number>((r) => ws.once("close", (c) => r(c)));
    expect(await closed).toBe(1000);
  });
});
