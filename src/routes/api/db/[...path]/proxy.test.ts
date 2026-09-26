/**
 * Tenant scoping in the /api/db/* proxy for POST /api/db/engine: the body's
 * top-level `connection_id` must carry the caller's prefix, which is stripped
 * before the call reaches the Rust service. The engine request itself is
 * forwarded untouched.
 */

import { afterEach, describe, expect, it, vi } from "vitest";
import { POST } from "./+server";

type Handler = typeof POST;
type Event = Parameters<Handler>[0];

function engineEvent(userId: string | null, body: unknown): Event {
  return {
    locals: { user: userId ? { id: userId } : null },
    params: { path: "engine" },
    request: new Request("http://localhost/api/db/engine", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    }),
    url: new URL("http://localhost/api/db/engine"),
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

const request = { method: "tableMetadata", params: { schema: "public", table: "t" } };

describe("/api/db/engine proxy", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("strips the caller's prefix from connection_id and forwards the request", async () => {
    const response = { kind: "tableMetadata", data: { columns: [], indexes: [] } };
    const fetchMock = vi.fn(
      async (_url: string, _init: RequestInit) =>
        new Response(JSON.stringify(response), {
          status: 200,
          headers: { "content-type": "application/json" },
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const res = await POST(
      engineEvent("user-1", { connection_id: "user-1:postgres-abc", request }),
    );

    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toMatch(/\/api\/db\/engine$/);
    expect(JSON.parse(init.body as string)).toEqual({ connection_id: "postgres-abc", request });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual(response);
  });

  it("rejects another user's connection with 403 without calling upstream", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    const status = await statusOf(
      POST(engineEvent("user-1", { connection_id: "user-2:postgres-abc", request })),
    );

    expect(status).toBe(403);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("rejects an unprefixed connection_id with 403", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    const status = await statusOf(
      POST(engineEvent("user-1", { connection_id: "postgres-abc", request })),
    );

    expect(status).toBe(403);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("requires a signed-in user", async () => {
    const status = await statusOf(
      POST(engineEvent(null, { connection_id: "user-1:postgres-abc", request })),
    );
    expect(status).toBe(401);
  });
});

describe("/api/db/* path allow-list", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  /** An event for `params.path` as SvelteKit decodes it (`%2F` becomes `/`). */
  function pathEvent(path: string, body: unknown): Event {
    const encoded = path.split("/").map(encodeURIComponent).join("%2F");
    return {
      locals: { user: { id: "user-1" } },
      params: { path },
      request: new Request(`http://localhost/api/db/${encoded}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      }),
      url: new URL(`http://localhost/api/db/${encoded}`),
    } as unknown as Event;
  }

  it.each([
    ["x/../query", "x%2F..%2Fquery"],
    ["../../rpc", "..%2F..%2Frpc"],
    ["x/../../../rpc", "x%2F..%2F..%2F..%2Frpc"],
    ["query/../engine", "query%2F..%2Fengine"],
    ["query/", "a trailing slash"],
    ["stream", "stream (server.js proxies the WebSocket)"],
    ["", "an empty path"],
    ["Query", "another case"],
  ])("refuses %s (%s) with 404 before any fetch", async (path) => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);

    const status = await statusOf(
      POST(pathEvent(path, { connection_id: "postgres-abc", sql: "SELECT 1" })),
    );

    expect(status).toBe(404);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it.each(["connect", "disconnect", "query", "execute", "transaction", "engine", "test"])(
    "still forwards %s",
    async (path) => {
      const fetchMock = vi.fn(
        async (_url: string, _init: RequestInit) =>
          new Response(JSON.stringify({ connection_id: "postgres-abc" }), {
            status: 200,
            headers: { "content-type": "application/json" },
          }),
      );
      vi.stubGlobal("fetch", fetchMock);

      const res = await POST(pathEvent(path, { connection_id: "user-1:postgres-abc" }));

      expect(res.status).toBe(200);
      expect(fetchMock).toHaveBeenCalledOnce();
      expect(fetchMock.mock.calls[0][0]).toMatch(new RegExp(`/api/db/${path}$`));
    },
  );
});
