/**
 * `CoreProvider` over a fake `CoreClient`: the `db` request shapes, stream
 * iteration (`selectStream`, and `selectReadOnly` collecting it into row
 * objects), cancelling through the signal or `onBatch`, and the `CANCELLED`
 * ending.
 */
import { describe, expect, it, vi } from "vitest";
import {
  cancelledEvent,
  StreamQueue,
  type CoreClient,
  type QueryStreamRequest,
  type StreamEvent,
} from "$lib/core";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import type { CoreResponse } from "$lib/types/generated/CoreResponse";
import { CoreProvider } from "./core-provider";

/** A client whose streams answer with `events` (then end), recording what it got. */
function fakeClient(events: StreamEvent[] | null = null) {
  const calls: CoreRequest[] = [];
  const streams: { request: QueryStreamRequest; signal?: AbortSignal; queue: StreamQueue }[] = [];
  const cancels: string[] = [];
  let answer: (request: CoreRequest) => unknown = () => null;
  const client: CoreClient = {
    call: vi.fn(async (request: CoreRequest) => {
      calls.push(request);
      return answer(request) as CoreResponse;
    }),
    stream(request, options = {}) {
      const id = request.params.params.streamId;
      const queue = new StreamQueue(() => cancels.push(id));
      options.signal?.addEventListener("abort", () => {
        cancels.push(id);
        queue.push(cancelledEvent());
      });
      streams.push({ request, signal: options.signal, queue });
      if (events) for (const event of events) queue.push(event);
      return queue;
    },
    events: () => () => {},
  };
  return {
    client,
    calls,
    streams,
    cancels,
    answer: (fn: (request: CoreRequest) => unknown) => {
      answer = fn;
    },
  };
}

const batch = (
  columns: string[] | null,
  rows: unknown[][],
  isFinal: boolean,
  truncated?: boolean,
): StreamEvent =>
  ({
    type: "batch",
    columns,
    rows,
    is_final: isFinal,
    ...(truncated === undefined ? {} : { truncated }),
  }) as StreamEvent;

const provider = (client: CoreClient) => new CoreProvider(() => client);

describe("CoreProvider request/response calls", () => {
  it("connect sends db.connect and returns the connection id", async () => {
    const fake = fakeClient();
    fake.answer(() => ({
      method: "db",
      result: { method: "connect", result: { connectionId: "c-1" } },
    }));
    const request = { target: { type: "saved" as const, id: "row-1" }, secrets: { db: "pw" } };
    expect(await provider(fake.client).connect(request)).toBe("c-1");
    expect(fake.calls).toEqual([{ method: "db", params: { method: "connect", params: request } }]);
  });

  it("select decodes rows and dedupes column names", async () => {
    const fake = fakeClient();
    fake.answer(() => ({
      method: "db",
      result: {
        method: "query",
        result: { columns: ["id", "id"], rows: [[{ $sq: "bigint", v: "9007199254740993" }, 2]] },
      },
    }));
    expect(await provider(fake.client).select("c-1", "SELECT", [5n])).toEqual([
      { id: 9007199254740993n, id_2: 2 },
    ]);
    expect(fake.calls[0]).toEqual({
      method: "db",
      params: {
        method: "query",
        params: { connectionId: "c-1", sql: "SELECT", params: [{ $sq: "bigint", v: "5" }] },
      },
    });
  });

  it("execute maps the snake_case result", async () => {
    const fake = fakeClient();
    fake.answer(() => ({
      method: "db",
      result: { method: "execute", result: { rows_affected: 3, last_insert_id: null } },
    }));
    expect(await provider(fake.client).execute("c-1", "DELETE")).toEqual({
      rowsAffected: 3,
      lastInsertId: undefined,
    });
  });

  it("refuses a response for another method", async () => {
    const fake = fakeClient();
    fake.answer(() => ({ method: "db", result: { method: "test", result: null } }));
    await expect(provider(fake.client).disconnect("c-1")).rejects.toThrow("PROTOCOL_ERROR");
  });
});

