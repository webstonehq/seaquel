/**
 * The demo's DuckDB paths through Core in the page (phase 8, Task 6): the
 * browser module's test build over DuckDB-WASM's Node build, reached as the
 * demo reaches it (`openBrowserCore`, its `CoreClient`, `CoreProvider`).
 * These replace the TypeScript twins' live suites (Parity):
 *
 * - the editor's runs (the demo runner's DuckDB suite): `db.run` and `db.page`
 *   through the view model and `CoreQueryRunner`, DuckDB's own messages on
 *   a page, a write and a stream, an inlined parameter, a hidden `SET`, and
 *   history Core records;
 * - the AI's and dashboards' read-only path (the demo provider's read-only suite):
 *   `db.queryStream` with `readOnly` (Core's token check, then the browser
 *   driver's read-only transaction), every attack checked from a plain
 *   connection afterwards.
 *
 * The engine suite (`src/lib/engine/engine-duckdb-browser.test.ts`) covers
 * the driver itself and the DuckDB edit fixtures.
 */
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { AsyncDuckDB, AsyncDuckDBConnection } from "@duckdb/duckdb-wasm";
import type { PendingChange, StatementResult } from "$lib/types";
import type { PersistedQueryHistoryItem } from "$lib/types/generated/PersistedQueryHistoryItem";
import {
  bootDuckDb,
  loadTestModule,
  testModuleMissing,
  type TestModule,
} from "$lib/core/browser/testing/node";
import type { SnapshotStore } from "$lib/core/browser/snapshot-store";
import type { OpenedBrowserCore } from "$lib/core/browser";
import type { DatabaseState } from "$lib/hooks/database/state.svelte.js";
import type { QueryHistoryManager } from "$lib/hooks/database/query-history.svelte.js";
import type { ProviderRegistry } from "$lib/providers";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
vi.mock("$lib/stores/license-nudge.svelte.js", () => ({
  licenseNudgeStore: { recordQuery: vi.fn() },
}));

const { openBrowserCore, makeDuckDbBridge } = await import("$lib/core/browser");
const { setCoreClient } = await import("$lib/core");
const { CoreProvider } = await import("$lib/providers/core-provider");
const { QueryExecutionManager } = await import("$lib/hooks/database/query-execution.svelte.js");
const { PendingChangesManager } = await import("$lib/hooks/database/pending-changes.svelte.js");
const { CoreEditService } = await import("$lib/hooks/database/edit-service/core-service.js");
const { fromPersisted } = await import("$lib/hooks/database/query-history.svelte.js");

const missing = testModuleMissing();

class MemoryStore implements SnapshotStore {
  async load() {
    return null;
  }
  async save() {}
  async moveAside() {}
}

let module: TestModule | null = null;
let duck: AsyncDuckDB;
let opened: OpenedBrowserCore;
let provider: InstanceType<typeof CoreProvider>;
/** The demo connection's Core id (a saved target, so history can name it). */
let demoId: string;
/** A plain DuckDB-WASM connection on the same database, to check what an attack left. */
let plain: AsyncDuckDBConnection;
/** The Node build writes to the real filesystem; attacks aim in here. */
let scratch: string;

beforeAll(async () => {
  module = await loadTestModule();
  if (!module) return;
  scratch = mkdtempSync(path.join(tmpdir(), "seaquel-demo-core-"));
  duck = await bootDuckDb();
  opened = await openBrowserCore({
    module,
    bridge: makeDuckDbBridge(duck),
    store: new MemoryStore(),
    localStorage: null,
    window: null,
    document: null,
  });
  setCoreClient(opened.client);
  provider = new CoreProvider(() => opened.client);
  await opened.core.ensureDemoConnection();
  demoId = await provider.connect({ target: { type: "saved", id: "demo-connection" } });
  plain = await duck.connect();
}, 60_000);

afterAll(async () => {
  setCoreClient(null);
  opened?.close();
  await plain?.close();
  if (scratch) rmSync(scratch, { recursive: true, force: true });
});

/** One value from the plain connection. */
async function scalar(sql: string): Promise<unknown> {
  const row = (await plain.query(sql)).toArray()[0]?.toJSON() as Record<string, unknown>;
  return Object.values(row)[0];
}

async function rowsInT(): Promise<number> {
  return Number(await scalar("SELECT count(*) FROM t"));
}

async function tableExists(name: string): Promise<boolean> {
  return (
    Number(await scalar(`SELECT count(*) FROM duckdb_tables() WHERE table_name = '${name}'`)) > 0
  );
}

