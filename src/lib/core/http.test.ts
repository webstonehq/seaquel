/**
 * `HttpCoreClient` over a fake WebSocket: one socket shared by every stream,
 * frames routed by stream id, the per-socket cap, cancel frames, failing
 * started streams when the socket closes, reconnecting with backoff, a 1008
 * close (access lost), and `connectionClosed` events.
 */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { CoreEvent } from "$lib/types/generated/CoreEvent";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));

const { HttpCoreClient } = await import("./http");
const { webPageOrigin } = await import("./origin");
const { windowIdReady } = await import("./window-id");
// The page's window id is settled before its first call (`window-id.test.ts`
// covers the wait itself).
beforeAll(async () => {
  await windowIdReady();
});
import type { AiChatRequest, QueryStreamRequest, RunRequest, StreamEvent } from "./client";
import type { AiEvent } from "$lib/types/generated/AiEvent";
import type { RunEvent } from "$lib/types/generated/RunEvent";

class FakeSocket {
  static all: FakeSocket[] = [];
  readyState: WebSocket["readyState"] = 0;
  sent: Record<string, unknown>[] = [];
  onopen: ((e: Event) => void) | null = null;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onerror: ((e: Event) => void) | null = null;
  onclose: ((e: CloseEvent) => void) | null = null;
  constructor(readonly url: string) {
    FakeSocket.all.push(this);
  }
  open() {
    this.readyState = 1;
    this.onopen?.({} as Event);
  }
  send(data: string) {
    this.sent.push(JSON.parse(data) as Record<string, unknown>);
  }
  close() {}
  /** The server closes the socket. */
  drop(code = 1006) {
    this.readyState = 3;
    this.onclose?.({ code } as CloseEvent);
  }
  receive(event: CoreEvent) {
    this.onmessage?.({ data: JSON.stringify(event) } as MessageEvent);
  }
  emit(streamId: string, event: StreamEvent) {
    this.receive({ type: "stream", streamId, event });
  }
  starts(): string[] {
    return this.sent.filter((f) => f.op === "start").map((f) => f.streamId as string);
  }
}

function request(streamId: string): QueryStreamRequest {
  return {
    method: "db",
    params: {
      method: "queryStream",
      params: { connectionId: "c-1", streamId, sql: "SELECT 1", params: [] },
    },
  };
}

async function collect(iterable: AsyncIterable<StreamEvent>): Promise<StreamEvent[]> {
  const out: StreamEvent[] = [];
  for await (const event of iterable) out.push(event);
  return out;
}

const onAccessLost = vi.fn();

function client(options: { maxStreams?: number } = {}) {
  return new HttpCoreClient({
    url: "ws://seaquel.test/api/rpc/stream",
    createSocket: (url) => new FakeSocket(url),
    initialDelayMs: 100,
    maxDelayMs: 1000,
    retryDelayMs: 10,
    onAccessLost,
    ...options,
  });
}

const socket = (i = 0) => FakeSocket.all[i];

beforeEach(() => {
  FakeSocket.all = [];
  onAccessLost.mockClear();
});
afterEach(() => {
  vi.useRealTimers();
});

