import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SchemaTable, StatementResult } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import type { SeaquelWasm } from "$lib/wasm";

vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({
    paginate: async (sql: string, limit: number, offset: number) =>
      `${sql} LIMIT ${limit} OFFSET ${offset}`,
  }),
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));

// The real module, with the exports named in `failing` trapping, as a panic
// in seaquel-wasm would.
const failing = new Set<string>();
vi.mock("$lib/wasm", async (importOriginal) => {
  const w = await importOriginal<typeof import("$lib/wasm")>();
  const trap = () => {
    throw new WebAssembly.RuntimeError("unreachable");
  };
  return {
    ...w,
    callWasm: <T>(fn: (m: SeaquelWasm) => T): T =>
      w.callWasm((m) =>
        fn(
          new Proxy(m, {
            get: (t, k) => (typeof k === "string" && failing.has(k) ? trap : Reflect.get(t, k)),
          }),
        ),
      ),
  };
});

const { QueryExecutionManager } = await import("./query-execution.svelte.js");
const { resolveQuery } = await import("./resolve-query.js");
const { errorToast } = await import("$lib/utils/toast");

const orders = {
  name: "orders",
  schema: "public",
  type: "table",
  columns: [
    { name: "id", type: "integer", nullable: false, isPrimaryKey: true, isForeignKey: false },
    { name: "created_at", type: "date", nullable: true, isPrimaryKey: false, isForeignKey: false },
  ],
  indexes: [],
} as unknown as SchemaTable;
// A table named like the column, so the old "first FROM anywhere" lookup had
// something to find.
const createdAt = { ...orders, name: "created_at" } as SchemaTable;

function setup(query: string) {
  const connection = {
    id: "conn-1",
    type: "postgres",
    name: "Local",
    providerConnectionId: "pc-1",
  };
  const tab: { id: string; query: string; results?: StatementResult[]; isExecuting?: boolean } = {
    id: "tab-1",
    query,
  };
  const state = {
    activeProjectId: "p",
    queryTabsByProject: { p: [tab] },
    activeConnection: connection,
    activeConnectionId: "conn-1",
    connections: [connection],
    schemas: { "conn-1": [createdAt, orders] },
  } as unknown as DatabaseState;
  const select = vi.fn(async () => [{ year: 2024 }]);
  const execute = vi.fn(async () => ({ rowsAffected: 0 }));
  const providers = {
    getForType: vi.fn(async () => ({ select, execute })),
  } as unknown as ProviderRegistry;
  const history = { addToHistory: vi.fn() } as unknown as QueryHistoryManager;
  const pending = { isEnabled: () => false } as unknown as PendingChangesManager;
  const manager = new QueryExecutionManager(state, history, providers, pending);
  const results = () => state.queryTabsByProject.p[0].results ?? [];
  const executing = () => state.queryTabsByProject.p[0].isExecuting;
  const ran = () => select.mock.calls.length + execute.mock.calls.length;
  return { manager, results, executing, select, ran, state };
}

beforeEach(() => {
  failing.clear();
  vi.mocked(errorToast).mockClear();
});

describe("query runner", () => {
  // Fix 12: the first top-level FROM, not the one inside EXTRACT(… FROM …).
  it("finds the source table of an EXTRACT query", async () => {
    const { manager, results } = setup("SELECT EXTRACT(YEAR FROM created_at) FROM orders");
    await manager.execute("tab-1");
    expect(results()[0].isError).toBe(false);
    expect(results()[0].sourceTable).toEqual({
      schema: "public",
      name: "orders",
      primaryKeys: ["id"],
    });
  });

  it("reports a failed row-limit check as the statement's error (run all)", async () => {
    failing.add("has_row_limit");
    const { manager, results, executing, select } = setup("SELECT * FROM orders");
    await manager.execute("tab-1");
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toContain("unreachable");
    expect(executing()).toBe(false);
    expect(select).not.toHaveBeenCalled();
  });

  it("reports a failed row-limit check as the statement's error (at cursor)", async () => {
    failing.add("has_row_limit");
    const { manager, results, executing, select } = setup("SELECT * FROM orders");
    await manager.executeCurrent("tab-1", 0);
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toContain("unreachable");
    expect(executing()).toBe(false);
    expect(select).not.toHaveBeenCalled();
  });

  it("reports a failed row-limit check when paging", async () => {
    const { manager, results, select } = setup("SELECT * FROM orders");
    await manager.execute("tab-1");
    expect(results()[0].isError).toBe(false);
    select.mockClear();
    failing.add("has_row_limit");
    await manager.goToPage("tab-1", 2, 0);
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toContain("unreachable");
    expect(select).not.toHaveBeenCalled();
  });

  // The review's probe: a `null` statement at the cursor used to run the
  // whole buffer.
  it("runs nothing at the cursor when the statement can't be found", async () => {
    failing.add("statement_at");
    const { manager, ran, state } = setup("SELECT 1;\nDROP TABLE users");
    await manager.executeCurrent("tab-1", 3);
    expect(errorToast).toHaveBeenCalledTimes(1);
    expect(ran()).toBe(0);
    expect(resolveQuery(state, "tab-1", 3)).toBeNull();
    expect(errorToast).toHaveBeenCalledTimes(2);
  });

  it("reports a failed split instead of running nothing silently", async () => {
    failing.add("split_statements");
    const { manager, results, executing, ran } = setup("SELECT 1; SELECT 2");
    await manager.execute("tab-1");
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toContain("unreachable");
    expect(executing()).toBe(false);
    expect(ran()).toBe(0);
  });

  // `"other"` would run a SELECT unpaged as a utility statement.
  it("reports a failed query-type check (run all and at cursor)", async () => {
    failing.add("query_type");
    const { manager, results, executing, ran } = setup("SELECT * FROM orders");
    await manager.execute("tab-1");
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toContain("unreachable");
    await manager.executeCurrent("tab-1", 0);
    expect(results()[0].isError).toBe(true);
    expect(executing()).toBe(false);
    expect(ran()).toBe(0);
  });

  it("reports a value that can't be substituted (run all and at cursor)", async () => {
    const { manager, results, ran } = setup("SELECT {{p}}");
    const values = [{ name: "p", value: new Uint8Array([1]) }];
    await manager.execute("tab-1", 1, undefined, values);
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toMatch(/\{\{p\}\}/);
    await manager.executeCurrent("tab-1", 0, 1, undefined, values);
    expect(errorToast).toHaveBeenCalledTimes(1);
    expect(vi.mocked(errorToast).mock.calls[0][0]).toMatch(/\{\{p\}\}/);
    expect(ran()).toBe(0);
  });
});
