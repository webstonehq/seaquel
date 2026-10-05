/**
 * Every `db` call and stream that fails
 * with `CONNECTION_NOT_FOUND` is reported once, centrally, with the Core
 * connection id it named, so the page can mark that connection
 * disconnected and reconnect it. The failed call itself is never retried.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { CoreCallError } from "$lib/storage/rust-client";
import type { CoreClient, StreamRequest } from "./client";
import { onConnectionNotFound, watchClient } from "./connection-watch";

function fakeClient(over: Partial<CoreClient> = {}): CoreClient {
  return {
    call: vi.fn(async () => {
      throw new CoreCallError({
        code: "CONNECTION_NOT_FOUND",
        message: "Connection not found",
      });
    }),
    stream: async function* () {
      yield { type: "error", code: "CONNECTION_NOT_FOUND", message: "gone" };
    } as unknown as CoreClient["stream"],
    events: () => () => {},
    onResubscribed: () => () => {},
    onEventsUnavailable: () => () => {},
    ...over,
  };
}

const db = (method: string, params: unknown) =>
  ({ method: "db", params: { method, params } }) as never;

let stop: (() => void) | undefined;
afterEach(() => stop?.());

describe("watchClient", () => {
  it("reports a call's CONNECTION_NOT_FOUND with its connection id, and still rejects", async () => {
    const seen: string[] = [];
    stop = onConnectionNotFound((id) => seen.push(id));
    const call = vi.fn(async () => {
      throw new CoreCallError({
        code: "CONNECTION_NOT_FOUND",
        message: "Connection not found",
      });
    });
    const client = watchClient(fakeClient({ call }));
    await expect(
      client.call(db("query", { connectionId: "pc-1", sql: "SELECT 1" })),
    ).rejects.toThrow("Connection not found");
    await expect(
      client.call(
        db("engine", {
          connectionId: "pc-2",
          request: { method: "listSchemas" },
        }),
      ),
    ).rejects.toThrow();
    expect(seen).toEqual(["pc-1", "pc-2"]);
    // Called once each: nothing is retried.
    expect(call).toHaveBeenCalledTimes(2);
  });

  it("doesn't report a disconnect, another code or a call that succeeds", async () => {
    const seen: string[] = [];
    stop = onConnectionNotFound((id) => seen.push(id));
    await expect(
      watchClient(fakeClient()).call(db("disconnect", { connectionId: "pc-1" })),
    ).rejects.toThrow();
    const other = fakeClient({
      call: vi.fn(async () => {
        throw new CoreCallError({ code: "QUERY_ERROR", message: "no" });
      }),
    });
    await expect(watchClient(other).call(db("query", { connectionId: "pc-1" }))).rejects.toThrow();
    const ok = fakeClient({ call: vi.fn(async () => ({}) as never) });
    await watchClient(ok).call(db("query", { connectionId: "pc-1" }));
    expect(seen).toEqual([]);
  });

  it("reports a stream that ends with CONNECTION_NOT_FOUND, passing its events through", async () => {
    const seen: string[] = [];
    stop = onConnectionNotFound((id) => seen.push(id));
    const request = {
      method: "db",
      params: {
        method: "run",
        params: { connectionId: "pc-9", streamId: "s1" },
      },
    } as unknown as StreamRequest;
    const events = [];
    for await (const event of watchClient(fakeClient()).stream(request)) events.push(event);
    expect(events).toEqual([{ type: "error", code: "CONNECTION_NOT_FOUND", message: "gone" }]);
    expect(seen).toEqual(["pc-9"]);
  });

  it("wraps each client once", () => {
    const inner = fakeClient();
    expect(watchClient(inner)).toBe(watchClient(inner));
  });
});