describe("HttpCoreClient streams", () => {
  it("runs every stream over one socket, routed by stream id", async () => {
    const c = client();
    const a = collect(c.stream(request("a")));
    const b = collect(c.stream(request("b")));
    expect(FakeSocket.all).toHaveLength(1);
    expect(socket().url).toBe("ws://seaquel.test/api/rpc/stream");
    socket().open();
    expect(socket().sent).toEqual([
      { op: "start", streamId: "a", request: request("a") },
      { op: "start", streamId: "b", request: request("b") },
    ]);
    socket().emit("b", { type: "done" });
    socket().emit("a", { type: "batch", columns: ["n"], rows: [[1]], is_final: true });
    socket().emit("a", { type: "done" });
    expect(await a).toEqual([
      { type: "batch", columns: ["n"], rows: [[1]], is_final: true },
      { type: "done" },
    ]);
    expect(await b).toEqual([{ type: "done" }]);
    // A later stream reuses the open socket.
    void c.stream(request("c"));
    expect(FakeSocket.all).toHaveLength(1);
    expect(socket().starts()).toEqual(["a", "b", "c"]);
  });

  it("starts at most maxStreams at once; the rest wait for a free slot", async () => {
    const c = client({ maxStreams: 2 });
    const streams = ["a", "b", "c"].map((id) => collect(c.stream(request(id))));
    socket().open();
    expect(socket().starts()).toEqual(["a", "b"]);
    socket().emit("a", { type: "done" });
    await streams[0];
    expect(socket().starts()).toEqual(["a", "b", "c"]);
  });

  it("gives up on a start refused too often, with TOO_MANY_STREAMS", async () => {
    const c = new HttpCoreClient({
      url: "ws://x",
      createSocket: (url) => new FakeSocket(url),
      retryDelayMs: 1,
      maxStreamRetries: 2,
    });
    const a = collect(c.stream(request("a")));
    socket().open();
    const refuse = () =>
      socket().emit("a", { type: "error", code: "TOO_MANY_STREAMS", message: "16 running" });
    refuse();
    await vi.waitFor(() => expect(socket().starts()).toHaveLength(2));
    refuse();
    await vi.waitFor(() => expect(socket().starts()).toHaveLength(3));
    refuse();
    expect(await a).toEqual([expect.objectContaining({ code: "TOO_MANY_STREAMS" })]);
  });

  it("retries a start the server refused with TOO_MANY_STREAMS", async () => {
    const c = client();
    const a = collect(c.stream(request("a")));
    socket().open();
    socket().emit("a", { type: "error", code: "TOO_MANY_STREAMS", message: "16 running" });
    await vi.waitFor(() => expect(socket().starts()).toEqual(["a", "a"]));
    socket().emit("a", { type: "done" });
    expect(await a).toEqual([{ type: "done" }]);
  });

  it("aborting sends a cancel frame and ends the stream as CANCELLED", async () => {
    const c = client();
    const controller = new AbortController();
    const a = collect(c.stream(request("a"), { signal: controller.signal }));
    socket().open();
    controller.abort();
    expect(await a).toEqual([expect.objectContaining({ type: "error", code: "CANCELLED" })]);
    expect(socket().sent.at(-1)).toEqual({ op: "cancel", streamId: "a" });
    // The server's late events for it go nowhere.
    socket().emit("a", { type: "done" });
  });

  it("a stream aborted before its start never starts", async () => {
    const c = client();
    const controller = new AbortController();
    const a = collect(c.stream(request("a"), { signal: controller.signal }));
    controller.abort();
    await a;
    socket().open();
    expect(socket().sent).toEqual([]);
  });

  it("fails started streams when the socket closes, and reconnects with backoff", async () => {
    vi.useFakeTimers();
    const c = client();
    const stop = c.events(() => {});
    const a = collect(c.stream(request("a")));
    socket().open();
    socket().drop();
    expect(await a).toEqual([expect.objectContaining({ type: "error", code: "WS_CLOSED" })]);
    expect(FakeSocket.all).toHaveLength(1);
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(2);
    // It never opens: the next wait doubles.
    socket(1).drop();
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(3);
    // A socket that proves itself (a message) resets the backoff, so a
    // later close (the server's lifetime cap) comes back quietly.
    socket(2).open();
    socket(2).receive({ type: "connectionClosed", connectionId: "x", code: "C", message: "" });
    socket(2).drop(1000);
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(4);
    expect(onAccessLost).not.toHaveBeenCalled();
    stop();
  });

  it("keeps backing off when sockets close right after opening", async () => {
    vi.useFakeTimers();
    const c = client();
    c.events(() => {});
    // Each socket opens and is closed at once (a refused upgrade).
    for (const wait of [100, 200, 400, 800, 1000, 1000]) {
      socket(FakeSocket.all.length - 1).open();
      socket(FakeSocket.all.length - 1).drop(1011);
      const before = FakeSocket.all.length;
      await vi.advanceTimersByTimeAsync(wait - 1);
      expect(FakeSocket.all).toHaveLength(before);
      await vi.advanceTimersByTimeAsync(1);
      expect(FakeSocket.all).toHaveLength(before + 1);
    }
  });

  it("resets the backoff once a socket has stayed open a while", async () => {
    vi.useFakeTimers();
    const c = client();
    c.events(() => {});
    socket().drop();
    await vi.advanceTimersByTimeAsync(100);
    socket(1).drop();
    await vi.advanceTimersByTimeAsync(200);
    socket(2).open();
    await vi.advanceTimersByTimeAsync(5_000);
    socket(2).drop(1000);
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(4);
  });

  it("fails every stream on TOO_MANY_SOCKETS and keeps backing off", async () => {
    vi.useFakeTimers();
    const c = client();
    c.events(() => {});
    const a = collect(c.stream(request("a")));
    const b = collect(c.stream(request("b")));
    socket().open();
    socket().onclose?.({
      code: 1013,
      reason: "TOO_MANY_SOCKETS: at most 8 open at once",
    } as CloseEvent);
    for (const out of [await a, await b]) {
      expect(out).toEqual([expect.objectContaining({ type: "error", code: "TOO_MANY_TABS" })]);
    }
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(2);
    socket(1).open();
    socket(1).onclose?.({ code: 1013, reason: "TOO_MANY_SOCKETS" } as CloseEvent);
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(2);
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(3);
  });

  it("fails a waiting stream when the server can't be reached", async () => {
    const c = client();
    const a = collect(c.stream(request("a")));
    socket().drop();
    expect(await a).toEqual([expect.objectContaining({ type: "error", code: "WS_CLOSED" })]);
  });

  it("a new stream doesn't wait out the backoff", async () => {
    vi.useFakeTimers();
    const c = client();
    c.events(() => {});
    socket().open();
    socket().drop();
    void c.stream(request("a"));
    expect(FakeSocket.all).toHaveLength(2);
  });

  it("on a 1008 close, fails every stream, says so once, and stops reconnecting", async () => {
    vi.useFakeTimers();
    const c = client();
    c.events(() => {});
    const a = collect(c.stream(request("a")));
    const b = collect(c.stream(request("b")));
    socket().open();
    socket().drop(1008);
    for (const out of [await a, await b]) {
      expect(out).toEqual([expect.objectContaining({ type: "error", code: "ACCESS_LOST" })]);
    }
    expect(onAccessLost).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(FakeSocket.all).toHaveLength(1);
    // A new query tries again.
    void c.stream(request("c"));
    expect(FakeSocket.all).toHaveLength(2);
  });
});

