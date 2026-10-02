/**
 * `QueryExecutionManager` as a view model over run events (phase 5b, Task 6):
 * a scripted `QueryRunner` hands it Core's events, and the tab's results,
 * pending changes, history cache and confirmation follow. The tab state is
 * real `$state`, so a write that skipped the proxy wouldn't show.
 *
 * Core's run itself is pinned by its replay (`seaquel-core/tests/run.rs`)
 * and, in the demo's page, by `src/lib/demo/duckdb-on-core.test.ts`.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SchemaTable, StatementResult } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import type { CoreClient, StreamRequest } from "$lib/core";
import type { RunEvent } from "$lib/types/generated/RunEvent";
import type { RunParams } from "$lib/types/generated/RunParams";
import type { PageParams } from "$lib/types/generated/PageParams";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
const recordQuery = vi.hoisted(() => vi.fn());
vi.mock("$lib/stores/license-nudge.svelte.js", () => ({
  licenseNudgeStore: { recordQuery },
}));

const { QueryExecutionManager, errorText } = await import("./query-execution.svelte.js");
const { CoreQueryRunner } = await import("./query-runner/core-runner.js");
const { StreamQueue, cancelledEvent } = await import("$lib/core/client");
const { errorToast } = await import("$lib/utils/toast");
const { toast } = await import("svelte-sonner");

type Queue = InstanceType<typeof StreamQueue<RunEvent>>;

/** A runner whose events the test pushes, one queue per run or page. */
class ScriptedRunner {
  runs: { params: RunParams; signal: AbortSignal; queue: Queue }[] = [];
  pages: { params: PageParams; signal: AbortSignal; queue: Queue }[] = [];
  private open(signal: AbortSignal): Queue {
    const queue = new StreamQueue<RunEvent>();
    if (signal.aborted) queue.pushError(cancelledEvent());
    signal.addEventListener("abort", () => queue.pushError(cancelledEvent()));
    return queue;
  }
  run(params: RunParams, signal: AbortSignal) {
    const queue = this.open(signal);
    this.runs.push({ params, signal, queue });
    return queue;
  }
  page(params: PageParams, signal: AbortSignal) {
    const queue = this.open(signal);
    this.pages.push({ params, signal, queue });
    return queue;
  }
}

const orders = {
  name: "orders",
  schema: "public",
  type: "table",
  columns: [
    { name: "id", type: "integer", nullable: false, isPrimaryKey: true, isForeignKey: false },
    { name: "total", type: "numeric", nullable: true, isPrimaryKey: false, isForeignKey: false },
  ],
  indexes: [],
} as unknown as SchemaTable;

interface Tab {
  id: string;
  query: string;
  results?: StatementResult[];
  activeResultIndex?: number;
  isExecuting?: boolean;
}

function setup(query = "SELECT 1 AS a", opts: { deferWrites?: boolean; runner?: unknown } = {}) {
  const connection = {
    id: "conn-1",
    type: "postgres",
    name: "Local",
    providerConnectionId: "pc-1",
  };
  const state = $state({
    activeProjectId: "p",
    queryTabsByProject: { p: [{ id: "tab-1", query } as Tab] },
    activeConnection: connection,
    activeConnectionId: "conn-1",
    connections: [connection],
    schemas: { "conn-1": [orders] },
  });
  const runner = (opts.runner as ScriptedRunner | undefined) ?? new ScriptedRunner();
  const history = {
    contextFor: (id: string) => ({
      connectionId: id,
      connectionName: "Local",
      connectionLabels: [],
    }),
    insertRecorded: vi.fn(),
  };
  const pending = {
    isEnabled: () => !!opts.deferWrites,
    addSql: vi.fn(),
    openSheet: vi.fn(),
  };
  const manager = new QueryExecutionManager(
    state as unknown as DatabaseState,
    history as unknown as QueryHistoryManager,
    {} as ProviderRegistry,
    pending as unknown as PendingChangesManager,
    async () => runner as never,
  );
  const tab = () => state.queryTabsByProject.p.find((t) => t.id === "tab-1");
  const results = () => tab()?.results ?? [];
  return { manager, runner, state, tab, results, history, pending };
}

/** Let the manager reach its runner (it awaits `runnerFor` first). */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

function push(queue: Queue, ...events: RunEvent[]) {
  for (const e of events) queue.push(e);
}

const start = (
  index: number,
  sql: string,
  kind: "page" | "stream" | "write" | "utility",
  extra: Partial<Extract<RunEvent, { type: "statementStart" }>> = {},
): RunEvent => ({
  type: "statementStart",
  index,
  sql,
  source: { sql, params: [] },
  queryType: kind === "write" ? "insert" : kind === "utility" ? "other" : "select",
  kind,
  page: 1,
  pageSize: 100,
  ...extra,
});
const batch = (columns: string[] | null, rows: unknown[][], isFinal = true): RunEvent => ({
  type: "batch",
  columns,
  rows,
  is_final: isFinal,
});
const stmtDone = (
  index: number,
  extra: Partial<Extract<RunEvent, { type: "statementDone" }>> = {},
): RunEvent => ({
  type: "statementDone",
  index,
  elapsedMs: 4.5,
  totalRows: 1,
  totalPages: 1,
  countEstimated: false,
  ...extra,
});
const done = (
  statements: number,
  extra: Partial<Extract<RunEvent, { type: "done" }>> = {},
): RunEvent => ({ type: "done", statements, succeeded: true, ...extra });

const historyRow = {
  id: "hist-1",
  query: "SELECT 1 AS a",
  timestamp: "2026-10-02T12:00:00.000Z",
  executionTime: 4.5,
  rowCount: 1,
  connectionId: "conn-1",
  favorite: false,
  connectionLabelsSnapshot: [],
  connectionNameSnapshot: "Local",
};

