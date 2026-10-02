/**
 * The bridge's own rules, over a fake DuckDB-WASM. Its behaviour against a
 * real one is the engine suite's (`engine-duckdb-browser.test.ts`).
 */
import { describe, expect, it } from "vitest";
import { makeDuckDbBridge, type BridgeDuckDb } from "./duckdb-bridge";

/**
 * DuckDB-WASM once its worker is gone (`terminate()`, or a worker that
 * died): every request resolves `undefined` at once instead of posting.
 */
function detached(): BridgeDuckDb {
  const nothing = async () => undefined as never;
  return {
    connectInternal: nothing,
    runQuery: nothing,
    startPendingQuery: nothing,
    pollPendingQuery: nothing,
    fetchQueryResults: nothing,
    cancelPendingQuery: nothing,
    disconnect: nothing,
  };
}

describe("a DuckDB whose worker is gone", () => {
  it("fails every request instead of answering 'not yet'", async () => {
    // `null` from startPending, pollPending or fetchChunk means "not yet",
    // so the driver asks again: `undefined` read that way polled forever.
    const bridge = makeDuckDbBridge(detached());
    await expect(bridge.connect()).rejects.toThrow("DuckDB isn't running");
    await expect(bridge.runQuery(1, "SELECT 1")).rejects.toThrow("DuckDB isn't running");
    await expect(bridge.startPending(1, "SELECT 1")).rejects.toThrow("DuckDB isn't running");
    await expect(bridge.pollPending(1)).rejects.toThrow("DuckDB isn't running");
    await expect(bridge.fetchChunk(1)).rejects.toThrow("DuckDB isn't running");
  });

  it("still never rejects a cancel or a close of everything", async () => {
    const bridge = makeDuckDbBridge(detached());
    await expect(bridge.cancel(1)).resolves.toBe(false);
    await expect(bridge.closeAll()).resolves.toBeUndefined();
  });
});

/**
 * DuckDB-WASM whose worker was killed from outside (`worker.terminate()` on
 * the page's worker, or a crash): requests are posted and never answered.
 * `alive` decides whether `getVersion`, the bridge's ping, answers.
 */
function silent(alive: () => boolean): BridgeDuckDb & { answer(): void } {
  const never = () => new Promise<never>(() => {});
  let release: (() => void) | null = null;
  return {
    connectInternal: async () => 1,
    runQuery: never,
    startPendingQuery: () =>
      new Promise((resolve) => {
        release = () => resolve(new Uint8Array([1]));
      }),
    pollPendingQuery: never,
    fetchQueryResults: never,
    cancelPendingQuery: never,
    disconnect: never,
    getVersion: () => (alive() ? Promise.resolve("v1.4.3") : new Promise<string>(() => {})),
    answer: () => release?.(),
  };
}

describe("a DuckDB whose worker stopped answering (Task 7 probe, item 5)", () => {
  const liveness = { checkAfterMs: 20, pingTimeoutMs: 30 };

  it("fails the requests waiting on it, and every later one, with a clear error", async () => {
    const bridge = makeDuckDbBridge(
      silent(() => false),
      { liveness },
    );
    const waiting = bridge.pollPending(1);
    await expect(waiting).rejects.toThrow(/DuckDB stopped responding/);
    await expect(bridge.startPending(1, "SELECT 1")).rejects.toThrow(/DuckDB stopped responding/);
    await expect(bridge.connect()).rejects.toThrow(/DuckDB stopped responding/);
    await expect(bridge.cancel(1)).resolves.toBe(false);
    await expect(bridge.closeAll()).resolves.toBeUndefined();
  });

  it("leaves a slow request alone while the worker answers its pings", async () => {
    const db = silent(() => true);
    const bridge = makeDuckDbBridge(db, { liveness });
    const slow = bridge.startPending(1, "SELECT 1");
    await new Promise((resolve) => setTimeout(resolve, 200));
    db.answer();
    await expect(slow).resolves.toEqual(new Uint8Array([1]));
  });
});

describe("the liveness check's verdict (probe-fix review, item 1)", () => {
  const liveness = { checkAfterMs: 20, pingTimeoutMs: 30 };

  it("is undone when the late ping answers: the next request works", async () => {
    let pingAnswer: (() => void) | null = null;
    let pollAnswer: ((b: Uint8Array) => void) | null = null;
    const db: BridgeDuckDb = {
      connectInternal: async () => 7,
      runQuery: async () => new Uint8Array([1]),
      startPendingQuery: () => new Promise(() => {}),
      pollPendingQuery: () => new Promise((resolve) => (pollAnswer = resolve)),
      fetchQueryResults: async () => new Uint8Array(),
      cancelPendingQuery: async () => true,
      disconnect: async () => {},
      // Answers, but only after the bridge gave up on it.
      getVersion: () => new Promise((resolve) => (pingAnswer = () => resolve("v1.4.3"))),
    };
    const bridge = makeDuckDbBridge(db, { liveness });
    await expect(bridge.startPending(1, "SELECT 1")).rejects.toThrow(/DuckDB stopped responding/);
    pingAnswer!();
    await new Promise((resolve) => setTimeout(resolve, 0));
    // Alive after all: what failed stays failed, what comes next runs.
    await expect(bridge.connect()).resolves.toBe(7);
    const poll = bridge.pollPending(1);
    pollAnswer!(new Uint8Array([2]));
    await expect(poll).resolves.toEqual(new Uint8Array([2]));
  });

  it("isn't passed while a runQuery is running (it can't be interrupted)", async () => {
    // A worker busy in one long runQuery answers nothing else, pings included.
    let busy = true;
    const db: BridgeDuckDb = {
      connectInternal: async () => 1,
      runQuery: () =>
        new Promise((resolve) =>
          setTimeout(() => {
            busy = false;
            resolve(new Uint8Array([3]));
          }, 300),
        ),
      startPendingQuery: () => new Promise(() => {}),
      pollPendingQuery: () => new Promise(() => {}),
      fetchQueryResults: async () => new Uint8Array(),
      cancelPendingQuery: async () => true,
      disconnect: async () => {},
      getVersion: () => (busy ? new Promise(() => {}) : Promise.resolve("v1.4.3")),
    };
    const bridge = makeDuckDbBridge(db, { liveness });
    await expect(bridge.runQuery(1, "SELECT * FROM big")).resolves.toEqual(new Uint8Array([3]));
    await expect(bridge.connect()).resolves.toBe(1);
  });
});