describe("HttpCoreClient.events", () => {
  it("opens the socket and delivers connectionClosed events", () => {
    const c = client();
    const handler = vi.fn();
    c.events(handler);
    expect(FakeSocket.all).toHaveLength(1);
    socket().open();
    const closed: CoreEvent = {
      type: "connectionClosed",
      connectionId: "c-1",
      code: "WORKSPACE_EVICTED",
      message: "evicted",
    };
    socket().receive(closed);
    expect(handler).toHaveBeenCalledWith(closed);
  });

  it("delivers storageChanged events too (phase 5d)", () => {
    const c = client();
    const handler = vi.fn();
    c.events(handler);
    socket().open();
    const changed: CoreEvent = {
      type: "storageChanged",
      kind: "savedQuery",
      scope: "default-seaquel",
      ids: ["saved-1"],
      origin: "tab-2",
      seq: { epoch: "e", n: 7 },
    };
    socket().receive(changed);
    expect(handler).toHaveBeenCalledWith(changed);
  });
});

describe("HttpCoreClient reconnect signals (phase 5d review, I2)", () => {
  it("runs onResubscribed on every socket open, initial the first time", async () => {
    vi.useFakeTimers();
    const c = client();
    const resubscribed = vi.fn();
    c.onResubscribed(resubscribed);
    c.events(() => {});
    socket(0).open();
    expect(resubscribed).toHaveBeenLastCalledWith({ initial: true });
    // A drop (as the server's `EVENTS_LAGGED` close), then the reconnect.
    socket(0).onclose?.({ code: 1013, reason: "EVENTS_LAGGED: behind" } as CloseEvent);
    await vi.advanceTimersByTimeAsync(100);
    expect(FakeSocket.all).toHaveLength(2);
    socket(1).open();
    expect(resubscribed).toHaveBeenCalledTimes(2);
    expect(resubscribed).toHaveBeenLastCalledWith({ initial: false });
  });

  it("says events are unavailable on a 1008 close and on TOO_MANY_SOCKETS", () => {
    const c = client();
    const unavailable = vi.fn();
    const stop = c.onEventsUnavailable(unavailable);
    c.events(() => {});
    socket(0).open();
    socket(0).onclose?.({ code: 1013, reason: "TOO_MANY_SOCKETS: at most 8" } as CloseEvent);
    expect(unavailable).toHaveBeenLastCalledWith("TOO_MANY_TABS");
    const d = client();
    const lost = vi.fn();
    d.onEventsUnavailable(lost);
    d.events(() => {});
    socket(1).open();
    socket(1).drop(1008);
    expect(lost).toHaveBeenCalledWith("ACCESS_LOST");
    stop();
  });

  it("opens the default socket URL with the page's origin", () => {
    const urls: string[] = [];
    const c = new HttpCoreClient({
      createSocket: (url) => {
        urls.push(url);
        return new FakeSocket(url);
      },
    });
    c.events(() => {});
    expect(urls[0]).toBe(`ws://localhost/api/rpc/stream?origin=${webPageOrigin()}`);
  });
});

