/**
 * The /api/rpc proxy: the user header comes from the session only, the body
 * reaches Rust byte for byte, and Rust's status and body come back as-is.
 */

import { EventEmitter } from "node:events";
import { afterEach, describe, expect, it, vi } from "vitest";
import { withClientRequest } from "$shared/client-request.js";
import { RPC_BODY_LIMIT, RPC_USER_BUDGET, rpcBytesInFlight } from "$lib/server/body-limit";
import { POST } from "./+server";

type Handler = typeof POST;
type Event = Parameters<Handler>[0];

function rpcEvent(
  userId: string | null,
  body: BodyInit,
  headers: Record<string, string> = {},
): Event {
  return {
    locals: { user: userId ? { id: userId } : null },
    request: new Request("http://localhost/api/rpc", {
      method: "POST",
      // The page's own origin, as a browser sends it.
      headers: {
        "content-type": "application/json",
        origin: "http://localhost",
        host: "localhost",
        ...headers,
      },
      body,
      duplex: "half",
    } as RequestInit),
    url: new URL("http://localhost/api/rpc"),
  } as unknown as Event;
}

/** SvelteKit's `error()` throws an HttpError with a `status`. */
async function statusOf(promise: ReturnType<Handler>): Promise<number> {
  try {
    await promise;
  } catch (e) {
    return (e as { status: number }).status;
  }
  throw new Error("expected the handler to throw");
}

/** A Node request whose socket a test can close, as adapter-node's `platform.req`. */
function fakeNodeRequest() {
  const socket = Object.assign(new EventEmitter(), { destroyed: false });
  return { socket, close: () => ((socket.destroyed = true), socket.emit("close")) };
}