describe("CoreProvider.selectStream", () => {
  it("sends db.queryStream with a fresh stream id, read-write", async () => {
    const fake = fakeClient([batch(["n"], [[1]], true), { type: "done" }]);
    const p = provider(fake.client);
    await p.selectStream("c-1", "SELECT 1", undefined, () => true);
    await p.selectStream("c-1", "SELECT 1", undefined, () => true);
    const [a, b] = fake.streams.map((s) => s.request.params.params);
    expect(a).toEqual({ connectionId: "c-1", streamId: a.streamId, sql: "SELECT 1", params: [] });
    expect(a.streamId).not.toBe(b.streamId);
  });

  it("hands each batch over, decoded, and resolves on done", async () => {
    const fake = fakeClient([
      batch(["n"], [[{ $sq: "bigint", v: "9007199254740993" }]], false),
      batch(null, [[2]], true),
      { type: "done" },
    ]);
    const seen: unknown[] = [];
    const outcome = await provider(fake.client).selectStream("c-1", "SELECT", [], (b) => {
      seen.push(b);
      return true;
    });
    expect(outcome).toEqual({ aborted: false });
    expect(seen).toEqual([
      { columns: ["n"], rows: [[9007199254740993n]], isFinal: false, truncated: undefined },
      { columns: null, rows: [[2]], isFinal: true, truncated: undefined },
    ]);
  });

  it("cancels when onBatch returns false", async () => {
    const fake = fakeClient([batch(["n"], [[1]], false)]);
    const outcome = await provider(fake.client).selectStream("c-1", "SELECT", [], () => false);
    expect(outcome).toEqual({ aborted: true });
    expect(fake.cancels).toEqual([fake.streams[0].request.params.params.streamId]);
  });

  it("cancels when the signal aborts mid-stream", async () => {
    const fake = fakeClient(null);
    const controller = new AbortController();
    const pending = provider(fake.client).selectStream(
      "c-1",
      "SELECT pg_sleep(5)",
      [],
      () => true,
      controller.signal,
    );
    await vi.waitFor(() => expect(fake.streams).toHaveLength(1));
    controller.abort();
    expect(await pending).toEqual({ aborted: true });
    expect(fake.cancels).toHaveLength(1);
  });

  it("reports a CANCELLED ending it didn't ask for as an error", async () => {
    // Core ended the stream with no done/error (someone else cancelled it).
    const fake = fakeClient([cancelledEvent()]);
    expect(await provider(fake.client).selectStream("c-1", "SELECT", [], () => true)).toEqual({
      aborted: false,
      error: "CANCELLED: The query was cancelled",
    });
  });

  it("reports an error event as CODE: message", async () => {
    const fake = fakeClient([{ type: "error", code: "QUERY_ERROR", message: "syntax" }]);
    expect(await provider(fake.client).selectStream("c-1", "SELEC", [], () => true)).toEqual({
      aborted: false,
      error: "QUERY_ERROR: syntax",
    });
  });

  it("doesn't start a stream for an already-aborted signal", async () => {
    const fake = fakeClient([]);
    const controller = new AbortController();
    controller.abort();
    expect(
      await provider(fake.client).selectStream("c-1", "S", [], () => true, controller.signal),
    ).toEqual({ aborted: true });
    expect(fake.streams).toEqual([]);
  });
});

describe("CoreProvider.selectReadOnly", () => {
  it("streams with readOnly: true and returns row objects", async () => {
    const fake = fakeClient([batch(["n", "s"], [[1, "a"]], true), { type: "done" }]);
    const result = await provider(fake.client).selectReadOnly("c-1", "SELECT 1");
    expect(result).toEqual({ rows: [{ n: 1, s: "a" }], truncated: false });
    expect(fake.streams[0].request.params.params).toMatchObject({
      connectionId: "c-1",
      sql: "SELECT 1",
      params: [],
      readOnly: true,
    });
    expect("maxRows" in fake.streams[0].request.params.params).toBe(false);
  });

  it("sends maxRows and reports the final batch's truncated", async () => {
    const fake = fakeClient([batch(["n"], [[1], [2]], true, true), { type: "done" }]);
    const result = await provider(fake.client).selectReadOnly("c-1", "SELECT", undefined, 2);
    expect(result).toEqual({ rows: [{ n: 1 }, { n: 2 }], truncated: true });
    expect(fake.streams[0].request.params.params).toMatchObject({ readOnly: true, maxRows: 2 });
  });

  it("isn't truncated when the final batch doesn't say so", async () => {
    const fake = fakeClient([
      batch(["n"], [[1]], false, true),
      batch(null, [[2]], true),
      { type: "done" },
    ]);
    const result = await provider(fake.client).selectReadOnly("c-1", "SELECT", undefined, 5);
    expect(result).toEqual({ rows: [{ n: 1 }, { n: 2 }], truncated: false });
  });

  it("returns [] for a result with no rows", async () => {
    const fake = fakeClient([batch([], [], true), { type: "done" }]);
    expect(await provider(fake.client).selectReadOnly("c-1", "SELECT")).toEqual({
      rows: [],
      truncated: false,
    });
  });

  it("rejects with the error event's message", async () => {
    const fake = fakeClient([
      { type: "error", code: "READ_ONLY", message: "cannot execute INSERT in a read-only txn" },
    ]);
    await expect(provider(fake.client).selectReadOnly("c-1", "SELECT f()")).rejects.toThrow(
      "READ_ONLY: cannot execute INSERT in a read-only txn",
    );
  });

  it("cancels when the signal aborts, and rejects with an AbortError", async () => {
    const fake = fakeClient(null);
    const controller = new AbortController();
    const pending = provider(fake.client).selectReadOnly("c-1", "SELECT 1", controller.signal);
    await vi.waitFor(() => expect(fake.streams).toHaveLength(1));
    controller.abort();
    await expect(pending).rejects.toMatchObject({ name: "AbortError" });
    expect(fake.cancels).toEqual([fake.streams[0].request.params.params.streamId]);
  });

  it("doesn't start a query for an already-aborted signal", async () => {
    const fake = fakeClient([]);
    const controller = new AbortController();
    controller.abort();
    await expect(
      provider(fake.client).selectReadOnly("c-1", "SELECT 1", controller.signal),
    ).rejects.toMatchObject({ name: "AbortError" });
    expect(fake.streams).toEqual([]);
  });
});