beforeEach(() => {
  vi.mocked(errorToast).mockClear();
  vi.mocked(toast.info).mockClear();
  recordQuery.mockClear();
});

describe("run events", () => {
  it("run all shows each statement as its events arrive", async () => {
    const { manager, runner, results, tab } = setup("SELECT 1 AS a;\nINSERT INTO t VALUES (1)");
    const running = manager.execute("tab-1");
    await settle();
    expect(tab()?.isExecuting).toBe(true);
    const { params, queue } = runner.runs[0];
    expect(params).toMatchObject({
      connectionId: "pc-1",
      text: "SELECT 1 AS a;\nINSERT INTO t VALUES (1)",
      target: { type: "all" },
      pageSize: 100,
      history: { connectionId: "conn-1", connectionName: "Local", connectionLabels: [] },
    });
    expect(params).not.toHaveProperty("confirmed");
    expect(params).not.toHaveProperty("params");

    push(queue, start(0, "SELECT 1 AS a", "page"));
    await settle();
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({ statementSql: "SELECT 1 AS a", isStreaming: true });

    push(queue, batch(["a", "a"], [[1, 2]]), stmtDone(0));
    await settle();
    expect(results()[0]).toMatchObject({
      columns: ["a", "a_2"],
      rows: [[1, 2]],
      rowCount: 1,
      totalRows: 1,
      isStreaming: false,
      pageSource: { sql: "SELECT 1 AS a", params: [] },
    });

    push(queue, start(1, "INSERT INTO t VALUES (1)", "write"));
    push(queue, stmtDone(1, { totalRows: 0, rowsAffected: 3, lastInsertId: 7 }));
    await settle();
    expect(results()).toHaveLength(2);
    expect(results()[1]).toMatchObject({
      columns: ["Result"],
      rows: [["3 row(s) affected"]],
      affectedRows: 3,
      lastInsertId: 7,
      pageSize: 1,
    });

    push(queue, done(2));
    await running;
    expect(tab()?.isExecuting).toBe(false);
  });

  it("utility results are hidden unless every result is one", async () => {
    const { manager, runner, results } = setup("SET x = 1; SELECT 1 AS a");
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SET x = 1", "utility"),
      stmtDone(0, { totalRows: 0 }),
      start(1, "SELECT 1 AS a", "page"),
      batch(["a"], [[1]]),
      stmtDone(1),
      done(2),
    );
    await running;
    expect(results().map((r) => r.statementSql)).toEqual(["SELECT 1 AS a"]);
    expect(results()[0].statementIndex).toBe(0);

    const again = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[1].queue,
      start(0, "SET x = 1", "utility"),
      stmtDone(0, { totalRows: 0 }),
      done(1),
    );
    await again;
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({ statementSql: "SET x = 1", isUtility: true });
  });

  it("a row-returning other statement is shown with its rows and counts for history", async () => {
    const { manager, runner, results, history } = setup(
      "WITH x AS (SELECT 1) SELECT * FROM x; SET y = 1",
    );
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "WITH x AS (SELECT 1) SELECT * FROM x", "utility"),
      batch(["?column?"], [[1], [2]]),
      stmtDone(0, { totalRows: 2 }),
      start(1, "SET y = 1", "utility"),
      stmtDone(1, { totalRows: 0 }),
      done(2, { history: { ...historyRow, rowCount: 2 } }),
    );
    await running;
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({ rows: [[1], [2]], rowCount: 2, totalRows: 2 });
    expect(results()[0].isUtility).toBeFalsy();
    expect(history.insertRecorded).toHaveBeenCalledWith({ ...historyRow, rowCount: 2 });
  });

  it("elapsed comes from statementDone", async () => {
    const { manager, runner, results } = setup("SELECT g FROM t LIMIT 5");
    const running = manager.execute("tab-1");
    await settle();
    const { queue } = runner.runs[0];
    push(queue, start(0, "SELECT g FROM t LIMIT 5", "stream"), batch(["g"], [[1]], false));
    await settle();
    // A live counter while it streams…
    expect(results()[0].executionTime).toBeGreaterThanOrEqual(0);
    push(queue, batch(null, [[2]]), stmtDone(0, { elapsedMs: 123.45, totalRows: 2 }), done(1));
    await running;
    // …replaced by Core's time.
    expect(results()[0].executionTime).toBe(123.45);
    expect(results()[0].rows).toEqual([[1], [2]]);
  });

  it("decodes rows from the cell wire format", async () => {
    const { manager, runner, results } = setup();
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT 1 AS a", "page"),
      batch(["a"], [[{ $sq: "bigint", v: "9007199254740993" }]]),
      stmtDone(0),
      done(1),
    );
    await running;
    expect(results()[0].rows).toEqual([[9007199254740993n]]);
  });

  it("source table and column sources resolve from Core's refs and the schema cache", async () => {
    const { manager, runner, results } = setup("SELECT id, total FROM orders");
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT id, total FROM orders", "page", {
        table: { table: "orders" },
        columnRefs: [{ table: "orders", column: "id" }, null],
      }),
      batch(["id", "total"], [[1, 2]]),
      stmtDone(0),
      done(1),
    );
    await running;
    expect(results()[0].sourceTable).toEqual({
      schema: "public",
      name: "orders",
      primaryKeys: ["id"],
    });
    expect(results()[0].columnSources).toEqual([
      { schema: "public", table: "orders", primaryKeys: ["id"], column: "id" },
      undefined,
    ]);
  });

  it("a statement error keeps a stream's rows and replaces a page with the error", async () => {
    const { manager, runner, results } = setup("SELECT g FROM t LIMIT 5; SELECT nope");
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT g FROM t LIMIT 5", "stream"),
      batch(["g"], [[1]], false),
      {
        type: "statementError",
        index: 0,
        code: "QUERY_ERROR",
        message: "division by zero",
        elapsedMs: 2,
      },
      start(1, "SELECT nope", "page"),
      { type: "statementError", index: 1, code: "QUERY_ERROR", message: "no column", elapsedMs: 1 },
      {
        type: "statementError",
        index: 2,
        code: "INVALID_PARAMETERS",
        message: "bad {{p}}",
        elapsedMs: 0,
        sql: "SELECT {{p}}",
      },
      done(3, { succeeded: false }),
    );
    await running;
    expect(results()[0]).toMatchObject({
      columns: ["g"],
      rows: [[1]],
      isError: true,
      error: "division by zero",
      totalRows: 1,
    });
    expect(results()[1]).toMatchObject({
      columns: ["Error"],
      error: "no column",
      isError: true,
      statementSql: "SELECT nope",
    });
    // A planned failure shows the bare message, with the statement as typed.
    expect(results()[2]).toMatchObject({
      error: "bad {{p}}",
      statementSql: "SELECT {{p}}",
      isError: true,
    });
    expect(results()[2].pageSource).toBeUndefined();
  });

  it("a run that fails before any statement shows its error", async () => {
    const { manager, runner, results, tab } = setup();
    const running = manager.execute("tab-1");
    await settle();
    push(runner.runs[0].queue, {
      type: "error",
      code: "CONNECTION_NOT_FOUND",
      message: "no such connection",
    });
    await running;
    expect(results()).toHaveLength(1);
    expect(results()[0].error).toBe("CONNECTION_NOT_FOUND: no such connection");
    expect(tab()?.isExecuting).toBe(false);
  });

  it("a run that fails mid-statement marks that statement", async () => {
    const { manager, runner, results } = setup("SELECT g FROM t LIMIT 5");
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT g FROM t LIMIT 5", "stream"),
      batch(["g"], [[1]], false),
      { type: "error", code: "WS_CLOSED", message: "closed" },
    );
    await running;
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({
      rows: [[1]],
      isError: true,
      isStreaming: false,
      error: "WS_CLOSED: closed",
    });
  });

  it("INVALID_PARAMETERS at the cursor is a toast and keeps the old results", async () => {
    const { manager, runner, results } = setup("SELECT {{p}}");
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT {{p}}", "page"),
      batch(["a"], [[1]]),
      stmtDone(0),
      done(1),
    );
    await running;
    const current = manager.executeCurrent("tab-1", 3, { params: [{ name: "p", value: 1n }] });
    await settle();
    expect(runner.runs[1].params.params).toEqual([{ name: "p", value: { $sq: "bigint", v: "1" } }]);
    expect(runner.runs[1].params.target).toEqual({ type: "current", cursor: 3 });
    push(runner.runs[1].queue, {
      type: "error",
      code: "INVALID_PARAMETERS",
      message: "can't bind {{p}}",
    });
    await current;
    expect(errorToast).toHaveBeenCalledWith("can't bind {{p}}");
    expect(results()[0].rows).toEqual([[1]]);
  });

  it("nothing to run is the no-statements toast", async () => {
    const { manager, runner, results } = setup("-- only a comment");
    const running = manager.execute("tab-1");
    await settle();
    push(runner.runs[0].queue, done(0, { succeeded: false }));
    await running;
    expect(toast.info).toHaveBeenCalledOnce();
    expect(results()).toEqual([]);
  });
});