describe("HttpCoreClient.call", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sends the page's origin as X-Seaquel-Origin, the same on every call", async () => {
    const fetchMock = vi.fn(
      async (_url: string, _init: RequestInit) =>
        new Response(
          JSON.stringify({ method: "storage", result: { method: "vaultStateLoad", result: null } }),
        ),
    );
    vi.stubGlobal("fetch", fetchMock);
    const c = client();
    await c.call({ method: "storage", params: { method: "vaultStateLoad" } });
    await c.call({ method: "storage", params: { method: "vaultStateLoad" } });
    const origins = fetchMock.mock.calls.map(([, init]) =>
      new Headers(init.headers).get("x-seaquel-origin"),
    );
    expect(origins[0]).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    expect(origins[1]).toBe(origins[0]);
    expect(origins[0]).toBe(webPageOrigin());
  });
});

describe("HttpCoreClient.call with a lone surrogate (Task 7 probe, item 7)", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("sends every string well-formed, keys included", async () => {
    const fetchMock = vi.fn(
      async (_url: string, _init: RequestInit) =>
        new Response(JSON.stringify({ method: "db", result: { method: "execute", result: null } })),
    );
    vi.stubGlobal("fetch", fetchMock);
    await client().call({
      method: "db",
      params: {
        method: "execute",
        params: {
          connectionId: "c\ud800",
          sql: "UPDATE t SET v = 'a\ud800b'",
          params: [{ ["k\udc00"]: "x" }],
        },
      },
    } as never);
    const sent = new TextDecoder().decode(fetchMock.mock.calls[0][1].body as Uint8Array);
    expect(sent).not.toMatch(/\\ud[89a-f]/i);
    expect(JSON.parse(sent).params.params).toEqual({
      connectionId: "c\ufffd",
      sql: "UPDATE t SET v = 'a\ufffdb'",
      params: [{ ["k\ufffd"]: "x" }],
    });
  });
});

