/**
 * `TsQueryRunner`, the demo's runner, driven through the view model over a
 * mocked provider: the SQL module failing on the run path (it reports the
 * failure instead of running unchecked), Task 1's paging with parameters,
 * and the rules it shares with Core (confirmation, Decision 18's rows, the
 * strict count, history only for a run that fully succeeded).
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SchemaTable, StatementResult } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "../state.svelte.js";
import type { QueryHistoryManager } from "../query-history.svelte.js";
import type { PendingChangesManager } from "../pending-changes.svelte.js";
import type { PersistedQueryHistoryItem } from "$lib/types/generated/PersistedQueryHistoryItem";
import type { SeaquelWasm } from "$lib/wasm";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
vi.mock("$lib/stores/license-nudge.svelte.js", () => ({
  licenseNudgeStore: { recordQuery: vi.fn() },
}));

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

const { QueryExecutionManager } = await import("../query-execution.svelte.js");
const { TsQueryRunner } = await import("./ts-runner.js");
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

function setup(query: string, type = "postgres") {
  const connection = { id: "conn-1", type, name: "Local", providerConnectionId: "pc-1" };
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
  const select = vi.fn(async (..._a: unknown[]): Promise<Record<string, unknown>[]> => [
    { year: 2024 },
  ]);
  const execute = vi.fn(async (..._a: unknown[]) => ({ rowsAffected: 0 }));
  const selectStream = vi.fn(
    async (
      _id: string,
      _sql: string,
      _binds: unknown[] | undefined,
      onBatch: (b: { columns: string[] | null; rows: unknown[][]; isFinal: boolean }) => boolean,
    ) => {
      onBatch({ columns: ["a"], rows: [[1]], isFinal: true });
      return { aborted: false };
    },
  );
  const provider = { select, execute, selectStream };
  const appended: PersistedQueryHistoryItem[] = [];
  const runner = new TsQueryRunner({
    provider: provider as never,
    engine: type as never,
    paginate: async (sql, limit, offset) => `${sql} LIMIT ${limit} OFFSET ${offset}`,
    appendHistory: async (item) => {
      appended.push(item);
    },
  });
  const history = {
    contextFor: (id: string) => ({
      connectionId: id,
      connectionName: "Local",
      connectionLabels: [],
    }),
    insertRecorded: vi.fn(),
  };
  const pending = { isEnabled: () => false } as unknown as PendingChangesManager;
  const manager = new QueryExecutionManager(
    state,
    history as unknown as QueryHistoryManager,
    {} as ProviderRegistry,
    pending,
    async () => runner,
  );
  const results = () => state.queryTabsByProject.p[0].results ?? [];
  const executing = () => state.queryTabsByProject.p[0].isExecuting;
  const ran = () => select.mock.calls.length + execute.mock.calls.length;
  return {
    manager,
    results,
    executing,
    select,
    execute,
    selectStream,
    ran,
    state,
    appended,
    history,
  };
}

beforeEach(() => {
  failing.clear();
  vi.mocked(errorToast).mockClear();
});

describe("the SQL module on the run path", () => {
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

  it("reports a failed row-limit check as the statement's error (run all and at cursor)", async () => {
    failing.add("has_row_limit");
    const { manager, results, executing, ran } = setup("SELECT * FROM orders");
    await manager.execute("tab-1");
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toContain("unreachable");
    expect(executing()).toBe(false);
    await manager.executeCurrent("tab-1", 0);
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toContain("unreachable");
    expect(ran()).toBe(0);
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
    const { manager, ran, results } = setup("SELECT 1;\nDROP TABLE users");
    await manager.executeCurrent("tab-1", 3);
    expect(ran()).toBe(0);
    expect(results()[0].error).toContain("unreachable");
  });

  it("reports a failed split instead of running nothing silently", async () => {
    failing.add("split_statements");
    const { manager, results, executing, ran } = setup("SELECT 1; SELECT 2");
    await manager.execute("tab-1");
    expect(results()[0].isError).toBe(true);
    // The translated sentence, not the module's code.
    expect(results()[0].error).toMatch(/^Couldn't find the statement.*unreachable/);
    expect(executing()).toBe(false);
    expect(ran()).toBe(0);
  });

  it("doesn't run unconfirmed when the destructive check fails", async () => {
    failing.add("destructive_reason");
    const { manager, ran, results } = setup("DELETE FROM t");
    await manager.execute("tab-1", { confirmed: true });
    expect(ran()).toBe(0);
    expect(results()[0].error).toContain("unreachable");
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
    const params = [{ name: "p", value: new Uint8Array([1]) }];
    await manager.execute("tab-1", { params });
    expect(results()[0].isError).toBe(true);
    expect(results()[0].error).toMatch(/\{\{p\}\}/);
    await manager.executeCurrent("tab-1", 0, { params });
    expect(errorToast).toHaveBeenCalledTimes(1);
    expect(vi.mocked(errorToast).mock.calls[0][0]).toMatch(/\{\{p\}\}/);
    expect(ran()).toBe(0);
  });
});

// Paging re-runs a statement: it must send the SQL after `{{param}}`
// substitution with its bind values, not the typed text.
describe("paging with parameters", () => {
  // A page of `pageSize + 1` rows, so the result has a second page, and a
  // count of 3 for the count probe.
  function answerPages(select: ReturnType<typeof setup>["select"]) {
    select.mockImplementation(async (_id: unknown, sql: unknown) =>
      /count/i.test(String(sql)) ? [{ total: 3 }] : [{ a: 1 }, { a: 2 }, { a: 3 }],
    );
  }
  const selectCalls = (select: ReturnType<typeof setup>["select"]) =>
    select.mock.calls.map(([, sql, binds]) => [sql, binds]);

  it("pages a parameterised statement with its substituted SQL and binds", async () => {
    const { manager, results, select } = setup("SELECT * FROM t WHERE a = {{a}}");
    answerPages(select);
    await manager.executeCurrent("tab-1", 0, { pageSize: 2, params: [{ name: "a", value: 5 }] });
    expect(results()[0].isError).toBe(false);
    expect(results()[0].totalPages).toBe(2);
    select.mockClear();
    select.mockImplementation(async () => [{ a: 3 }]);
    await manager.goToPage("tab-1", 2, 0);
    expect(results()[0].isError).toBe(false);
    expect(selectCalls(select)).toEqual([["SELECT * FROM t WHERE a = $1 LIMIT 3 OFFSET 2", [5]]]);
  });

  it("a full re-paged page counts with the binds", async () => {
    const { manager, results, select } = setup("SELECT * FROM t WHERE a = {{a}}");
    answerPages(select);
    await manager.executeCurrent("tab-1", 0, { pageSize: 2, params: [{ name: "a", value: 5 }] });
    select.mockClear();
    await manager.goToPage("tab-1", 2, 0);
    expect(results()[0].isError).toBe(false);
    const calls = selectCalls(select);
    expect(calls).toHaveLength(2);
    expect(calls[0]).toEqual(["SELECT * FROM t WHERE a = $1 LIMIT 3 OFFSET 2", [5]]);
    expect(String(calls[1][0])).toMatch(/count/i);
    expect(String(calls[1][0])).toContain("$1");
    expect(calls[1][1]).toEqual([5]);
  });

  it("changing the page size keeps the binds", async () => {
    const { manager, results, select } = setup("SELECT * FROM t WHERE a = {{a}}");
    answerPages(select);
    await manager.executeCurrent("tab-1", 0, { pageSize: 2, params: [{ name: "a", value: 5 }] });
    select.mockClear();
    select.mockImplementation(async () => [{ a: 1 }]);
    await manager.setPageSize("tab-1", 500, 0);
    expect(results()[0].isError).toBe(false);
    expect(results()[0].page).toBe(1);
    expect(selectCalls(select)).toEqual([["SELECT * FROM t WHERE a = $1 LIMIT 501 OFFSET 0", [5]]]);
  });

  it("re-streams a row-limited parameterised statement with its binds", async () => {
    const { manager, results, select, selectStream } = setup(
      "SELECT * FROM t WHERE a = {{a}} LIMIT 5",
    );
    await manager.executeCurrent("tab-1", 0, { pageSize: 2, params: [{ name: "a", value: 5 }] });
    expect(selectStream).toHaveBeenCalledTimes(1);
    selectStream.mockClear();
    await manager.setPageSize("tab-1", 500, 0);
    expect(results()[0].isError).toBe(false);
    expect(select).not.toHaveBeenCalled();
    expect(selectStream).toHaveBeenCalledTimes(1);
    expect(selectStream.mock.calls[0].slice(1, 3)).toEqual([
      "SELECT * FROM t WHERE a = $1 LIMIT 5",
      [5],
    ]);
  });

  it("run all pages each statement with its own binds", async () => {
    const { manager, results, select } = setup(
      "SELECT * FROM t WHERE a = {{a}};\nSELECT * FROM u WHERE b = {{b}} AND a = {{a}}",
    );
    answerPages(select);
    const params = [
      { name: "a", value: 5 },
      { name: "b", value: "x" },
    ];
    await manager.execute("tab-1", { pageSize: 2, params });
    expect(results().map((r) => r.isError)).toEqual([false, false]);
    select.mockClear();
    select.mockImplementation(async () => [{ a: 3 }]);
    await manager.goToPage("tab-1", 2, 0);
    await manager.goToPage("tab-1", 2, 1);
    expect(results().map((r) => r.isError)).toEqual([false, false]);
    expect(selectCalls(select)).toEqual([
      ["SELECT * FROM t WHERE a = $1 LIMIT 3 OFFSET 2", [5]],
      ["SELECT * FROM u WHERE b = $1 AND a = $2 LIMIT 3 OFFSET 2", ["x", 5]],
    ]);
  });

  it("inlined parameters page with the inlined SQL", async () => {
    const { manager, results, select } = setup("SELECT * FROM t WHERE a = {{a}}", "mssql");
    answerPages(select);
    await manager.executeCurrent("tab-1", 0, { pageSize: 2, params: [{ name: "a", value: "x" }] });
    expect(results()[0].isError).toBe(false);
    select.mockClear();
    select.mockImplementation(async () => [{ a: 3 }]);
    await manager.goToPage("tab-1", 2, 0);
    expect(selectCalls(select)).toEqual([
      ["SELECT * FROM t WHERE a = N'x' LIMIT 3 OFFSET 2", undefined],
    ]);
  });

  it("statementSql stays the typed text, and history records it", async () => {
    const typed = "SELECT * FROM t WHERE a = {{a}}";
    const { manager, results, select, appended, history } = setup(typed);
    answerPages(select);
    await manager.executeCurrent("tab-1", 0, { pageSize: 2, params: [{ name: "a", value: 5 }] });
    expect(results()[0].statementSql).toBe(typed);
    expect(appended.map((h) => h.query)).toEqual([typed]);
    expect(history.insertRecorded).toHaveBeenCalledWith(appended[0]);
    await manager.goToPage("tab-1", 2, 0);
    expect(results()[0].statementSql).toBe(typed);
    // A page records nothing.
    expect(appended).toHaveLength(1);
  });
});

describe("rules shared with Core", () => {
  it("a destructive run isn't run until it is confirmed", async () => {
    const { manager, execute } = setup("DELETE FROM t");
    // As the file drop and the grid reruns call it.
    await manager.execute("tab-1");
    expect(execute).not.toHaveBeenCalled();
    expect(manager.pendingConfirm?.statements).toEqual([
      { index: 0, sql: "DELETE FROM t", reason: "delete_no_where" },
    ]);
    await manager.confirmPending("tab-1");
    expect(execute).toHaveBeenCalledOnce();
  });

  it("lists the first 100 destructive statements and their total, as Core does", async () => {
    const text = Array.from({ length: 150 }, (_, i) => `DROP TABLE t${i};`).join("\n");
    const { manager, execute } = setup(text);
    await manager.execute("tab-1");
    expect(execute).not.toHaveBeenCalled();
    expect(manager.pendingConfirm?.statements).toHaveLength(100);
    expect(manager.pendingConfirm?.statements[99]).toMatchObject({
      index: 99,
      sql: "DROP TABLE t99",
    });
    expect(manager.pendingConfirm?.total).toBe(150);
  });

  it("a row-returning other statement is shown with its rows and counts for history", async () => {
    const { manager, results, select, appended } = setup("SHOW search_path; SET x = 1");
    select.mockImplementation(async (_id: unknown, sql: unknown) =>
      String(sql).startsWith("SHOW") ? [{ search_path: "public" }] : [],
    );
    await manager.execute("tab-1");
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({
      columns: ["search_path"],
      rows: [["public"]],
      rowCount: 1,
      totalRows: 1,
    });
    expect(appended[0].rowCount).toBe(1);
  });

  it("DuckDB's lone Count answer stays a hidden utility result", async () => {
    const { manager, results, select } = setup(
      "CREATE TABLE t AS SELECT 1; SELECT 2 AS b",
      "duckdb",
    );
    select.mockImplementation(async (_id: unknown, sql: unknown) =>
      String(sql).startsWith("CREATE") ? [{ Count: 1 }] : [{ b: 2 }],
    );
    await manager.execute("tab-1");
    expect(results().map((r) => r.statementSql)).toEqual(["SELECT 2 AS b"]);
  });

  it("a count that isn't a whole number is estimated", async () => {
    const { manager, results, select } = setup("SELECT * FROM t");
    select.mockImplementation(async (_id: unknown, sql: unknown) =>
      /count/i.test(String(sql)) ? [{ total: "lots" }] : [{ a: 1 }, { a: 2 }, { a: 3 }],
    );
    await manager.execute("tab-1", { pageSize: 2 });
    expect(results()[0]).toMatchObject({ totalRows: 3, totalPages: 2, countEstimated: true });
  });

  it("records no history when a statement failed", async () => {
    const { manager, execute, appended, results } = setup(
      "SELECT 1 AS a; INSERT INTO t VALUES (1)",
    );
    execute.mockRejectedValue(new Error("boom"));
    await manager.execute("tab-1");
    expect(results()[1].error).toBe("boom");
    expect(appended).toEqual([]);
  });

  it("a comment-only buffer at the cursor runs nothing", async () => {
    const { manager, ran, appended } = setup("-- nothing here");
    await manager.executeCurrent("tab-1", 3);
    expect(ran()).toBe(0);
    expect(appended).toEqual([]);
  });

  it("Stop ends a stream and runs nothing after it", async () => {
    const { manager, results, selectStream, select } = setup(
      "SELECT g FROM t LIMIT 5; SELECT 1 AS a",
    );
    let release: () => void = () => {};
    selectStream.mockImplementation(async (_id, _sql, _binds, onBatch, signal?: AbortSignal) => {
      onBatch({ columns: ["g"], rows: [[1]], isFinal: false });
      await new Promise<void>((resolve) => {
        release = resolve;
        signal?.addEventListener("abort", () => resolve());
      });
      return { aborted: true };
    });
    const running = manager.execute("tab-1");
    await vi.waitFor(() => expect(results()[0]?.rows).toEqual([[1]]));
    manager.cancelStream("tab-1");
    release();
    await running;
    expect(results()).toHaveLength(1);
    expect(results()[0].isStreaming).toBe(false);
    expect(select).not.toHaveBeenCalled();
  });
});
