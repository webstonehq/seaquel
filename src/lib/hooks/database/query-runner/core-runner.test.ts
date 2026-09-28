/** `CoreQueryRunner`: `db.run` and `db.page` through the page's `CoreClient`. */
import { describe, expect, it, vi } from "vitest";
import type { CoreClient, StreamRequest } from "$lib/core";
import { CoreQueryRunner } from "./core-runner";

describe("CoreQueryRunner", () => {
  it("streams db.run and db.page with the caller's signal", () => {
    const stream = vi.fn((_request: StreamRequest, _options?: { signal?: AbortSignal }) => ({
      async *[Symbol.asyncIterator]() {},
    }));
    const runner = new CoreQueryRunner(() => ({ stream }) as unknown as CoreClient);
    const signal = new AbortController().signal;
    const run = {
      connectionId: "c",
      streamId: "s",
      text: "SELECT 1",
      target: { type: "all" as const },
      pageSize: 100,
    };
    runner.run(run, signal);
    const page = {
      connectionId: "c",
      streamId: "t",
      source: { sql: "SELECT 1", params: [] },
      page: 2,
      pageSize: 100,
    };
    runner.page(page, signal);
    expect(stream.mock.calls).toEqual([
      [{ method: "db", params: { method: "run", params: run } }, { signal }],
      [{ method: "db", params: { method: "page", params: page } }, { signal }],
    ]);
  });
});