describe("HttpCoreClient.stream of a run (phase 5b)", () => {
  function runRequest(streamId: string, text = "SELECT 1"): RunRequest {
    return {
      method: "db",
      params: {
        method: "run",
        params: { connectionId: "c-1", streamId, text, target: { type: "all" }, pageSize: 100 },
      },
    };
  }

  it("routes run frames by stream id and ends at the run's done", async () => {
    const c = client();
    const events: RunEvent[] = [];
    const done = (async () => {
      for await (const e of c.stream(runRequest("r"))) events.push(e);
    })();
    socket().open();
    expect(socket().starts()).toEqual(["r"]);
    socket().receive({
      type: "run",
      streamId: "other",
      event: { type: "done", statements: 2, succeeded: true },
    });
    socket().receive({
      type: "run",
      streamId: "r",
      event: { type: "done", statements: 1, succeeded: true },
    });
    await done;
    expect(events).toEqual([{ type: "done", statements: 1, succeeded: true }]);
  });

  it("retries a run whose start the server refused with TOO_MANY_STREAMS", async () => {
    vi.useFakeTimers();
    const c = client();
    void c.stream(runRequest("r"));
    socket().open();
    socket().receive({
      type: "run",
      streamId: "r",
      event: { type: "error", code: "TOO_MANY_STREAMS", message: "busy" },
    });
    await vi.advanceTimersByTimeAsync(10);
    expect(socket().starts()).toEqual(["r", "r"]);
  });

  it("sends a cancel frame for a run when its signal aborts", async () => {
    const c = client();
    const controller = new AbortController();
    const out = collect(c.stream(runRequest("r"), { signal: controller.signal }) as never);
    socket().open();
    controller.abort();
    expect(socket().sent.at(-1)).toEqual({ op: "cancel", streamId: "r" });
    expect(await out).toEqual([expect.objectContaining({ code: "CANCELLED" })]);
  });

  it("replaces a lone surrogate in the run's text before sending", () => {
    const c = client();
    void c.stream(runRequest("r", "SELECT '\uD83D'"));
    socket().open();
    const start = socket().sent[0] as { request: RunRequest };
    expect(start.request.params.params.text).toBe("SELECT '�'");
  });
});

describe("HttpCoreClient.stream of an assistant turn (phase 6 Task 7)", () => {
  function turnRequest(streamId: string): AiChatRequest {
    return {
      method: "ai",
      params: {
        method: "chat",
        params: {
          streamId,
          chatId: "chat-1",
          connectionId: "c-1",
          userMessage: { id: "u-1", content: "How many?" },
          assistantMessageId: "a-1",
          apiKey: "test-key-not-real",
        },
      },
    };
  }

  it("routes ai frames by stream id and ends at the turn's done", async () => {
    const c = client();
    const events: AiEvent[] = [];
    const done = (async () => {
      for await (const e of c.stream(turnRequest("t"))) events.push(e);
    })();
    socket().open();
    expect(socket().starts()).toEqual(["t"]);
    socket().receive({ type: "ai", streamId: "t", event: { type: "text", delta: "Hi" } });
    socket().receive({
      type: "ai",
      streamId: "other",
      event: { type: "error", code: "X", message: "not this one" },
    });
    const end: AiEvent = { type: "done", messages: [], seq: { epoch: "e", n: 1 }, stop: "end" };
    socket().receive({ type: "ai", streamId: "t", event: end });
    await done;
    expect(events).toEqual([{ type: "text", delta: "Hi" }, end]);
  });

  it("sends a cancel frame for a turn when its signal aborts", async () => {
    const c = client();
    const controller = new AbortController();
    const out = collect(c.stream(turnRequest("t"), { signal: controller.signal }) as never);
    socket().open();
    controller.abort();
    expect(socket().sent.at(-1)).toEqual({ op: "cancel", streamId: "t" });
    expect(await out).toEqual([expect.objectContaining({ code: "CANCELLED" })]);
  });

  it("ends a started turn with WS_CLOSED when the socket drops", async () => {
    const c = client();
    const out = collect(c.stream(turnRequest("t")) as never);
    socket().open();
    socket().drop();
    expect(await out).toEqual([expect.objectContaining({ code: "WS_CLOSED" })]);
  });
});
