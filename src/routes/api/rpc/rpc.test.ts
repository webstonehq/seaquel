/**
 * The /api/rpc proxy: the user header comes from the session only, the body
 * reaches Rust byte for byte, and Rust's status and body come back as-is.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
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
    }),
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