describe("history and the nudge", () => {
  it("done inserts the history row and counts the nudge once", async () => {
    const { manager, runner, history } = setup();
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT 1 AS a", "page"),
      batch(["a"], [[1]]),
      stmtDone(0),
      done(1, { history: historyRow }),
    );
    await running;
    expect(history.insertRecorded).toHaveBeenCalledExactlyOnceWith(historyRow);
    expect(recordQuery).toHaveBeenCalledOnce();
  });

  it("a failed run counts no nudge and records nothing", async () => {
    const { manager, runner, history } = setup();
    const running = manager.execute("tab-1");
    await settle();
    push(runner.runs[0].queue, done(1, { succeeded: false }));
    await running;
    expect(history.insertRecorded).not.toHaveBeenCalled();
    expect(recordQuery).not.toHaveBeenCalled();
  });

  it("a page never records history", async () => {
    const { manager, runner, history } = setup();
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT 1 AS a", "page", { pageSize: 2 }),
      batch(["a"], [[1], [2]]),
      stmtDone(0, { totalRows: 5, totalPages: 3 }),
      done(1, { history: historyRow }),
    );
    await running;
    history.insertRecorded.mockClear();
    recordQuery.mockClear();

    const paging = manager.goToPage("tab-1", 2, 0);
    await settle();
    push(
      runner.pages[0].queue,
      start(0, "SELECT 1 AS a", "page", { page: 2, pageSize: 2 }),
      batch(["a"], [[3], [4]]),
      stmtDone(0, { totalRows: 5, totalPages: 3 }),
      { type: "done", statements: 1, succeeded: true },
    );
    await paging;
    expect(history.insertRecorded).not.toHaveBeenCalled();
    expect(recordQuery).not.toHaveBeenCalled();
  });
});

