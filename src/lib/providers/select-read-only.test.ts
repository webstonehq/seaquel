/**
 * `selectReadOnly` on the two Rust transports: the Tauri channel
 * (`db_query_stream` with `readOnly`) and the WebSocket (`"read_only"` in the
 * first frame). Both run the same stream code as `selectStream`, with the
 * flag set, and collect it into row objects.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DbStreamEvent } from "./wire";

// -------- Tauri --------

type Args = Record<string, unknown> & {
  onEvent?: { onmessage: ((event: DbStreamEvent) => void) | null };
};

const tauri = vi.hoisted(() => ({
  /** Answers each `db_query_stream` invoke with these events, if set. */
  events: null as DbStreamEvent[] | null,
  calls: [] as { cmd: string; args: Record<string, unknown> }[],
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: tauri.Channel,
  invoke: vi.fn(async (cmd: string, args: Args) => {
    tauri.calls.push({ cmd, args });
    if (cmd === "db_query_stream" && tauri.events) {
      for (const event of tauri.events) args.onEvent?.onmessage?.(event);
    }
  }),
}));

import { UnifiedTauriProvider } from "./unified-tauri-provider";
import { HttpProvider } from "./http-provider";

function streamCall() {
  const call = tauri.calls.find((c) => c.cmd === "db_query_stream");
  if (!call) throw new Error("no db_query_stream invoke");
  return call.args;
}

const batch = (columns: string[] | null, rows: unknown[][], isFinal: boolean): DbStreamEvent =>
  ({ type: "batch", columns, rows, is_final: isFinal }) as DbStreamEvent;

beforeEach(() => {
  tauri.events = null;
  tauri.calls = [];
});