/** A fetch that answers only when its signal aborts (then it rejects). */
function hangingFetch() {
  const fetchMock = vi.fn(
    (_url: string, init: RequestInit) =>
      new Promise<Response>((_, reject) => {
        init.signal?.addEventListener("abort", () =>
          reject(new DOMException("aborted", "AbortError")),
        );
      }),
  );
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

function stubFetch(response: () => Response) {
  const fetchMock = vi.fn(async (_url: string, _init: RequestInit) => response());
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

async function sentBytes(init: RequestInit): Promise<Uint8Array> {
  return new Uint8Array(await new Response(init.body).arrayBuffer());
}

// `params` after `method`, keys out of order, odd number spellings and
// non-ASCII text: a parse and re-stringify would change every one of them.
const BODY =
  '{"method":"storage","params":{"method":"onboardingSave","params":{"data":{"zeta":1e+21,"alpha":{"b":1.50,"a":-0.0},"mid":"éé \\u00e9","big":12345678901234567890}}}}';

describe("/api/rpc proxy", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("returns 401 without a session and calls nothing", async () => {
    const fetchMock = stubFetch(() => new Response("{}"));
    expect(await statusOf(POST(rpcEvent(null, BODY)))).toBe(401);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each([
    ["another site", { origin: "https://evil.example" }],
    ["no Origin", { origin: "" }],
  ])("refuses a call from %s with 403 ORIGIN_NOT_ALLOWED and calls nothing", async (_, headers) => {
    const fetchMock = stubFetch(() => new Response("{}"));
    const event = rpcEvent("user-1", BODY, headers);
    if (!headers.origin) event.request.headers.delete("origin");
    const res = (await POST(event)) as Response;
    expect(res.status).toBe(403);
    expect(await res.json()).toEqual({
      code: "ORIGIN_NOT_ALLOWED",
      message: "request origin not allowed",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("sets X-Seaquel-User from the session and drops the client's copy", async () => {
    const fetchMock = stubFetch(() => new Response("{}", { status: 200 }));

    await POST(
      rpcEvent("user-1", BODY, {
        "X-Seaquel-User": "user-2",
        cookie: "session=secret",
        authorization: "Bearer x",
      }),
    );

    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/rpc$/);
    expect(init.method).toBe("POST");
    const headers = new Headers(init.headers);
    expect(headers.get("x-seaquel-user")).toBe("user-1");
    expect(headers.get("cookie")).toBeNull();
    expect(headers.get("authorization")).toBeNull();
    expect([...headers.keys()].sort()).toEqual(["content-type", "x-seaquel-user"]);
  });

  it("forwards a valid X-Seaquel-Origin and drops a bad one", async () => {
    const fetchMock = stubFetch(() => new Response("{}", { status: 200 }));
    const long = "x".repeat(65);
    const cases: Array<[Record<string, string>, string | null]> = [
      [{ "X-Seaquel-Origin": "tab-1_A9" }, "tab-1_A9"],
      [
        { "X-Seaquel-Origin": "0f8f5b0e-3d7a-4f4e-9a38-6f1d1c2b3a4d" },
        "0f8f5b0e-3d7a-4f4e-9a38-6f1d1c2b3a4d",
      ],
      [{ "X-Seaquel-Origin": "x".repeat(64) }, "x".repeat(64)],
      [{ "X-Seaquel-Origin": long }, null],
      [{ "X-Seaquel-Origin": "a b" }, null],
      [{ "X-Seaquel-Origin": "a/b" }, null],
      [{ "X-Seaquel-Origin": "a,b" }, null],
      [{ "X-Seaquel-Origin": "" }, null],
      [{}, null],
    ];
    for (const [sent, forwarded] of cases) {
      fetchMock.mockClear();
      await POST(rpcEvent("user-1", BODY, sent));
      const headers = new Headers(fetchMock.mock.calls[0][1].headers);
      expect(headers.get("x-seaquel-origin"), JSON.stringify(sent)).toBe(forwarded);
      expect(headers.get("x-seaquel-user")).toBe("user-1");
      const expected = forwarded === null ? [] : ["x-seaquel-origin"];
      expect([...headers.keys()].sort()).toEqual(["content-type", ...expected, "x-seaquel-user"]);
    }
  });

  it("drops a repeated X-Seaquel-Origin", async () => {
    const fetchMock = stubFetch(() => new Response("{}", { status: 200 }));
    const event = rpcEvent("user-1", BODY);
    event.request.headers.append("X-Seaquel-Origin", "tab-1");
    event.request.headers.append("X-Seaquel-Origin", "tab-2");
    await POST(event);
    const headers = new Headers(fetchMock.mock.calls[0][1].headers);
    expect(headers.get("x-seaquel-origin")).toBeNull();
  });

  it("still drops a client-sent user alongside a valid origin", async () => {
    const fetchMock = stubFetch(() => new Response("{}", { status: 200 }));
    await POST(rpcEvent("user-1", BODY, { "X-Seaquel-User": "user-2", "X-Seaquel-Origin": "t1" }));
    const headers = new Headers(fetchMock.mock.calls[0][1].headers);
    expect(headers.get("x-seaquel-user")).toBe("user-1");
    expect(headers.get("x-seaquel-origin")).toBe("t1");
  });

  it("forwards the body byte for byte", async () => {
    const fetchMock = stubFetch(() => new Response("{}"));
    const bytes = new TextEncoder().encode(BODY);

    await POST(rpcEvent("user-1", bytes));

    const [, init] = fetchMock.mock.calls[0];
    expect(await sentBytes(init)).toEqual(bytes);
  });

  it("forwards bytes that aren't even valid UTF-8 or JSON unchanged", async () => {
    const fetchMock = stubFetch(() => new Response("{}"));
    const bytes = new Uint8Array([0x7b, 0xff, 0x00, 0xc3, 0x28, 0x7d]);

    await POST(rpcEvent("user-1", bytes));

    const [, init] = fetchMock.mock.calls[0];
    expect(await sentBytes(init)).toEqual(bytes);
  });

  it("passes Rust's status and RpcError body through", async () => {
    const rpcError = '{"code":"NOT_SUPPORTED","message":"Secrets isn\'t supported here"}';
    stubFetch(
      () =>
        new Response(rpcError, {
          status: 501,
          headers: { "content-type": "application/json" },
        }),
    );

    const res = await POST(rpcEvent("user-1", BODY));

    expect(res.status).toBe(501);
    expect(res.headers.get("content-type")).toBe("application/json");
    expect(await res.text()).toBe(rpcError);
  });

  it("passes a successful response's bytes through", async () => {
    const ok = `{"method":"storage","result":{"method":"onboardingLoad","result":{"z":1e+21,"a":"é"}}}`;
    stubFetch(
      () => new Response(ok, { status: 200, headers: { "content-type": "application/json" } }),
    );

    const res = await POST(rpcEvent("user-1", BODY));

    expect(res.status).toBe(200);
    expect(await res.text()).toBe(ok);
  });

  // Probe I1: SvelteKit's `request.signal` only aborts before the body is
  // read, so the route watches the client's socket itself, or an aborted
  // apply would run (and commit) on.
  it("aborts the call to Rust when the client's socket closes (adapter-node)", async () => {
    const fetchMock = hangingFetch();
    const node = fakeNodeRequest();
    const event = rpcEvent("user-1", BODY);
    (event as unknown as { platform: unknown }).platform = { req: node };

    const pending = POST(event);
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    const signal = fetchMock.mock.calls[0][1].signal!;
    expect(signal.aborted).toBe(false);
    node.close();
    expect(signal.aborted).toBe(true);
    const res = (await pending) as Response;
    expect(res.status).toBe(499);
  });

  it("aborts it in the Vite dev server too, through the dev plugin's request", async () => {
    const fetchMock = hangingFetch();
    const node = fakeNodeRequest();

    const pending = withClientRequest(node as never, () => POST(rpcEvent("user-1", BODY)));
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());
    node.close();
    expect(fetchMock.mock.calls[0][1].signal!.aborted).toBe(true);
    expect(((await pending) as Response).status).toBe(499);
  });

  it("doesn't call Rust when the client is already gone", async () => {
    const fetchMock = stubFetch(() => new Response("{}"));
    const node = fakeNodeRequest();
    node.close();
    const event = rpcEvent("user-1", BODY);
    (event as unknown as { platform: unknown }).platform = { req: node };

    const res = (await POST(event)) as Response;
    expect(res.status).toBe(499);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("stops watching the socket once Rust answers (keep-alive sockets are reused)", async () => {
    stubFetch(() => new Response("{}"));
    const node = fakeNodeRequest();
    const event = rpcEvent("user-1", BODY);
    (event as unknown as { platform: unknown }).platform = { req: node };

    const res = (await POST(event)) as Response;
    expect(res.status).toBe(200);
    // A later close (the keep-alive socket ends) aborts nothing.
    const signal = vi.mocked(fetch).mock.calls[0][1]!.signal!;
    node.close();
    expect(signal.aborted).toBe(false);
  });

  // Probe I2: `/api/rpc` takes up to RPC_BODY_LIMIT (the hook enforces it
  // while the body streams in); past it the GUI gets an RpcError it shows.
  it("answers a body past the limit with 413 INVALID_ARGUMENT naming the limit", async () => {
    const fetchMock = stubFetch(() => new Response("{}"));
    const big = new Uint8Array(RPC_BODY_LIMIT + 1);

    const res = (await POST(rpcEvent("user-1", big))) as Response;

    expect(res.status).toBe(413);
    const body = await res.json();
    expect(body.code).toBe("INVALID_ARGUMENT");
    expect(body.message).toContain("20 MiB");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // Probe review, M4 (b): Node holds every body it reads, so a user's
  // calls in flight may hold at most RPC_USER_BUDGET of bodies; past it the
  // route answers 429 before reading.
  it("holds a user's bodies in flight to the budget and answers 429 past it", async () => {
    expect(RPC_USER_BUDGET).toBe(40 * 1024 * 1024);
    const fetchMock = hangingFetch();
    const size = 900 * 1024;
    const nodes = Array.from({ length: 200 }, () => fakeNodeRequest());
    const calls = nodes.map((node) => {
      const event = rpcEvent("user-1", new Uint8Array(size), { "content-length": String(size) });
      (event as unknown as { platform: unknown }).platform = { req: node };
      return POST(event) as Promise<Response>;
    });
    const fits = Math.floor(RPC_USER_BUDGET / size);
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(fits));
    // Everyone else was refused without a call to Rust.
    const settled = await Promise.all(
      calls.map((c) => Promise.race([c, new Promise((r) => setTimeout(() => r(null), 50))])),
    );
    const refused = settled.filter((r) => r !== null) as Response[];
    expect(refused).toHaveLength(200 - fits);
    for (const res of refused) {
      expect(res.status).toBe(429);
      expect((await res.json()).code).toBe("TOO_MANY_REQUESTS");
    }
    expect(fetchMock).toHaveBeenCalledTimes(fits);

    // Another user isn't held up.
    stubFetch(() => new Response("{}"));
    expect(((await POST(rpcEvent("user-2", BODY))) as Response).status).toBe(200);
    // Every path releases: the held calls end when their clients leave.
    hangingFetch();
    nodes.forEach((n) => n.close());
    await Promise.all(calls);
    expect(rpcBytesInFlight("user-1")).toBe(0);
    expect(rpcBytesInFlight("user-2")).toBe(0);
  });

  it("always admits a small call, even with the budget full, and counts it", async () => {
    const fetchMock = hangingFetch();
    const nodes = [fakeNodeRequest(), fakeNodeRequest()];
    const sizes = [RPC_BODY_LIMIT, RPC_USER_BUDGET - RPC_BODY_LIMIT];
    const pending = nodes.map((node, i) => {
      const held = rpcEvent("user-4", new Uint8Array(sizes[i]), {
        "content-length": String(sizes[i]),
      });
      (held as unknown as { platform: unknown }).platform = { req: node };
      return POST(held) as Promise<Response>;
    });
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));

    // A storage save under 64 KiB, declared or streamed, still goes through.
    stubFetch(() => new Response("{}"));
    const small = 64 * 1024 - 1;
    const declared = rpcEvent("user-4", new Uint8Array(small), { "content-length": String(small) });
    expect(((await POST(declared)) as Response).status).toBe(200);
    const streamed = rpcEvent(
      "user-4",
      new ReadableStream<Uint8Array>({
        start(c) {
          c.enqueue(new Uint8Array(small));
          c.close();
        },
      }),
    );
    expect(((await POST(streamed)) as Response).status).toBe(200);
    // One at 64 KiB isn't small.
    const edge = 64 * 1024;
    const big = rpcEvent("user-4", new Uint8Array(edge), { "content-length": String(edge) });
    expect(((await POST(big)) as Response).status).toBe(429);

    // A small call counts while it runs.
    hangingFetch();
    const node = fakeNodeRequest();
    const counted = rpcEvent("user-4", new Uint8Array(small), { "content-length": String(small) });
    (counted as unknown as { platform: unknown }).platform = { req: node };
    const waiting = POST(counted) as Promise<Response>;
    await vi.waitFor(() => expect(rpcBytesInFlight("user-4")).toBe(RPC_USER_BUDGET + small));

    [...nodes, node].forEach((n) => n.close());
    await Promise.all([...pending, waiting]);
    expect(rpcBytesInFlight("user-4")).toBe(0);
  });

  it("counts a body sent without Content-Length as it arrives", async () => {
    const fetchMock = hangingFetch();
    // Two held calls fill the budget but for 1 KiB.
    const nodes = [fakeNodeRequest(), fakeNodeRequest()];
    const sizes = [RPC_BODY_LIMIT, RPC_USER_BUDGET - RPC_BODY_LIMIT - 1024];
    const pending = nodes.map((node, i) => {
      const held = rpcEvent("user-3", new Uint8Array(sizes[i]), {
        "content-length": String(sizes[i]),
      });
      (held as unknown as { platform: unknown }).platform = { req: node };
      return POST(held) as Promise<Response>;
    });
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(2));

    const chunk = new Uint8Array(64 * 1024);
    const stream = new ReadableStream<Uint8Array>({
      start(c) {
        for (let i = 0; i < 4; i++) c.enqueue(chunk);
        c.close();
      },
    });
    const chunked = rpcEvent("user-3", stream);
    const res = (await POST(chunked)) as Response;
    expect(res.status).toBe(429);
    expect(fetchMock).toHaveBeenCalledTimes(2);

    nodes.forEach((n) => n.close());
    await Promise.all(pending);
    expect(rpcBytesInFlight("user-3")).toBe(0);
  });

  it("answers 502 with RpcError JSON when Rust is unreachable", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("fetch failed: connect ECONNREFUSED 127.0.0.1:8788");
      }),
    );

    const res = await POST(rpcEvent("user-1", BODY));

    expect(res.status).toBe(502);
    const body = await res.json();
    expect(body.code).toBe("UPSTREAM_UNAVAILABLE");
    expect(JSON.stringify(body)).not.toContain("127.0.0.1");
  });
});