describe("paging", () => {
  async function paged(sql = "SELECT * FROM t WHERE a = $1") {
    const env = setup("SELECT * FROM t WHERE a = {{a}}");
    const running = env.manager.execute("tab-1", {
      pageSize: 2,
      params: [{ name: "a", value: 5 }],
    });
    await settle();
    push(
      env.runner.runs[0].queue,
      start(0, "SELECT * FROM t WHERE a = {{a}}", "page", {
        source: { sql, params: [5] },
        pageSize: 2,
      }),
      batch(["a"], [[1], [2]]),
      stmtDone(0, { totalRows: 3, totalPages: 2 }),
      done(1),
    );
    await running;
    return env;
  }

  it("a paged result keeps its page source and goToPage sends db.page with it", async () => {
    const { manager, runner, results } = await paged();
    expect(results()[0].pageSource).toEqual({ sql: "SELECT * FROM t WHERE a = $1", params: [5] });
    const paging = manager.goToPage("tab-1", 2, 0);
    await settle();
    expect(runner.pages[0].params).toMatchObject({
      connectionId: "pc-1",
      source: { sql: "SELECT * FROM t WHERE a = $1", params: [5] },
      page: 2,
      pageSize: 2,
    });
    // The rows it shows stay until the new page arrives.
    expect(results()[0].rows).toEqual([[1], [2]]);
    push(
      runner.pages[0].queue,
      start(0, "SELECT * FROM t WHERE a = $1", "page", {
        source: { sql: "SELECT * FROM t WHERE a = $1", params: [5] },
        page: 2,
        pageSize: 2,
      }),
      batch(["a"], [[3]]),
      stmtDone(0, { totalRows: 3, totalPages: 2, elapsedMs: 9 }),
      { type: "done", statements: 1, succeeded: true },
    );
    await paging;
    expect(results()[0]).toMatchObject({
      rows: [[3]],
      page: 2,
      totalRows: 3,
      executionTime: 9,
      // The typed text stays what the tab shows.
      statementSql: "SELECT * FROM t WHERE a = {{a}}",
      pageSource: { sql: "SELECT * FROM t WHERE a = $1", params: [5] },
      isStreaming: false,
    });
  });

  it("setPageSize to 0 re-streams through db.page", async () => {
    const { manager, runner, results } = await paged();
    const paging = manager.setPageSize("tab-1", 0, 0);
    await settle();
    expect(runner.pages[0].params).toMatchObject({ page: 1, pageSize: 0 });
    push(
      runner.pages[0].queue,
      start(0, "SELECT * FROM t WHERE a = $1", "stream", { pageSize: 0 }),
    );
    await settle();
    expect(results()[0]).toMatchObject({ rows: [], isStreaming: true, pageSize: 0 });
    push(
      runner.pages[0].queue,
      batch(["a"], [[1], [2]], false),
      batch(null, [[3]]),
      stmtDone(0, { totalRows: 3 }),
      { type: "done", statements: 1, succeeded: true },
    );
    await paging;
    expect(results()[0]).toMatchObject({ rows: [[1], [2], [3]], totalRows: 3, isStreaming: false });
  });

  it("a refused page marks the result", async () => {
    const { manager, runner, results } = await paged();
    const paging = manager.goToPage("tab-1", 2, 0);
    await settle();
    push(runner.pages[0].queue, { type: "error", code: "INVALID_ARGUMENT", message: "no" });
    await paging;
    expect(results()[0]).toMatchObject({ isError: true, error: "INVALID_ARGUMENT: no" });
  });

  it("doesn't page a result that isn't a SELECT", async () => {
    const { manager, runner } = setup("SHOW x");
    const running = manager.execute("tab-1", { pageSize: 0 });
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SHOW x", "utility"),
      batch(["x"], [[1]]),
      stmtDone(0),
      done(1),
    );
    await running;
    await manager.setPageSize("tab-1", 100, 0);
    expect(runner.pages).toHaveLength(0);
  });
});

describe("lifecycle", () => {
  it("Stop cancels the run once and marks the streaming result stopped", async () => {
    // Through Core's runner and a client that records its cancels.
    const cancels: string[] = [];
    const requests: StreamRequest[] = [];
    let queue: Queue | null = null;
    const client = {
      call: vi.fn(),
      events: () => () => {},
      stream(request: StreamRequest, options: { signal?: AbortSignal } = {}) {
        requests.push(request);
        const q = new StreamQueue<RunEvent>(() => cancels.push(request.params.params.streamId));
        options.signal?.addEventListener("abort", () => {
          cancels.push(request.params.params.streamId);
          q.pushError(cancelledEvent());
        });
        queue = q;
        return q;
      },
    } as unknown as CoreClient;
    const runner = new CoreQueryRunner(() => client);
    const { manager, results, tab } = setup("SELECT pg_sleep(30); SELECT 42", { runner });
    const running = manager.execute("tab-1");
    await settle();
    expect(requests[0].params.method).toBe("run");
    push(queue!, start(0, "SELECT pg_sleep(30)", "page"));
    await settle();
    expect(results()[0].isStreaming).toBe(true);

    manager.cancelStream("tab-1");
    manager.cancelStream("tab-1");
    expect(results()[0].isStreaming).toBe(false);
    await running;
    expect(cancels).toEqual([(requests[0].params.params as RunParams).streamId]);
    // Nothing more lands after the cancel.
    push(queue!, start(1, "SELECT 42", "page"));
    await settle();
    expect(results()).toHaveLength(1);
    expect(tab()?.isExecuting).toBe(false);
  });

  it("a new run on the tab cancels the previous run", async () => {
    const { manager, runner, results } = setup("SELECT 1 AS a; SELECT 2 AS b");
    const first = manager.execute("tab-1");
    await settle();
    push(runner.runs[0].queue, start(0, "SELECT 1 AS a", "page"));
    await settle();
    const second = manager.executeCurrent("tab-1", 20);
    await settle();
    expect(runner.runs[0].signal.aborted).toBe(true);
    expect(runner.runs[1].signal.aborted).toBe(false);
    // The old run's spinner stops even before the new one shows anything.
    expect(results()[0].isStreaming).toBe(false);
    await first;
    push(
      runner.runs[1].queue,
      start(0, "SELECT 2 AS b", "page"),
      batch(["b"], [[2]]),
      stmtDone(0),
      done(1),
    );
    await second;
    expect(results().map((r) => r.statementSql)).toEqual(["SELECT 2 AS b"]);
    expect(results()[0].rows).toEqual([[2]]);
  });

  it("a run that finishes after its tab closed changes nothing", async () => {
    const { manager, runner, state, pending, history } = setup("INSERT INTO t VALUES (1)", {
      deferWrites: true,
    });
    const running = manager.execute("tab-1");
    await settle();
    // The tab closes without telling the manager (the listener path is below).
    state.queryTabsByProject = { p: [] };
    push(
      runner.runs[0].queue,
      {
        type: "statementDeferred",
        index: 0,
        sql: "INSERT INTO t VALUES (1)",
        source: { sql: "INSERT INTO t VALUES (1)", params: [] },
        queryType: "insert",
      },
      done(1, { history: historyRow }),
    );
    await running;
    // The run was cancelled in Core, and nothing reached pending changes.
    expect(runner.runs[0].signal.aborted).toBe(true);
    expect(pending.addSql).not.toHaveBeenCalled();
    expect(toast.info).not.toHaveBeenCalled();
    // A `done` that was already on its way still caches what Core stored.
    expect(history.insertRecorded).toHaveBeenCalledWith(historyRow);
  });

  it("closing the tab cancels its run", async () => {
    const { manager, runner } = setup();
    const running = manager.execute("tab-1");
    await settle();
    manager.forgetTab("tab-1");
    expect(runner.runs[0].signal.aborted).toBe(true);
    await running;
  });

  it("results land on the tab the run started on after a tab switch", async () => {
    const { manager, runner, state, results } = setup();
    state.queryTabsByProject.p.push({ id: "tab-2", query: "SELECT 2" });
    const running = manager.execute("tab-1");
    await settle();
    // Another project becomes active while tab-1's run goes on.
    state.activeProjectId = "q";
    push(
      runner.runs[0].queue,
      start(0, "SELECT 1 AS a", "page"),
      batch(["a"], [[1]]),
      stmtDone(0),
      done(1),
    );
    await running;
    expect(results()[0].rows).toEqual([[1]]);
    expect(state.queryTabsByProject.p.find((t) => t.id === "tab-2")?.results).toBeUndefined();
  });
});