/** The editor's view model over Core, on the demo connection, as the demo wires it. */
function editor(query: string) {
  const connection = {
    id: "demo-connection",
    type: "duckdb",
    name: "Demo Database",
    providerConnectionId: demoId,
  };
  const tab = { id: "tab-1", query } as { id: string; query: string; results?: StatementResult[] };
  const state = {
    activeProjectId: "p",
    queryTabsByProject: { p: [tab] },
    activeConnection: connection,
    activeConnectionId: "demo-connection",
    connections: [connection],
    schemas: {},
  } as unknown as DatabaseState;
  const history = {
    contextFor: (id: string) => ({
      connectionId: id,
      connectionName: "Demo Database",
      connectionLabels: [],
    }),
    insertRecorded: vi.fn(),
  };
  const manager = new QueryExecutionManager(
    state,
    history as unknown as QueryHistoryManager,
    { getForType: async () => provider } as unknown as ProviderRegistry,
    { isEnabled: () => false } as unknown as InstanceType<typeof PendingChangesManager>,
  );
  const results = () => state.queryTabsByProject.p[0].results ?? [];
  return { manager, results, history };
}

describe.skipIf(missing)("the demo's editor runs in Core", () => {
  beforeEach(async () => {
    await plain.query("DROP TABLE IF EXISTS t; CREATE TABLE t (a INTEGER)");
  });

  it("runs a page, a write and a failing statement, showing DuckDB's own message", async () => {
    const { manager, results, history } = editor(
      "SELECT 1 AS a;\nINSERT INTO t VALUES (1);\nSELECT * FROM nope",
    );
    await manager.execute("tab-1");
    expect(results()[0]).toMatchObject({ columns: ["a"], rows: [[1]], totalRows: 1 });
    expect(results()[1]).toMatchObject({ affectedRows: 1, rows: [["1 row(s) affected"]] });
    expect(results()[2]).toMatchObject({ isError: true });
    expect(results()[2].error).toContain("Table with name nope does not exist!");
    // The write ran; a failed statement records no history.
    expect(await rowsInT()).toBe(1);
    expect(history.insertRecorded).not.toHaveBeenCalled();
  });

  it("a failing stream shows DuckDB's message", async () => {
    const { manager, results } = editor("SELECT * FROM nope LIMIT 5");
    await manager.execute("tab-1");
    expect(results()[0]).toMatchObject({ kind: "stream", isError: true });
    expect(results()[0].error).toContain("Table with name nope does not exist!");
  });

  it("pages with an inlined parameter, hides SET, and records history", async () => {
    const { manager, results, history } = editor(
      "SET default_order = 'asc';\nSELECT g FROM range(10) t(g) WHERE g > {{min}} ORDER BY g",
    );
    await manager.execute("tab-1", { pageSize: 2, params: [{ name: "min", value: 2 }] });
    expect(results()).toHaveLength(1);
    expect(results()[0]).toMatchObject({ rows: [[3], [4]], totalRows: 7, totalPages: 4 });
    // Core stored the run and handed back the row it stored.
    expect(history.insertRecorded).toHaveBeenCalledOnce();
    const recorded = history.insertRecorded.mock.calls[0][0] as { query: string };
    expect(recorded.query).toContain("{{min}}");

    await manager.goToPage("tab-1", 2, 0);
    expect(results()[0]).toMatchObject({ page: 2, rows: [[5], [6]] });
    // A page records nothing.
    expect(history.insertRecorded).toHaveBeenCalledOnce();
  });
});

describe.skipIf(missing)("the demo's applied edits keep their values in history", () => {
  beforeEach(async () => {
    await plain.query("DROP TABLE IF EXISTS t; CREATE TABLE t (a INTEGER)");
  });

  /** The pending-changes queue over the demo's Core, as the demo wires it. */
  function queue() {
    const state = {
      connections: [
        {
          id: "demo-connection",
          type: "duckdb",
          name: "Demo Database",
          providerConnectionId: demoId,
        },
      ],
      pendingChangesByConnection: {} as Record<string, PendingChange[]>,
      pendingChangesInterrupted: {} as Record<string, boolean>,
      isPendingChangesOpen: false,
    };
    const history = {
      contextFor: (id: string) => ({
        connectionId: id,
        connectionName: "Demo Database",
        connectionLabels: [],
      }),
      insertRecorded: vi.fn(),
    };
    const service = new CoreEditService(() => opened.client);
    const manager = new PendingChangesManager(
      state as unknown as DatabaseState,
      {} as ProviderRegistry,
      history as unknown as QueryHistoryManager,
      async () => service,
    );
    return { manager, history };
  }

  async function storedHistory(): Promise<PersistedQueryHistoryItem[]> {
    const response = (await opened.client.call({
      method: "storage",
      params: {
        method: "queryHistoryLoadByConnection",
        params: { connectionId: "demo-connection" },
      },
    })) as { result: { result: PersistedQueryHistoryItem[] } };
    return response.result.result;
  }

  it("records the values, shows them, and runs the row again with them", async () => {
    const { manager, history } = queue();
    manager.addSql("demo-connection", "INSERT INTO t VALUES (?)", [7], "insert", "query-editor");
    expect(await manager.apply("demo-connection")).toMatchObject({ kind: "applied", applied: 1 });
    expect(await rowsInT()).toBe(1);
    const recorded = history.insertRecorded.mock.calls[0][0] as PersistedQueryHistoryItem;
    expect(recorded).toMatchObject({ query: "INSERT INTO t VALUES (?)", params: [7] });
    const stored = (await storedHistory()).find((h) => h.id === recorded.id);
    expect(stored?.params).toEqual([7]);

    // Run it again from history: queued with its values, applied as before.
    manager.addFromHistory(fromPersisted(stored!));
    expect(await manager.apply("demo-connection")).toMatchObject({ kind: "applied", applied: 1 });
    expect(Number(await scalar("SELECT count(*) FROM t WHERE a = 7"))).toBe(2);
    const again = history.insertRecorded.mock.calls[1][0] as PersistedQueryHistoryItem;
    expect(again.params).toEqual([7]);
  });
});

