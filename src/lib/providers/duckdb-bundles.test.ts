/**
 * DuckDB-WASM startup: web serves its own bundles, and a worker that fails
 * or never answers ends in an error instead of a spinner.
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import type { DuckDBBundles } from "@duckdb/duckdb-wasm";
import { absoluteUrl, duckdbBundles, startWithin } from "./duckdb-bundles";

const cdn: DuckDBBundles = {
  mvp: { mainModule: "https://cdn/mvp.wasm", mainWorker: "https://cdn/mvp.worker.js" },
  eh: { mainModule: "https://cdn/eh.wasm", mainWorker: "https://cdn/eh.worker.js" },
};

describe("duckdbBundles", () => {
  it("serves the web build's own copy, not the CDN", async () => {
    const jsDelivr = vi.fn(() => cdn);
    const bundles = await duckdbBundles(
      jsDelivr,
      async () => (await import("./duckdb-local-bundles")).LOCAL_BUNDLES,
    );
    expect(jsDelivr).not.toHaveBeenCalled();
    for (const b of [bundles.mvp, bundles.eh!]) {
      expect(b.mainModule).toMatch(/duckdb-(mvp|eh)\.wasm/);
      expect(b.mainWorker).toMatch(/duckdb-browser-(mvp|eh)\.worker\.js/);
      expect(b.mainModule).not.toMatch(/^https?:/);
    }
  });

  it("uses jsDelivr without a local copy (desktop, demo, and this test build)", async () => {
    expect(await duckdbBundles(() => cdn, null)).toBe(cdn);
    expect(await duckdbBundles(() => cdn)).toBe(cdn);
  });
});

describe("absoluteUrl", () => {
  it("resolves a root-relative asset against the page", () => {
    expect(
      absoluteUrl("/_app/immutable/assets/duckdb-eh.abc.wasm", "https://seaquel.example/app/x"),
    ).toBe("https://seaquel.example/_app/immutable/assets/duckdb-eh.abc.wasm");
    expect(absoluteUrl("https://cdn/eh.wasm", "https://seaquel.example/")).toBe(
      "https://cdn/eh.wasm",
    );
  });
});

describe("startWithin", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("resolves with the start's value and stops listening", async () => {
    const worker = new EventTarget();
    const remove = vi.spyOn(worker, "removeEventListener");
    await expect(startWithin(Promise.resolve("ok"), worker, 1000)).resolves.toBe("ok");
    expect(remove).toHaveBeenCalledWith("error", expect.any(Function));
  });

  it("passes the start's own failure through", async () => {
    await expect(
      startWithin(Promise.reject(new Error("bad wasm")), new EventTarget(), 1000),
    ).rejects.toThrow("bad wasm");
  });

  it("rejects on the worker's error event while the start hangs", async () => {
    const worker = new EventTarget();
    const started = startWithin(new Promise<never>(() => {}), worker, 60_000);
    const event = new Event("error");
    Object.defineProperty(event, "message", { value: "NetworkError: importScripts failed" });
    worker.dispatchEvent(event);
    await expect(started).rejects.toThrow(
      "DuckDB failed to start: NetworkError: importScripts failed",
    );
  });

  it("rejects after the timeout when nothing answers", async () => {
    vi.useFakeTimers();
    const started = startWithin(new Promise<never>(() => {}), new EventTarget(), 30_000);
    const assertion = expect(started).rejects.toThrow("DuckDB didn't start within 30 s");
    await vi.advanceTimersByTimeAsync(30_000);
    await assertion;
  });
});