describe("UnifiedTauriProvider.selectReadOnly", () => {
  it("streams with readOnly: true and returns row objects", async () => {
    tauri.events = [batch(["n", "s"], [[1, "a"]], true), { type: "done" }];
    const rows = await new UnifiedTauriProvider().selectReadOnly("pc-1", "SELECT 1");
    expect(rows).toEqual([{ n: 1, s: "a" }]);
    const args = streamCall();
    expect(args).toMatchObject({
      connectionId: "pc-1",
      sql: "SELECT 1",
      values: [],
      readOnly: true,
    });
    expect(typeof args.queryId).toBe("string");
  });

  it("keeps selectStream read-write", async () => {
    tauri.events = [batch(["n"], [[1]], true), { type: "done" }];
    await new UnifiedTauriProvider().selectStream("pc-1", "SELECT 1", undefined, () => true);
    expect(streamCall().readOnly).toBe(false);
  });

  it("dedupes duplicate column names as select does", async () => {
    tauri.events = [batch(["id", "id", "id_2"], [[1, 2, 3]], true), { type: "done" }];
    expect(await new UnifiedTauriProvider().selectReadOnly("pc-1", "SELECT")).toEqual([
      { id: 1, id_3: 2, id_2: 3 },
    ]);
  });

  it("collects every batch and decodes tagged cells", async () => {
    tauri.events = [
      batch(["n"], [[{ $sq: "bigint", v: "9007199254740993" }]], false),
      batch(null, [[2]], true),
      { type: "done" },
    ];
    expect(await new UnifiedTauriProvider().selectReadOnly("pc-1", "SELECT")).toEqual([
      { n: 9007199254740993n },
      { n: 2 },
    ]);
  });

  it("returns [] for a result with no rows", async () => {
    tauri.events = [batch([], [], true), { type: "done" }];
    expect(await new UnifiedTauriProvider().selectReadOnly("pc-1", "SELECT")).toEqual([]);
  });

  it("rejects with the error frame's message", async () => {
    tauri.events = [
      {
        type: "error",
        code: "READ_ONLY",
        message: "cannot execute INSERT in a read-only transaction",
      } as DbStreamEvent,
    ];
    await expect(new UnifiedTauriProvider().selectReadOnly("pc-1", "SELECT f()")).rejects.toThrow(
      "READ_ONLY: cannot execute INSERT in a read-only transaction",
    );
  });

  it("cancels by query id when the signal aborts, and rejects with an AbortError", async () => {
    // No events: the query is still running when the signal aborts.
    const controller = new AbortController();
    const pending = new UnifiedTauriProvider().selectReadOnly(
      "pc-1",
      "SELECT pg_sleep(5)",
      controller.signal,
    );
    await vi.waitFor(() => streamCall());
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
    const cancel = tauri.calls.find((c) => c.cmd === "db_cancel_stream");
    expect(cancel?.args).toEqual({ queryId: streamCall().queryId });
  });

  it("doesn't start a query for an already-aborted signal", async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(
      new UnifiedTauriProvider().selectReadOnly("pc-1", "SELECT 1", controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(tauri.calls).toEqual([]);
  });
});

// -------- WebSocket --------

class FakeWebSocket {
  static readonly CONNECTING = 0;
  static readonly OPEN = 1;
  static readonly CLOSING = 2;
  static readonly CLOSED = 3;
  static instances: FakeWebSocket[] = [];
  /** Sent as soon as the first frame arrives, if set. */
  static reply: DbStreamEvent[] | null = null;

  readyState = FakeWebSocket.CONNECTING;
  sent: string[] = [];
  closed = false;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: ((event: { reason: string }) => void) | null = null;

  constructor(readonly url: string) {
    FakeWebSocket.instances.push(this);
    queueMicrotask(() => {
      this.readyState = FakeWebSocket.OPEN;
      this.onopen?.();
    });
  }
  send(data: string) {
    this.sent.push(data);
    for (const event of FakeWebSocket.reply ?? []) {
      queueMicrotask(() => this.onmessage?.({ data: JSON.stringify(event) }));
    }
  }
  close() {
    this.closed = true;
    this.readyState = FakeWebSocket.CLOSED;
  }
  firstFrame(): Record<string, unknown> {
    return JSON.parse(this.sent[0]) as Record<string, unknown>;
  }
}

describe("HttpProvider.selectReadOnly", () => {
  beforeEach(() => {
    FakeWebSocket.instances = [];
    FakeWebSocket.reply = null;
    vi.stubGlobal("WebSocket", FakeWebSocket);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  const provider = () => new HttpProvider({ baseUrl: "http://seaquel.test" });

  it("sends read_only: true in the first frame and returns row objects", async () => {
    FakeWebSocket.reply = [batch(["n"], [[1]], true), { type: "done" }];
    expect(await provider().selectReadOnly("pc-1", "SELECT 1")).toEqual([{ n: 1 }]);
    const ws = FakeWebSocket.instances[0];
    expect(ws.url).toBe("ws://seaquel.test/api/db/stream");
    expect(ws.firstFrame()).toMatchObject({
      connection_id: "pc-1",
      sql: "SELECT 1",
      values: [],
      read_only: true,
    });
  });

  it("keeps selectStream read-write", async () => {
    FakeWebSocket.reply = [batch(["n"], [[1]], true), { type: "done" }];
    await provider().selectStream("pc-1", "SELECT 1", undefined, () => true);
    expect(FakeWebSocket.instances[0].firstFrame().read_only).toBe(false);
  });

  it("dedupes duplicate column names as select does", async () => {
    FakeWebSocket.reply = [batch(["a", "a"], [[1, 2]], true), { type: "done" }];
    expect(await provider().selectReadOnly("pc-1", "SELECT")).toEqual([{ a: 1, a_2: 2 }]);
  });

  it("rejects with the error frame's message", async () => {
    FakeWebSocket.reply = [
      {
        type: "error",
        code: "READ_ONLY",
        message: "Only read-only SELECT queries are permitted",
      } as DbStreamEvent,
    ];
    await expect(provider().selectReadOnly("pc-1", "DELETE FROM t")).rejects.toThrow(
      "READ_ONLY: Only read-only SELECT queries are permitted",
    );
  });

  it("closes the socket when the signal aborts, and rejects with an AbortError", async () => {
    const controller = new AbortController();
    const pending = provider().selectReadOnly("pc-1", "SELECT SLEEP(5)", controller.signal);
    await vi.waitFor(() => expect(FakeWebSocket.instances[0]?.sent).toHaveLength(1));
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
    expect(FakeWebSocket.instances[0].closed).toBe(true);
  });

  it("doesn't open a socket for an already-aborted signal", async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(
      provider().selectReadOnly("pc-1", "SELECT 1", controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(FakeWebSocket.instances).toEqual([]);
  });
});
