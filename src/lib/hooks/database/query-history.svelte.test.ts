/**
 * History is written by targeted calls (`append`, `setFavorite`), never by
 * replacing a connection's whole list (phase 5b Task 3). The in-memory list
 * is a cache, trimmed by the same rule `seaquel-storage` applies to the file.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";
import type { PersistedQueryHistoryItem, QueryHistoryItem } from "$lib/types";

/** Every storage call, as `repo.method` with its arguments. */
const calls: Array<{ call: string; args: unknown[] }> = [];
let failAppend = false;

vi.mock("$lib/storage", () => {
  const repo = (name: string) =>
    new Proxy(
      {},
      {
        get: (_t, method: string) =>
          vi.fn(async (...args: unknown[]) => {
            calls.push({ call: `${name}.${method}`, args });
            if (method === "append" && failAppend) {
              throw new Error("STORAGE_ERROR: FOREIGN KEY constraint failed");
            }
            return undefined;
          }),
      },
    );
  const storage = new Proxy({}, { get: (_t, name: string) => repo(name) });
  return { getStorage: () => storage };
});
const recordQuery = vi.fn();
vi.mock("$lib/stores/license-nudge.svelte.js", () => ({
  licenseNudgeStore: { recordQuery: () => recordQuery() },
}));
const logged: unknown[][] = [];
vi.mock("$lib/utils/logger", () => ({
  log: {
    debug: vi.fn(),
    error: vi.fn((...a: unknown[]) => logged.push(a)),
    info: vi.fn(),
    warn: vi.fn(),
    trace: vi.fn(),
  },
}));

const { QueryHistoryManager, HISTORY_KEEP } = await import("./query-history.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");

const LABELS = [{ id: "l1", name: "Prod", color: "red", isPredefined: false }];

function setup() {
  const state = new DatabaseState();
  state.activeConnectionId = "c1";
  const history = new QueryHistoryManager(
    state,
    () => LABELS,
    () => "Prod DB",
  );
  return { state, history };
}

function cached(id: string, n: number, favorite = false): QueryHistoryItem {
  return {
    id,
    query: `SELECT ${n}`,
    timestamp: new Date(Date.UTC(2026, 0, 1, 0, 0, n)),
    executionTime: 1,
    rowCount: 1,
    connectionId: "c1",
    favorite,
    connectionLabelsSnapshot: [],
    connectionNameSnapshot: "Prod DB",
  };
}

const writes = () => calls.filter((c) => !/\.(load|get)/.test(c.call)).map((c) => c.call);

/** Waits for the fire-and-forget storage call to settle. */
const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  calls.length = 0;
  logged.length = 0;
  failAppend = false;
  recordQuery.mockClear();
});

