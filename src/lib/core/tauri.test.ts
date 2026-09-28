/**
 * `TauriCoreClient`: `core_stream` with the request as a JSON string and a
 * channel, stream iteration, abort (`db.cancel` through `core_call`), the
 * `CANCELLED` ending of a stream that resolves with no terminal event, and
 * one `core_events` registration per page.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CoreEvent } from "$lib/types/generated/CoreEvent";

type Listener = (event: CoreEvent) => void;

const tauri = vi.hoisted(() => ({
  calls: [] as { cmd: string; args: unknown }[],
  /** Resolves or rejects the next `core_stream` invoke. */
  settle: null as null | { resolve: (sent: number) => void; reject: (e: unknown) => void },
  Channel: class {
    onmessage: ((event: unknown) => void) | null = null;
  },
}));

vi.mock("@tauri-apps/api/core", () => ({
  Channel: tauri.Channel,
  invoke: vi.fn((cmd: string, args: unknown) => {
    tauri.calls.push({ cmd, args });
    if (cmd === "core_stream") {
      return new Promise<number>((resolve, reject) => {
        tauri.settle = { resolve, reject };
      });
    }
    if (cmd === "core_call") {
      return Promise.resolve({ method: "db", result: { method: "cancel", result: null } });
    }
    return Promise.resolve();
  }),
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const { TauriCoreClient } = await import("./tauri");
import type { PageRequest, QueryStreamRequest, RunRequest, StreamEvent } from "./client";
import type { RunEvent } from "$lib/types/generated/RunEvent";

function request(streamId = "s-1"): QueryStreamRequest {
  return {
    method: "db",
    params: {
      method: "queryStream",
      params: { connectionId: "c-1", streamId, sql: "SELECT 1", params: [] },
    },
  };
}

function streamCall() {
  const call = tauri.calls.find((c) => c.cmd === "core_stream");
  if (!call) throw new Error("no core_stream invoke");
  return call.args as { request: string; channel: { onmessage: Listener } };
}

/** Push `event` for `streamId` through the stream's channel. */
function send(event: StreamEvent, streamId = "s-1") {
  streamCall().channel.onmessage({ type: "stream", streamId, event });
}

async function collect(iterable: AsyncIterable<StreamEvent>): Promise<StreamEvent[]> {
  const out: StreamEvent[] = [];
  for await (const event of iterable) out.push(event);
  return out;
}

const decode = (body: unknown) => JSON.parse(new TextDecoder().decode(body as Uint8Array));

beforeEach(() => {
  tauri.calls = [];
  tauri.settle = null;
});

describe("TauriCoreClient.stream", () => {
  it("sends the request as a JSON string with a channel, and yields its events", async () => {
    const client = new TauriCoreClient();
    const events = collect(client.stream(request()));
    expect(JSON.parse(streamCall().request)).toEqual(request());
    send({ type: "batch", columns: ["n"], rows: [[1]], is_final: true });
    send({ type: "done" });
    tauri.settle?.resolve(2);
    expect(await events).toEqual([
      { type: "batch", columns: ["n"], rows: [[1]], is_final: true },
      { type: "done" },
    ]);
  });

  it("ignores another stream's events on the channel", async () => {
    const client = new TauriCoreClient();
    const events = collect(client.stream(request()));
    send({ type: "done" }, "other");
    send({ type: "error", code: "QUERY_ERROR", message: "x" });
    expect(await events).toEqual([{ type: "error", code: "QUERY_ERROR", message: "x" }]);
  });

  it("ends as CANCELLED when core_stream resolves with no done or error", async () => {
    const client = new TauriCoreClient();
    const events = collect(client.stream(request()));
    send({ type: "batch", columns: ["n"], rows: [], is_final: false });
    tauri.settle?.resolve(1);
    const out = await events;
    expect(out.at(-1)).toMatchObject({ type: "error", code: "CANCELLED" });
  });

  it("waits for every event core_stream says it sent, however late", async () => {
    const client = new TauriCoreClient();
    const events = collect(client.stream(request()));
    send({ type: "batch", columns: ["n"], rows: [[1]], is_final: false });
    // The reply overtook the last two events.
    tauri.settle?.resolve(3);
    await new Promise((r) => setTimeout(r, 20));
    send({ type: "batch", columns: null, rows: [[2]], is_final: true });
    send({ type: "done" });
    expect((await events).map((e) => e.type)).toEqual(["batch", "batch", "done"]);
  });

  it("counts every channel message, whatever stream id it names", async () => {
    const client = new TauriCoreClient();
    const events = collect(client.stream(request()));
    send({ type: "batch", columns: ["n"], rows: [], is_final: false }, "other");
    tauri.settle?.resolve(1);
    expect(await events).toEqual([expect.objectContaining({ code: "CANCELLED" })]);
  });

  it("ends as CANCELLED after the safety wait when counted events never come", async () => {
    vi.useFakeTimers();
    try {
      const client = new TauriCoreClient(1_000);
      const events = collect(client.stream(request()));
      tauri.settle?.resolve(2);
      await vi.advanceTimersByTimeAsync(999);
      send({ type: "batch", columns: ["n"], rows: [], is_final: false });
      // The message started the wait again.
      await vi.advanceTimersByTimeAsync(999);
      await vi.advanceTimersByTimeAsync(1);
      const out = await events;
      expect(out.map((e) => e.type)).toEqual(["batch", "error"]);
      expect(out[1]).toMatchObject({ code: "CANCELLED" });
    } finally {
      vi.useRealTimers();
    }
  });

  it("the safety wait is an idle wait: slow events keep the stream going", async () => {
    vi.useFakeTimers();
    try {
      const client = new TauriCoreClient(1_000);
      const events = collect(client.stream(request()));
      tauri.settle?.resolve(4);
      // Each under the wait apart, more than it in total.
      for (let i = 0; i < 3; i++) {
        await vi.advanceTimersByTimeAsync(900);
        send({ type: "batch", columns: i === 0 ? ["n"] : null, rows: [[i]], is_final: i === 2 });
      }
      await vi.advanceTimersByTimeAsync(900);
      send({ type: "done" });
      expect((await events).map((e) => e.type)).toEqual(["batch", "batch", "batch", "done"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("ends as CANCELLED only once the counted events are in", async () => {
    const client = new TauriCoreClient();
    const events = collect(client.stream(request()));
    tauri.settle?.resolve(1);
    await new Promise((r) => setTimeout(r, 20));
    send({ type: "batch", columns: ["n"], rows: [[1]], is_final: false });
    const out = await events;
    expect(out.map((e) => e.type)).toEqual(["batch", "error"]);
    expect(out[1]).toMatchObject({ code: "CANCELLED" });
  });

  it("aborting sends db.cancel with the stream id and ends as CANCELLED", async () => {
    const client = new TauriCoreClient();
    const controller = new AbortController();
    const events = collect(client.stream(request("s-9"), { signal: controller.signal }));
    controller.abort();
    const out = await events;
    expect(out).toEqual([expect.objectContaining({ type: "error", code: "CANCELLED" })]);
    const cancel = tauri.calls.find((c) => c.cmd === "core_call");
    expect(decode(cancel?.args)).toEqual({
      method: "db",
      params: { method: "cancel", params: { streamId: "s-9" } },
    });
    // Late events after the cancel are dropped.
    send({ type: "done" }, "s-9");
  });

  it("stopping the iteration early cancels too", async () => {
    const client = new TauriCoreClient();
    const iterable = client.stream(request());
    send({ type: "batch", columns: ["n"], rows: [[1]], is_final: false });
    for await (const _event of iterable) break;
    expect(tauri.calls.some((c) => c.cmd === "core_call")).toBe(true);
  });

  it("doesn't start a stream for an already-aborted signal", async () => {
    const controller = new AbortController();
    controller.abort();
    const out = await collect(
      new TauriCoreClient().stream(request(), { signal: controller.signal }),
    );
    expect(out).toEqual([expect.objectContaining({ code: "CANCELLED" })]);
    expect(tauri.calls).toEqual([]);
  });

  it("turns a rejected core_stream into an error event", async () => {
    const client = new TauriCoreClient();
    const events = collect(client.stream(request()));
    tauri.settle?.reject({ code: "INVALID_ARGUMENT", message: "not a stream" });
    expect(await events).toEqual([
      { type: "error", code: "INVALID_ARGUMENT", message: "not a stream" },
    ]);
  });
});

describe("TauriCoreClient.events", () => {
  it("registers core_events once and delivers connectionClosed to every handler", () => {
    const client = new TauriCoreClient();
    const a = vi.fn();
    const b = vi.fn();
    client.events(a);
    const stopB = client.events(b);
    const registrations = tauri.calls.filter((c) => c.cmd === "core_events");
    expect(registrations).toHaveLength(1);
    const { channel } = registrations[0].args as { channel: { onmessage: Listener } };
    const closed: CoreEvent = {
      type: "connectionClosed",
      connectionId: "c-1",
      code: "CONNECTION_CLOSED",
      message: "lost",
    };
    channel.onmessage(closed);
    expect(a).toHaveBeenCalledWith(closed);
    expect(b).toHaveBeenCalledWith(closed);
    stopB();
    channel.onmessage(closed);
    expect(a).toHaveBeenCalledTimes(2);
    expect(b).toHaveBeenCalledTimes(1);
  });
});

describe("TauriCoreClient.stream of a run (phase 5b)", () => {
  function runRequest(streamId = "r-1", text = "SELECT 1"): RunRequest {
    return {
      method: "db",
      params: {
        method: "run",
        params: { connectionId: "c-1", streamId, text, target: { type: "all" }, pageSize: 100 },
      },
    };
  }

  it("yields the run's events and ends at its done", async () => {
    const client = new TauriCoreClient();
    const events: RunEvent[] = [];
    const done = (async () => {
      for await (const e of client.stream(runRequest())) events.push(e);
    })();
    const { channel } = streamCall();
    const start: RunEvent = {
      type: "statementStart",
      index: 0,
      sql: "SELECT 1",
      source: { sql: "SELECT 1", params: [] },
      queryType: "select",
      kind: "page",
      page: 1,
      pageSize: 100,
    };
    channel.onmessage({ type: "run", streamId: "r-1", event: start });
    // Another stream's event on this channel is not this run's.
    channel.onmessage({
      type: "run",
      streamId: "other",
      event: { type: "done", statements: 9, succeeded: true },
    });
    channel.onmessage({
      type: "run",
      streamId: "r-1",
      event: { type: "done", statements: 1, succeeded: true },
    });
    tauri.settle?.resolve(3);
    await done;
    expect(events).toEqual([start, { type: "done", statements: 1, succeeded: true }]);
  });

  it("ends a run refused before its first statement with that error", async () => {
    const client = new TauriCoreClient();
    const events: RunEvent[] = [];
    const done = (async () => {
      for await (const e of client.stream(runRequest())) events.push(e);
    })();
    streamCall().channel.onmessage({
      type: "run",
      streamId: "r-1",
      event: { type: "error", code: "CONFIRM_REQUIRED", message: "confirm", destructive: [] },
    });
    tauri.settle?.resolve(1);
    await done;
    expect(events).toEqual([
      { type: "error", code: "CONFIRM_REQUIRED", message: "confirm", destructive: [] },
    ]);
  });

  it("replaces a lone surrogate in the run's text, keeping its UTF-16 length", () => {
    const client = new TauriCoreClient();
    const text = "SELECT '\uD83D' AS a; SELECT 2 AS b";
    void client.stream(runRequest("r-1", text));
    const sent = JSON.parse(streamCall().request) as RunRequest;
    expect(sent.params.params.text).toBe("SELECT '�' AS a; SELECT 2 AS b");
    expect(sent.params.params.text.length).toBe(text.length);
  });

  it("replaces a lone surrogate in a page's SQL", () => {
    const client = new TauriCoreClient();
    const request: PageRequest = {
      method: "db",
      params: {
        method: "page",
        params: {
          connectionId: "c-1",
          streamId: "p-1",
          source: { sql: "SELECT '\uDC00'", params: [] },
          page: 2,
          pageSize: 10,
        },
      },
    };
    void client.stream(request);
    const sent = JSON.parse(streamCall().request) as PageRequest;
    expect(sent.params.params.source.sql).toBe("SELECT '�'");
  });
});