describe("CONFIRM_REQUIRED", () => {
  const destructive = [{ index: 0, sql: "DELETE FROM t", reason: "delete_no_where" as const }];

  it("keeps Core's total when it lists only the first statements", async () => {
    const { manager, runner } = setup("DELETE FROM t");
    const running = manager.execute("tab-1");
    await settle();
    push(runner.runs[0].queue, {
      type: "error",
      code: "CONFIRM_REQUIRED",
      message: "250 destructive statements must be confirmed",
      destructive,
      destructiveTotal: 250,
    });
    await running;
    expect(manager.pendingConfirm).toMatchObject({ statements: destructive, total: 250 });
  });

  it("opens the destructive dialog, and Confirm resends with confirmed", async () => {
    const { manager, runner, state } = setup("DELETE FROM t");
    const running = manager.execute("tab-1");
    await settle();
    push(runner.runs[0].queue, {
      type: "error",
      code: "CONFIRM_REQUIRED",
      message: "1 destructive statement must be confirmed",
      destructive,
    });
    await running;
    expect(manager.pendingConfirm).toEqual({
      tabId: "tab-1",
      statements: destructive,
      total: 1,
      connectionId: "conn-1",
      providerConnectionId: "pc-1",
    });

    // What was listed is what runs, even if the text changed meanwhile.
    state.queryTabsByProject.p[0].query = "DROP TABLE t";
    const confirmed = manager.confirmPending("tab-1");
    await settle();
    expect(manager.pendingConfirm).toBeNull();
    expect(runner.runs[1].params).toMatchObject({ text: "DELETE FROM t", confirmed: true });
    push(runner.runs[1].queue, done(1));
    await confirmed;
  });

  it("is cleared on a tab switch and a close", async () => {
    const { manager, runner } = setup("DELETE FROM t");
    for (let i = 0; i < 2; i++) {
      const running = manager.execute("tab-1");
      await settle();
      push(runner.runs[i].queue, {
        type: "error",
        code: "CONFIRM_REQUIRED",
        message: "confirm",
        destructive,
      });
      await running;
      expect(manager.pendingConfirm).not.toBeNull();
      if (i === 0) manager.activeTabChanged("tab-2");
      else manager.forgetTab("tab-1");
      expect(manager.pendingConfirm).toBeNull();
    }
    // Switching to the same tab keeps it.
    const running = manager.execute("tab-1");
    await settle();
    push(runner.runs[2].queue, {
      type: "error",
      code: "CONFIRM_REQUIRED",
      message: "confirm",
      destructive,
    });
    await running;
    manager.activeTabChanged("tab-1");
    expect(manager.pendingConfirm).not.toBeNull();
  });

  it("a file-drop run of a destructive statement asks first", async () => {
    // `db.queries.execute(tabId)`, as the file drop and the grid reruns call
    // it: no local prompt, so Core's refusal is what asks.
    const { manager, runner, results } = setup("DELETE FROM t");
    const running = manager.execute("tab-1");
    await settle();
    expect(runner.runs[0].params).not.toHaveProperty("confirmed");
    push(runner.runs[0].queue, {
      type: "error",
      code: "CONFIRM_REQUIRED",
      message: "confirm",
      destructive,
    });
    await running;
    expect(manager.pendingConfirm?.statements).toEqual(destructive);
    expect(results()).toEqual([]);
  });
});