describe("QueryHistoryManager", () => {
  it("a recorded row goes on top of the cache and writes nothing", async () => {
    const { state, history } = setup();
    state.queryHistoryByConnection = { c1: [cached("old", 1)] };
    history.insertRecorded({
      id: "hist-new",
      query: "SELECT 1",
      timestamp: "2026-10-03T00:00:00.000Z",
      executionTime: 1,
      rowCount: 1,
      connectionId: "c1",
      favorite: false,
      connectionLabelsSnapshot: LABELS,
      connectionNameSnapshot: "Prod DB",
    });
    await settle();
    expect(calls).toEqual([]);
    expect(state.queryHistoryByConnection.c1.map((h) => h.id)).toEqual(["hist-new", "old"]);
  });

  it("the favourite toggle sends setFavorite for that id", async () => {
    const { state, history } = setup();
    state.queryHistoryByConnection = { c1: [cached("h1", 1), cached("h2", 2)] };

    history.toggleQueryFavorite("h2");
    await settle();
    expect(calls.map((c) => [c.call, ...c.args])).toEqual([
      ["queryHistory.setFavorite", "h2", true],
    ]);
    expect(state.queryHistoryByConnection.c1.map((h) => h.favorite)).toEqual([false, true]);

    history.toggleQueryFavorite("h2");
    await settle();
    expect(calls.at(-1)?.args).toEqual(["h2", false]);
    expect(writes()).not.toContain("queryHistory.replaceAll");
  });

  it("toggling an unknown id sends nothing", async () => {
    const { state, history } = setup();
    state.queryHistoryByConnection = { c1: [cached("h1", 1)] };
    history.toggleQueryFavorite("nope");
    await settle();
    expect(calls).toEqual([]);
  });

  it("the cache is trimmed like the file", async () => {
    const { state, history } = setup();
    // Newest first, as loaded: 510 rows, two favourites past the cap.
    const list = Array.from({ length: 510 }, (_, i) =>
      cached(`h${i}`, 1000 - i, i === 505 || i === 3),
    );
    state.queryHistoryByConnection = { c1: list };

    const appended = "hist-appended";
    history.insertRecorded({
      id: appended,
      query: "SELECT 1",
      timestamp: "2026-10-03T00:00:00.000Z",
      executionTime: 1,
      rowCount: 1,
      connectionId: "c1",
      favorite: false,
      connectionLabelsSnapshot: LABELS,
      connectionNameSnapshot: "Prod DB",
    });
    await settle();

    const ids = state.queryHistoryByConnection.c1.map((h) => h.id);
    // The new row and the 499 newest after it, then the favourite past them.
    expect(HISTORY_KEEP).toBe(500);
    expect(ids).toEqual([appended, ...list.slice(0, 499).map((h) => h.id), "h505"]);
  });

  it("insertRecorded puts a row Core appended at the top, without writing", async () => {
    const { state, history } = setup();
    state.queryHistoryByConnection = { c1: [cached("old", 1)] };
    history.insertRecorded({
      id: "hist-core",
      query: "SELECT 2",
      timestamp: "2026-02-03T04:05:06.789Z",
      executionTime: 3,
      rowCount: 4,
      connectionId: "c1",
      favorite: false,
      connectionLabelsSnapshot: LABELS,
      connectionNameSnapshot: "Prod DB",
    });
    await settle();
    expect(calls).toEqual([]);
    const top = state.queryHistoryByConnection.c1[0];
    expect(top.id).toBe("hist-core");
    expect(top.timestamp.toISOString()).toBe("2026-02-03T04:05:06.789Z");
    expect(state.queryHistoryByConnection.c1).toHaveLength(2);
  });
});

describe("restoreQueryHistory", () => {
  const persisted = (id: string, n: number, favorite = false): PersistedQueryHistoryItem => ({
    ...cached(id, n, favorite),
    timestamp: cached(id, n).timestamp.toISOString(),
  });

  it("keeps rows cached during the load on top, then the loaded ones", () => {
    const { state, history } = setup();
    // A query run at startup, before the load answered; the load already
    // holds one of the cached rows.
    history.insertRecorded(persisted("early", 50));
    history.insertRecorded(persisted("h1", 10));
    const restoration = new StateRestorationManager(state, {} as never);

    restoration.restoreQueryHistory("c1", [persisted("h1", 10), persisted("h0", 5)]);

    expect(state.queryHistoryByConnection.c1.map((h) => h.id)).toEqual(["early", "h1", "h0"]);
  });

  it("trims the merged list like the file", () => {
    const { state, history } = setup();
    history.insertRecorded(persisted("early", 9999));
    const restoration = new StateRestorationManager(state, {} as never);
    const loaded = Array.from({ length: 500 }, (_, i) => persisted(`h${i}`, 1000 - i, i === 499));

    restoration.restoreQueryHistory("c1", loaded);

    const ids = state.queryHistoryByConnection.c1.map((h) => h.id);
    // 500 kept (early + h0..h498), then h499, a favourite past the cap.
    expect(ids).toHaveLength(501);
    expect(ids[0]).toBe("early");
    expect(ids.at(-1)).toBe("h499");
  });
});
