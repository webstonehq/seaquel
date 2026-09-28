/**
 * The demo's real stack over a scripted DuckDB-WASM connection: the view
 * model, `getQueryRunner` (so `TsQueryRunner` with the demo's engine client
 * and its `paginate`), and the real `DuckDBProvider`, whose connection
 * answers from a script. It covers what the Core-provider replay can't:
 * DuckDB-WASM's plain `Error`s (no code) on a page, a write and a stream.
 */
import { describe, expect, it, vi } from "vitest";
import type { StatementResult } from "$lib/types";
import type { DatabaseState } from "../state.svelte.js";
import type { QueryHistoryManager } from "../query-history.svelte.js";
import type { PendingChangesManager } from "../pending-changes.svelte.js";
import type { ProviderRegistry } from "$lib/providers";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
vi.mock("$lib/stores/license-nudge.svelte.js", () => ({
  licenseNudgeStore: { recordQuery: vi.fn() },
}));
const append = vi.hoisted(() => vi.fn(async () => {}));
vi.mock("$lib/storage", () => ({ getStorage: () => ({ queryHistory: { append } }) }));

const { QueryExecutionManager } = await import("../query-execution.svelte.js");
const { DuckDBProvider } = await import("$lib/providers/duckdb-provider");

/** An Arrow result as DuckDB-WASM hands it over. */
function arrow(rows: Record<string, unknown>[]) {
  const names = rows.length > 0 ? Object.keys(rows[0]) : [];
  return {
    numRows: rows.length,
    schema: { fields: names.map((name) => ({ name })) },
    toArray: () => rows.map((r) => ({ toJSON: () => r })),
    getChildAt: (i: number) => ({ get: (r: number) => rows[r]?.[names[i]] }),
  };
}

/** Answers each `conn.query(sql)` with the first rule whose pattern matches. */
function setup(query: string, rules: [RegExp, Record<string, unknown>[] | Error][]) {
  const seen: string[] = [];
  const conn = {
    query: vi.fn(async (sql: string) => {
      seen.push(sql);
      const rule = rules.find(([re]) => re.test(sql));
      if (!rule) throw new Error(`unscripted: ${sql}`);
      if (rule[1] instanceof Error) throw rule[1];
      return arrow(rule[1]);
    }),
  };
  const provider = new DuckDBProvider();
  (provider as unknown as { connections: Map<string, unknown> }).connections.set("pc-1", conn);
  const connection = { id: "conn-1", type: "duckdb", name: "Demo", providerConnectionId: "pc-1" };
  const tab = { id: "tab-1", query } as { id: string; query: string; results?: StatementResult[] };
  const state = {
    activeProjectId: "p",
    queryTabsByProject: { p: [tab] },
    activeConnection: connection,
    activeConnectionId: "conn-1",
    connections: [connection],
    schemas: {},
  } as unknown as DatabaseState;
  const history = {
    contextFor: (id: string) => ({
      connectionId: id,
      connectionName: "Demo",
      connectionLabels: [],
    }),
    insertRecorded: vi.fn(),
  };
  // No runner factory: the demo's own choice (`getQueryRunner`).
  const manager = new QueryExecutionManager(
    state,
    history as unknown as QueryHistoryManager,
    { getForType: async () => provider } as unknown as ProviderRegistry,
    { isEnabled: () => false } as unknown as PendingChangesManager,
  );
  const results = () => state.queryTabsByProject.p[0].results ?? [];
  return { manager, results, seen, history };
}

describe("the demo runner over DuckDB-WASM", () => {
  it("runs a page, a write and a failing statement, showing DuckDB's own message", async () => {
    append.mockClear();
    const { manager, results, seen, history } = setup(
      "SELECT 1 AS a;\nINSERT INTO t VALUES (1);\nSELECT * FROM nope",
      [
        [/^SELECT 1 AS a/, [{ a: 1 }]],
        [/^INSERT/, [{ Count: 1n }]],
        [/nope/, new Error("Catalog Error: Table with name nope does not exist!")],
      ],
    );
    await manager.execute("tab-1");
    expect(seen[0]).toBe("SELECT 1 AS a LIMIT 101 OFFSET 0");
    expect(results()[0]).toMatchObject({ columns: ["a"], rows: [[1]], totalRows: 1 });
    expect(results()[1]).toMatchObject({ affectedRows: 1, rows: [["1 row(s) affected"]] });
    expect(results()[2]).toMatchObject({
      isError: true,
      error: "Catalog Error: Table with name nope does not exist!",
    });
    // A failed statement: no history.
    expect(append).not.toHaveBeenCalled();
    expect(history.insertRecorded).not.toHaveBeenCalled();
  });

  it("a failing stream shows DuckDB's message", async () => {
    const { manager, results } = setup("SELECT * FROM nope LIMIT 5", [
      [/nope/, new Error("Catalog Error: nope")],
    ]);
    await manager.execute("tab-1");
    expect(results()[0]).toMatchObject({
      kind: "stream",
      isError: true,
      error: "Catalog Error: nope",
    });
  });

  it("pages with an inlined parameter, hides SET, and records history", async () => {
    append.mockClear();
    const { manager, results, seen, history } = setup(
      "SET threads = 1;\nSELECT g FROM range(10) t(g) WHERE g > {{min}}",
      [
        [/^SET/, []],
        [/count/i, [{ total: 7n }]],
        [/OFFSET 0$/, [{ g: 3n }, { g: 4n }, { g: 5n }]],
        [/OFFSET 2$/, [{ g: 5n }, { g: 6n }, { g: 7n }]],
      ],
    );
    await manager.execute("tab-1", { pageSize: 2, params: [{ name: "min", value: 2 }] });
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({ rows: [[3n], [4n]], totalRows: 7, totalPages: 4 });
    expect(append).toHaveBeenCalledOnce();
    expect(history.insertRecorded).toHaveBeenCalledOnce();

    seen.length = 0;
    await manager.goToPage("tab-1", 2, 0);
    expect(seen[0]).toBe("SELECT g FROM range(10) t(g) WHERE g > 2 LIMIT 3 OFFSET 2");
    expect(results()[0]).toMatchObject({ page: 2, rows: [[5n], [6n]] });
    expect(append).toHaveBeenCalledOnce();
  });
});