describe("pending changes", () => {
  it("deferred statements go to pending changes with their binds", async () => {
    const { manager, runner, pending, results } = setup(
      "INSERT INTO t VALUES ({{a}}); SELECT 1 AS a",
      {
        deferWrites: true,
      },
    );
    const running = manager.execute("tab-1", { params: [{ name: "a", value: 5n }] });
    await settle();
    expect(runner.runs[0].params.deferWrites).toBe(true);
    push(
      runner.runs[0].queue,
      {
        type: "statementDeferred",
        index: 0,
        sql: "INSERT INTO t VALUES ({{a}})",
        source: { sql: "INSERT INTO t VALUES ($1)", params: [{ $sq: "bigint", v: "5" }] },
        queryType: "insert",
      },
      start(1, "SELECT 1 AS a", "page"),
      batch(["a"], [[1]]),
      stmtDone(1),
      done(2),
    );
    await running;
    // The typed SQL as Core sent it, binds in the wire format (phase 5c, Decision 2).
    expect(pending.addSql).toHaveBeenCalledExactlyOnceWith(
      "conn-1",
      "INSERT INTO t VALUES ($1)",
      [{ $sq: "bigint", v: "5" }],
      "insert",
      "query-editor",
      "tab-1",
    );
    expect(toast.info).toHaveBeenCalledWith("1 statement added to pending changes");
    expect(pending.openSheet).toHaveBeenCalledOnce();
    expect(results().map((r) => r.statementSql)).toEqual(["SELECT 1 AS a"]);
  });
});

describe("review fixes", () => {
  it("a page request while a run is going doesn't cancel it", async () => {
    const { manager, runner, results } = setup(
      "SELECT 1 AS a; SELECT pg_sleep(30); INSERT INTO t VALUES (1)",
    );
    const running = manager.execute("tab-1");
    await settle();
    const { queue, signal } = runner.runs[0];
    push(
      queue,
      start(0, "SELECT 1 AS a", "page", { pageSize: 1 }),
      batch(["a"], [[1]]),
      stmtDone(0, { totalRows: 2, totalPages: 2 }),
      start(1, "SELECT pg_sleep(30)", "page"),
    );
    await settle();
    await manager.setPageSize("tab-1", 500, 0);
    await manager.goToPage("tab-1", 2, 0);
    expect(runner.pages).toHaveLength(0);
    expect(signal.aborted).toBe(false);
    push(
      queue,
      batch(["pg_sleep"], [[null]]),
      stmtDone(1),
      start(2, "INSERT INTO t VALUES (1)", "write"),
      stmtDone(2, { rowsAffected: 1 }),
      done(3),
    );
    await running;
    expect(results()).toHaveLength(3);
    expect(results()[2].affectedRows).toBe(1);
  });

  it("errors show the database's message alone, other codes with their code", () => {
    expect(errorText("QUERY_ERROR", 'relation "t" does not exist')).toBe(
      'relation "t" does not exist',
    );
    expect(errorText("CONNECTION_CLOSED", "closed")).toBe("CONNECTION_CLOSED: closed");
    expect(errorText("SQL_CHECK_FAILED", "unreachable")).toMatch(
      /^Couldn't find the statement.*unreachable/,
    );
  });

  it("a run-level error on a statement uses the same text", async () => {
    const { manager, runner, results } = setup("SELECT 1 AS a");
    const running = manager.execute("tab-1");
    await settle();
    push(runner.runs[0].queue, start(0, "SELECT 1 AS a", "page"), {
      type: "error",
      code: "QUERY_ERROR",
      message: "server closed the connection",
    });
    await running;
    expect(results()[0].error).toBe("server closed the connection");
  });

  it("a stopped statement that never finished shows as cancelled, a stream keeps its rows", async () => {
    const { manager, runner, results } = setup("SELECT g FROM t LIMIT 5; SELECT pg_sleep(30)");
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT g FROM t LIMIT 5", "stream"),
      batch(["g"], [[1]], false),
    );
    await settle();
    manager.cancelStream("tab-1");
    await running;
    expect(results()[0]).toMatchObject({ rows: [[1]], isError: false, isStreaming: false });

    const again = manager.execute("tab-1");
    await settle();
    push(runner.runs[1].queue, start(0, "SELECT pg_sleep(30)", "page"));
    await settle();
    manager.cancelStream("tab-1");
    await again;
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({ isError: true, error: "Cancelled before it finished." });
  });

  it("a cancelled page keeps the page, size and rows it showed", async () => {
    const { manager, runner, results } = setup();
    const running = manager.execute("tab-1", { pageSize: 2 });
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT 1 AS a", "page", { pageSize: 2 }),
      batch(["a"], [[1], [2]]),
      stmtDone(0, { totalRows: 5, totalPages: 3 }),
      done(1),
    );
    await running;
    const paging = manager.goToPage("tab-1", 2, 0);
    await settle();
    push(runner.pages[0].queue, start(0, "SELECT 1 AS a", "page", { page: 2, pageSize: 2 }));
    await settle();
    expect(results()[0].page).toBe(1);
    manager.cancelStream("tab-1");
    await paging;
    expect(results()[0]).toMatchObject({
      page: 1,
      pageSize: 2,
      rows: [[1], [2]],
      isError: false,
      isStreaming: false,
    });
  });

  it("a run replaced by one refused before showing anything leaves nothing half-done", async () => {
    const { manager, runner, results } = setup("SET x = 1; SELECT pg_sleep(30)");
    const first = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SET x = 1", "utility"),
      stmtDone(0, { totalRows: 0 }),
      start(1, "SELECT pg_sleep(30)", "page"),
    );
    await settle();
    const second = manager.execute("tab-1");
    await settle();
    push(runner.runs[1].queue, {
      type: "error",
      code: "CONFIRM_REQUIRED",
      message: "confirm",
      destructive: [{ index: 0, sql: "DELETE FROM t", reason: "delete_no_where" }],
    });
    await Promise.all([first, second]);
    // The hidden SET is filtered out, and the unfinished statement says so.
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({
      statementSql: "SELECT pg_sleep(30)",
      isError: true,
      isStreaming: false,
    });
  });

  it("dropping a project's tabs cancels their runs", async () => {
    const { manager, runner, state } = setup();
    const running = manager.execute("tab-1");
    await settle();
    // Tabs gone without `queryTabs.remove` (a project's tabs replaced).
    state.queryTabsByProject = { p: [] };
    manager.forgetOrphans();
    expect(runner.runs[0].signal.aborted).toBe(true);
    await running;

    const env = setup();
    const again = env.manager.execute("tab-1");
    await settle();
    env.manager.forgetProject("p");
    expect(env.runner.runs[0].signal.aborted).toBe(true);
    await again;
  });

  describe("Confirm can't run on another connection", () => {
    const destructive = [{ index: 0, sql: "DELETE FROM t", reason: "delete_no_where" as const }];
    async function refused() {
      const env = setup("DELETE FROM t");
      const running = env.manager.execute("tab-1");
      await settle();
      push(env.runner.runs[0].queue, {
        type: "error",
        code: "CONFIRM_REQUIRED",
        message: "confirm",
        destructive,
      });
      await running;
      expect(env.manager.pendingConfirm).not.toBeNull();
      return env;
    }

    it("refuses with a toast when the refused run's connection reconnected or went", async () => {
      for (const change of ["reconnected", "removed", "disconnected"] as const) {
        vi.mocked(errorToast).mockClear();
        const { manager, runner, state } = await refused();
        const conn = state.connections[0];
        if (change === "reconnected")
          state.connections[0] = { ...conn, providerConnectionId: "pc-9" };
        if (change === "disconnected") state.connections[0] = { ...conn, providerConnectionId: "" };
        if (change === "removed") state.connections.splice(0, 1);
        await manager.confirmPending("tab-1");
        expect(runner.runs).toHaveLength(1);
        expect(manager.pendingConfirm).toBeNull();
        expect(errorToast).toHaveBeenCalledWith(
          "The connection changed since you were asked, so nothing was run. Run the query again.",
        );
      }
    });

    it("confirms on the refused run's connection, not whichever is active now", async () => {
      const { manager, runner, state } = await refused();
      const other = { id: "conn-2", type: "duckdb", name: "Quick", providerConnectionId: "pc-2" };
      state.connections.push(other);
      state.activeConnection = other;
      state.activeConnectionId = "conn-2";
      const confirmed = manager.confirmPending("tab-1");
      await settle();
      expect(runner.runs).toHaveLength(2);
      expect(runner.runs[1].params).toMatchObject({ connectionId: "pc-1", confirmed: true });
      push(runner.runs[1].queue, done(1));
      await confirmed;
    });

    it("a file drop for another connection clears it, so going back and confirming runs nothing", async () => {
      const { manager, runner, state } = await refused();
      // The file drop: another connection made active, a new tab added (the
      // tab manager reports it as a switch), and its own run.
      state.activeConnection = {
        id: "conn-2",
        type: "duckdb",
        name: "Quick",
        providerConnectionId: "pc-2",
      };
      manager.activeTabChanged("tab-2");
      expect(manager.pendingConfirm).toBeNull();
      // Back on tab-1, the dialog is gone; a stale Confirm does nothing.
      manager.activeTabChanged("tab-1");
      await manager.confirmPending("tab-1");
      expect(runner.runs).toHaveLength(1);
    });

    it("a project switch clears it", async () => {
      const { manager } = await refused();
      manager.activeTabChanged(null);
      expect(manager.pendingConfirm).toBeNull();
    });
  });
});