/** The demo's read-only query (the AI's `run_query`, dashboard widgets). */
const readOnly = (sql: string, signal?: AbortSignal, maxRows?: number) =>
  provider.selectReadOnly(demoId, sql, signal, maxRows);

describe.skipIf(missing)("the demo's read-only path runs in Core", () => {
  beforeEach(async () => {
    await plain.query(`
      DROP TABLE IF EXISTS t; DROP TABLE IF EXISTS x; DROP VIEW IF EXISTS x;
      DROP SEQUENCE IF EXISTS s;
      CREATE TABLE t (a INTEGER); INSERT INTO t VALUES (1);
      CREATE SEQUENCE s;
    `);
  });

  it("runs a SELECT and its read-only forms", async () => {
    expect((await readOnly("SELECT a FROM t")).rows).toEqual([{ a: 1 }]);
    expect((await readOnly("SELECT a FROM t;")).rows).toEqual([{ a: 1 }]);
    // DuckDB's `FROM t` shorthand: Core's token check (`seaquel_sql::read_only`)
    // refuses it, as on desktop; the twin let DuckDB's `query()` parse it.
    await expect(readOnly("FROM t")).rejects.toThrow(/^READ_ONLY: /);
    expect((await readOnly("WITH w AS (SELECT a + 1 AS b FROM t) SELECT b FROM w")).rows).toEqual([
      { b: 2 },
    ]);
  });

  it("returns [] for no rows", async () => {
    expect(await readOnly("SELECT a FROM t WHERE a > 5")).toEqual({ rows: [], truncated: false });
  });

  it("stops at maxRows and says so", async () => {
    const sql = "SELECT * FROM range(1000000) r(n)";
    expect(await readOnly(sql, undefined, 3)).toEqual({
      rows: [{ n: 0 }, { n: 1 }, { n: 2 }],
      truncated: true,
    });
    expect(await readOnly("SELECT a FROM t", undefined, 3)).toEqual({
      rows: [{ a: 1 }],
      truncated: false,
    });
  });

  it("names duplicate columns as query() does", async () => {
    expect((await readOnly("SELECT 1 AS a, 2 AS a")).rows).toEqual([{ a: 1, a_1: 2 }]);
  });

  // Core's token check refuses each before DuckDB sees it (the twin left the
  // CTE to DuckDB's binder, a QUERY_ERROR).
  const writes: [string, string][] = [
    ["INSERT", "INSERT INTO t VALUES (2)"],
    ["INSERT … RETURNING", "INSERT INTO t VALUES (2) RETURNING a"],
    ["UPDATE", "UPDATE t SET a = 2"],
    ["DELETE", "DELETE FROM t"],
    ["COMMIT, then a write", "COMMIT; INSERT INTO t VALUES (2); SELECT 1"],
    ["a second statement that writes", "SELECT 1; INSERT INTO t VALUES (2)"],
    ["a CTE that deletes", "WITH d AS (DELETE FROM t RETURNING a) SELECT * FROM d"],
  ];
  it.each(writes)("refuses %s and leaves t as it was", async (_name, sql) => {
    await expect(readOnly(sql)).rejects.toThrow(/^READ_ONLY: /);
    expect(await rowsInT()).toBe(1);
    expect(await scalar("SELECT a FROM t")).toBe(1);
  });

  it("refuses DDL and leaves no table", async () => {
    for (const sql of [
      "CREATE TABLE x AS SELECT 1 AS n",
      "CREATE TEMP TABLE x (n INTEGER)",
      "CREATE VIEW x AS SELECT 1",
    ]) {
      await expect(readOnly(sql), sql).rejects.toThrow(/^READ_ONLY: /);
    }
    expect(await tableExists("x")).toBe(false);
    expect(Number(await scalar("SELECT count(*) FROM duckdb_views() WHERE view_name = 'x'"))).toBe(
      0,
    );
    await expect(readOnly("DROP TABLE t")).rejects.toThrow(/^READ_ONLY: /);
    expect(await tableExists("t")).toBe(true);
  });

  it("refuses nextval and leaves the sequence where it was", async () => {
    await expect(readOnly("SELECT nextval('s')")).rejects.toThrow(/^READ_ONLY: /);
    expect(Number(await scalar("SELECT nextval('s')"))).toBe(1);
  });

  it("refuses COPY … TO and writes no file", async () => {
    const file = path.join(scratch, "copy.csv");
    await expect(readOnly(`COPY (SELECT 1) TO '${file}'`)).rejects.toThrow(/^READ_ONLY: /);
    expect(existsSync(file)).toBe(false);
  });

  it("refuses EXPORT DATABASE and writes nothing", async () => {
    const dir = path.join(scratch, "export");
    await expect(readOnly(`EXPORT DATABASE '${dir}'`)).rejects.toThrow(/^READ_ONLY: /);
    expect(existsSync(dir)).toBe(false);
  });

  it("refuses ATTACH and attaches nothing", async () => {
    const file = path.join(scratch, "attached.duckdb");
    await expect(readOnly(`ATTACH '${file}' AS m`)).rejects.toThrow(/^READ_ONLY: /);
    expect(existsSync(file)).toBe(false);
    expect(
      Number(await scalar("SELECT count(*) FROM duckdb_databases() WHERE database_name = 'm'")),
    ).toBe(0);
  });

  it("refuses SET and leaves the setting as it was", async () => {
    const before = await scalar("SELECT current_setting('default_order')");
    for (const sql of ["SET default_order = 'desc'", "SET GLOBAL default_order = 'desc'"]) {
      await expect(readOnly(sql), sql).rejects.toThrow(/^READ_ONLY: /);
    }
    expect(await scalar("SELECT current_setting('default_order')")).toBe(before);
    expect((await readOnly("SELECT current_setting('default_order') AS o")).rows).toEqual([
      { o: before },
    ]);
  });

  it("refuses INSTALL, LOAD, CHECKPOINT and PRAGMA", async () => {
    for (const sql of ["INSTALL httpfs", "LOAD parquet", "CHECKPOINT", "PRAGMA version"]) {
      await expect(readOnly(sql), sql).rejects.toThrow(/^READ_ONLY: /);
    }
  });

  it("reports other errors with the QUERY_ERROR code", async () => {
    await expect(readOnly("SELECT * FROM missing_table")).rejects.toThrow(
      /^QUERY_ERROR: .*Catalog Error/,
    );
  });

  it("leaves the user's own transaction open and unaffected", async () => {
    await provider.execute(demoId, "BEGIN TRANSACTION");
    await provider.execute(demoId, "INSERT INTO t VALUES (7)");
    // The read-only query runs on its own connection and doesn't see it.
    expect((await readOnly("SELECT count(*)::INTEGER AS n FROM t")).rows).toEqual([{ n: 1 }]);
    // Still in the user's transaction: their row is there, and COMMIT works.
    expect((await provider.select(demoId, "SELECT count(*)::INTEGER AS n FROM t"))[0]).toEqual({
      n: 2,
    });
    await provider.execute(demoId, "COMMIT");
    expect(await rowsInT()).toBe(2);
  });

  it("rejects with an AbortError for an aborted signal, before anything runs", async () => {
    const controller = new AbortController();
    controller.abort();
    await expect(readOnly("SELECT 1", controller.signal)).rejects.toMatchObject({
      name: "AbortError",
    });
  });

  it("stops the query in DuckDB when the signal aborts, and the next call works", async () => {
    // A cancel reaches DuckDB-WASM (the twin's only rejected the promise and
    // let the query finish in the worker).
    const controller = new AbortController();
    const slow = readOnly(
      "SELECT count(*) AS n FROM range(600000000) a, range(10) b WHERE (a.range + b.range) % 7 = 0",
      controller.signal,
    );
    const started = Date.now();
    setTimeout(() => controller.abort(), 100);
    await expect(slow).rejects.toMatchObject({ name: "AbortError" });
    expect((await readOnly("SELECT a FROM t")).rows).toEqual([{ a: 1 }]);
    expect(Date.now() - started).toBeLessThan(5_000);
  }, 60_000);
});