describe("approval fixes", () => {
  it("returning to a project mid-run cancels the run once and leaves no tab executing", async () => {
    const { manager, runner, state, tab } = setup("SELECT pg_sleep(30)");
    const running = manager.execute("tab-1");
    await settle();
    let aborts = 0;
    runner.runs[0].signal.addEventListener("abort", () => (aborts += 1));
    push(runner.runs[0].queue, start(0, "SELECT pg_sleep(30)", "page"));
    await settle();
    // Switch to B, then back to A: `loadProjectState` reloads A's tabs,
    // telling the view model first.
    state.activeProjectId = "q";
    state.activeProjectId = "p";
    manager.cancelProject("p");
    state.queryTabsByProject = {
      p: [{ id: "tab-1", query: "SELECT pg_sleep(30)", isExecuting: false }],
    };
    await running;
    expect(aborts).toBe(1);
    expect(runner.runs[0].signal.aborted).toBe(true);
    expect(tab()?.isExecuting).toBe(false);
    // Nothing is left registered for the project (the page guard is released).
    manager.cancelProject("p");
    expect(aborts).toBe(1);
  });

  it("a runner that fails to start is a toast and nothing executes", async () => {
    const { state } = setup();
    const history = { contextFor: () => ({}), insertRecorded: vi.fn() };
    const manager = new QueryExecutionManager(
      state as unknown as DatabaseState,
      history as unknown as QueryHistoryManager,
      {} as ProviderRegistry,
      { isEnabled: () => false } as unknown as PendingChangesManager,
      async () => {
        throw new Error("DuckDB didn't start");
      },
    );
    await manager.execute("tab-1");
    expect(errorToast).toHaveBeenCalledWith("DuckDB didn't start");
    expect(state.queryTabsByProject.p[0].isExecuting).toBeFalsy();
    await manager.goToPage("tab-1", 2, 0);
  });
});

describe("edits target the result's connection (phase 5c, Task 1)", () => {
  const other = { id: "conn-2", type: "postgres", name: "Other", providerConnectionId: "pc-2" };

  /** Run `SELECT id, total FROM orders` on conn-1, then make conn-2 active. */
  async function ranThenSwitched() {
    const env = setup("SELECT id, total FROM orders");
    const running = env.manager.execute("tab-1");
    await settle();
    push(
      env.runner.runs[0].queue,
      start(0, "SELECT id, total FROM orders", "page", {
        table: { table: "orders" },
        columnRefs: [
          { table: "orders", column: "id" },
          { table: "orders", column: "total" },
        ],
      }),
      batch(["id", "total"], [[1, 2]]),
      stmtDone(0),
      done(1),
    );
    await running;
    env.state.connections.push(other);
    env.state.activeConnection = other;
    env.state.activeConnectionId = "conn-2";
    return env;
  }

  it("every result a run makes records its connection", async () => {
    const { manager, runner, results } = setup("SELECT 1;\nSELECT {{x}}");
    const running = manager.execute("tab-1");
    await settle();
    push(
      runner.runs[0].queue,
      start(0, "SELECT 1", "page"),
      batch(["a"], [[1]]),
      stmtDone(0),
      {
        type: "statementError",
        index: 1,
        sql: "SELECT {{x}}",
        code: "INVALID_PARAMETERS",
        message: "x",
        elapsedMs: 0,
      },
      done(2),
    );
    await running;
    expect(results().map((r) => r.connectionId)).toEqual(["conn-1", "conn-1"]);

    const failing = setup();
    const failed = failing.manager.execute("tab-1");
    await settle();
    push(failing.runner.runs[0].queue, { type: "error", code: "UNKNOWN", message: "no" });
    await failed;
    expect(failing.results()[0]).toMatchObject({ isError: true, connectionId: "conn-1" });
  });

  it("an edit on an old query result goes to the connection it came from", async () => {
    const { manager, results } = await ranThenSwitched();
    expect(results()[0].connectionId).toBe("conn-1");
    const update = vi.spyOn(manager.crud, "updateCellDirect").mockResolvedValue({ success: true });

    const source = { schema: "public", name: "orders", primaryKeys: ["id"] };
    expect(await manager.updateCell("tab-1", 0, 0, "total", 3, source)).toEqual({ success: true });
    expect(update).toHaveBeenCalledWith("conn-1", source, { id: 1 }, "total", 3);
    expect(results()[0].rows).toEqual([[1, 3]]);
  });

  it("Set default goes to the result's connection and reruns there", async () => {
    const { manager, runner } = await ranThenSwitched();
    const setDefault = vi
      .spyOn(manager.crud, "setCellDefaultDirect")
      .mockResolvedValue({ success: true });

    const source = { schema: "public", name: "orders", primaryKeys: ["id"] };
    const pending = manager.setCellDefault("tab-1", 0, 0, "total", source);
    await settle();
    await settle();
    expect(setDefault).toHaveBeenCalledWith("conn-1", source, { id: 1 }, "total");
    expect(runner.runs).toHaveLength(2);
    expect(runner.runs[1].params).toMatchObject({
      connectionId: "pc-1",
      history: { connectionId: "conn-1" },
    });
    push(runner.runs[1].queue, done(0));
    await pending;
  });

  it("paging an old result pages on its connection", async () => {
    const { manager, runner } = await ranThenSwitched();
    void manager.goToPage("tab-1", 1, 0);
    await settle();
    expect(runner.pages[0].params.connectionId).toBe("pc-1");
    manager.cancelStream("tab-1");
  });

  it("an edit on a result whose connection is gone is refused, and nothing runs", async () => {
    const { manager, state } = await ranThenSwitched();
    state.connections.splice(0, 1);
    const update = vi.spyOn(manager.crud, "updateCellDirect");
    const source = { schema: "public", name: "orders", primaryKeys: ["id"] };
    // The CRUD manager refuses a connection it can't find: nothing is built or run.
    const result = await manager.updateCell("tab-1", 0, 0, "total", 3, source);
    expect(result).toEqual({ success: false, error: "No connection established" });
    expect(update).toHaveBeenCalledWith("conn-1", source, { id: 1 }, "total", 3);
  });
});

describe("query tab row delete (phase 5c, Task 1 follow-up)", () => {
  it("deletes by the key's column source, so an aliased key column binds", async () => {
    const env = setup("SELECT id AS order_id, total FROM orders");
    const running = env.manager.execute("tab-1");
    await settle();
    push(
      env.runner.runs[0].queue,
      start(0, "SELECT id AS order_id, total FROM orders", "page", {
        table: { table: "orders" },
        columnRefs: [
          { table: "orders", column: "id" },
          { table: "orders", column: "total" },
        ],
      }),
      batch(["order_id", "total"], [[7, 2]]),
      stmtDone(0),
      done(1),
    );
    await running;
    env.state.connections.push({
      id: "conn-2",
      type: "postgres",
      name: "Other",
      providerConnectionId: "pc-2",
    });
    env.state.activeConnectionId = "conn-2";
    const del = vi.spyOn(env.manager.crud, "deleteRow").mockResolvedValue({ success: true });

    const source = env.results()[0].sourceTable!;
    expect(await env.manager.deleteRowAt("tab-1", 0, 0, source)).toEqual({ success: true });
    expect(del).toHaveBeenCalledWith(
      "conn-1",
      { schema: "public", name: "orders", primaryKeys: ["id"] },
      { id: 7 },
    );
  });

  it("refuses a row that isn't there and a result without a connection", async () => {
    const env = setup("SELECT 1");
    const del = vi.spyOn(env.manager.crud, "deleteRow");
    const source = { schema: "public", name: "orders", primaryKeys: ["id"] };
    expect(await env.manager.deleteRowAt("tab-1", 0, 0, source)).toEqual({
      success: false,
      error: "Row not found",
    });
    env.tab()!.results = [
      { columns: ["id"], rows: [[1]], isError: false } as unknown as StatementResult,
    ];
    expect(await env.manager.deleteRowAt("tab-1", 0, 0, source)).toEqual({
      success: false,
      error: "No connection established",
    });
    expect(del).not.toHaveBeenCalled();
  });
});
