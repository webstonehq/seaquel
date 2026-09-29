/**
 * The demo's `TsEditService` (phase 5c, Decision 16) through the demo's real
 * stack: the view models, `getEditService` (the demo's choice, so
 * `TsEditService` with the DuckDB adapter) and the real `DuckDBProvider`,
 * whose DuckDB-WASM connection answers from a script. It replays the DuckDB
 * cases of `crates/seaquel-workspace/tests/fixtures/edits`:
 *
 * - plan: the queue each case ends with (`change`, `origin`, `target`,
 *   `description`, ids and dedupe) and each step's outcome;
 * - apply: what the queue holds after the apply (`queueAfter`), and the
 *   statements the demo ran, in one transaction for a DML batch;
 * - table page: the default-catalog case's page and count SQL (today's,
 *   which the demo keeps) and the result.
 *
 * The attached-catalog table page isn't replayed: the demo lists its schemas
 * bare and quotes `cat.main` as one name, where the recording (desktop)
 * quoted two. The demo's SQL inlines values, so SQL text and binds are the
 * demo's own, pinned here only where they show the builders ran.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DataTab, PendingChange, SchemaTable, StatementResult } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "../state.svelte.js";
import type { TabOrderingManager } from "../tab-ordering.svelte.js";
import type { QueryHistoryManager } from "../query-history.svelte.js";
import type { Change, Edit } from "./types";
import { decodeCell, decodeRows, encodeParam } from "$lib/values";
import applyCases from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/apply.json";
import changesFile from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/changes.json";
import planDuckdb from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/plan-duckdb.json";
import pageDuckdb from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/table-page-duckdb.json";

const settings = vi.hoisted(() => ({ enabled: false }));
vi.mock("$lib/stores/pending-changes-settings.svelte.js", () => ({
  pendingChangesSettingsStore: settings,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
const append = vi.hoisted(() => vi.fn(async (..._args: unknown[]) => {}));
vi.mock("$lib/storage", () => ({ getStorage: () => ({ queryHistory: { append } }) }));

const { DataTabManager } = await import("../data-tabs.svelte.js");
const { QueryExecutionManager } = await import("../query-execution.svelte.js");
const { PendingChangesManager } = await import("../pending-changes.svelte.js");
const { setEditService } = await import("./index.js");
const { DuckDBProvider } = await import("$lib/providers/duckdb-provider");

type Json = Record<string, unknown>;
const changes = changesFile as unknown as Record<string, { expected: Json }>;
function withChanges<T extends { name: string }>(c: T): T {
  const expected = changes[c.name]?.expected;
  return expected ? ({ ...c, ...expected } as T) : c;
}

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

/** The real provider over a connection answering each `query(sql)` with `answer`. */
function demoProvider(answer: (sql: string) => Record<string, unknown>[] | Error) {
  const seen: string[] = [];
  const conn = {
    query: vi.fn(async (sql: string) => {
      seen.push(sql);
      const rows = answer(sql);
      if (rows instanceof Error) throw rows;
      return arrow(rows);
    }),
  };
  const provider = new DuckDBProvider();
  (provider as unknown as { connections: Map<string, unknown> }).connections.set("pc-1", conn);
  const providers = { getForType: async () => provider } as unknown as ProviderRegistry;
  return { providers, seen };
}

const connection = { id: "conn-1", type: "duckdb", name: "Demo", providerConnectionId: "pc-1" };

function table(name: string): SchemaTable {
  return {
    name,
    schema: "main",
    type: "table",
    columns: [
      { name: "id", type: "INTEGER", nullable: false, isPrimaryKey: true, isForeignKey: false },
      { name: "v", type: "VARCHAR", nullable: true, isPrimaryKey: false, isForeignKey: false },
    ],
    indexes: [],
  } as unknown as SchemaTable;
}
const history = {
  contextFor: (id: string) => ({ connectionId: id, connectionName: "Demo", connectionLabels: [] }),
  insertRecorded: vi.fn(),
} as unknown as QueryHistoryManager;

function wireTarget(target: PendingChange["target"]): Json | undefined {
  if (!target) return undefined;
  const map = (values?: Record<string, unknown>) =>
    values && Object.fromEntries(Object.entries(values).map(([k, v]) => [k, encodeParam(v)]));
  return JSON.parse(
    JSON.stringify({
      ...target,
      ...(target.primaryKeyValues ? { primaryKeyValues: map(target.primaryKeyValues) } : {}),
      ...("newValue" in target ? { newValue: encodeParam(target.newValue) ?? null } : {}),
      ...(target.insertValues ? { insertValues: map(target.insertValues) } : {}),
    }),
  );
}

beforeEach(() => {
  settings.enabled = false;
  setEditService(null);
  append.mockClear();
});
afterEach(() => {
  setEditService(null);
});

// -------- Plan --------

interface PlanStep {
  action: "updateCell" | "setDefault" | "deleteRow" | "insertRow";
  row?: number;
  column?: string;
  value?: unknown;
  values?: Json;
  edit: Edit | null;
  driver: Array<{ op: string; answer?: { rowsAffected?: number } }>;
  outcome: { success?: boolean; queued?: boolean; saved?: boolean; error?: string; code?: string };
}
interface PlanCase {
  name: string;
  input: {
    via: "dataTab" | "queryTab";
    pending: boolean;
    table?: { schema: string; table: string };
    result: {
      columns: string[];
      rows: unknown[][];
      sourceTable: { schema: string; name: string; primaryKeys: string[] };
      columnSources: StatementResult["columnSources"] | null;
    };
    schemaCache: SchemaTable[];
  };
  steps: PlanStep[];
  queue: Array<{
    id: string;
    change: Change;
    origin: string;
    target: Json;
    description: string;
  }>;
}

describe("the demo's TsEditService replays the DuckDB plan cases", () => {
  const cases = (planDuckdb as unknown as PlanCase[]).map(withChanges);

  it.each(cases.map((c) => [c.name, c] as const))("%s", async (_name, c) => {
    settings.enabled = c.input.pending;
    const rows = decodeRows(structuredClone(c.input.result.rows));
    const columns = c.input.result.columns;
    let rowsAffected = 1;
    const { providers, seen } = demoProvider((sql) => {
      if (/^(UPDATE|DELETE|INSERT)/.test(sql)) return [{ Count: BigInt(rowsAffected) }];
      if (sql.startsWith("SELECT COUNT")) return [{ count: rows.length }];
      return rows.map((r) => Object.fromEntries(columns.map((col, i) => [col, r[i]])));
    });
    const result: StatementResult = {
      columns,
      rows,
      rowCount: rows.length,
      totalRows: rows.length,
      executionTime: 0,
      page: 1,
      pageSize: 100,
      totalPages: 1,
      queryType: "select",
      statementIndex: 0,
      statementSql: "",
      connectionId: "conn-1",
      isError: false,
      sourceTable: c.input.result.sourceTable,
      ...(c.input.result.columnSources ? { columnSources: c.input.result.columnSources } : {}),
    };
    const dataTab: DataTab | null =
      c.input.via === "dataTab"
        ? {
            id: "data-1",
            connectionId: "conn-1",
            tableName: c.input.table!.table,
            schemaName: c.input.table!.schema,
            filters: [],
            filterLogic: "AND",
            sortColumns: [],
            page: 1,
            pageSize: 100,
            isLoading: false,
            pendingNewRows: [{}],
            results: result,
          }
        : null;
    const state = $state({
      activeProjectId: "p",
      activeConnectionId: "conn-1",
      activeConnection: connection,
      connections: [connection],
      schemas: { "conn-1": c.input.schemaCache },
      dataTabsByProject: { p: dataTab ? [dataTab] : [] },
      activeDataTabIdByProject: {},
      queryTabsByProject: {
        p: c.input.via === "queryTab" ? [{ id: "tab-1", query: "", results: [result] }] : [],
      },
      pendingChangesByConnection: {} as Record<string, PendingChange[]>,
      pendingChangesInterrupted: {},
      isPendingChangesOpen: false,
    });
    const db = state as unknown as DatabaseState;
    const pending = new PendingChangesManager(db, providers, history);
    const queries = new QueryExecutionManager(db, history, providers, pending);
    const dataTabs = new DataTabManager(
      db,
      { add: vi.fn() } as unknown as TabOrderingManager,
      () => {},
      () => {},
      queries,
      providers,
    );
    const source = c.input.result.sourceTable;

    for (const [i, s] of c.steps.entries()) {
      rowsAffected = s.driver.find((d) => d.op === "execute")?.answer?.rowsAffected ?? 1;
      const before = seen.length;
      let outcome: Json;
      if (dataTab) {
        if (s.action === "updateCell") {
          outcome = await dataTabs.updateCell("data-1", s.row!, s.column!, decodeCell(s.value));
        } else if (s.action === "setDefault") {
          outcome = await dataTabs.setCellDefault("data-1", s.row!, s.column!);
        } else if (s.action === "deleteRow") {
          outcome = await dataTabs.deleteRow("data-1", rows[s.row!]);
        } else {
          const values = Object.fromEntries(
            Object.entries(s.values!).map(([k, v]) => [k, decodeCell(v)]),
          );
          outcome = { saved: await dataTabs.saveNewRow("data-1", 0, values) };
        }
      } else if (s.action === "updateCell") {
        outcome = await queries.updateCell(
          "tab-1",
          0,
          s.row!,
          s.column!,
          decodeCell(s.value),
          source,
        );
      } else {
        outcome = await queries.deleteRowAt("tab-1", 0, s.row!, source);
      }
      await new Promise((resolve) => setTimeout(resolve, 0));
      const where = `${c.name} step ${i}`;
      if ("saved" in s.outcome) {
        expect(outcome.saved, where).toBe(s.outcome.saved);
      } else {
        expect(outcome.success, where).toBe(s.outcome.success);
        expect(!!outcome.queued, where).toBe(!!s.outcome.queued);
        if (s.outcome.code === "NO_ROWS_AFFECTED")
          expect(outcome.error, where).toBe(s.outcome.error);
      }
      const ran = seen.slice(before).filter((sql) => /^(UPDATE|DELETE|INSERT)/.test(sql));
      // Queued: the demo built the SQL and ran nothing. Immediate: the one statement.
      expect(ran, where).toHaveLength(c.input.pending ? 0 : 1);
    }

    const names = new Map<string, string>();
    const rename = (id: string) => {
      if (!names.has(id)) names.set(id, `c${names.size + 1}`);
      return names.get(id)!;
    };
    const queue = (state.pendingChangesByConnection["conn-1"] ?? []).map((entry) => {
      const id = rename(entry.id);
      return {
        id,
        change: { ...entry.change, id },
        origin: entry.origin,
        target: wireTarget(entry.target),
        description: entry.description,
      };
    });
    expect(queue).toEqual(
      c.queue.map((q) => ({
        id: q.id,
        change: q.change,
        origin: q.origin,
        target: q.target,
        description: q.description,
      })),
    );
  });

  it("builds the demo's SQL with values inlined", async () => {
    const { providers } = demoProvider(() => []);
    const { getEditService } = await import("./index.js");
    const service = await getEditService(connection as never, { schemas: {} }, providers);
    const [planned] = await service.plan({
      connectionId: "pc-1",
      edits: [
        {
          type: "updateCell",
          target: { schema: "main", table: "t" },
          key: [["id", { $sq: "bigint", v: "9007199254740993" }]],
          column: "j",
          value: { $sq: "json", v: { x: 2 } },
        },
      ],
    });
    expect(planned).toEqual({
      sql: `UPDATE "main"."t" SET "j" = '{"x":2}' WHERE "id" = 9007199254740993`,
      params: [],
      queryType: "update",
      dml: true,
      summary: { verb: "update", table: "t", column: "j" },
    });
  });
});

// -------- Apply --------

interface ApplyCase {
  name: string;
  engine: string;
  mode: "single" | "atomic" | "inOrder";
  input: { confirmed: boolean };
  queue: Array<{
    id: string;
    change: Change;
    sql: string;
    queryType: PendingChange["queryType"];
    dml: boolean;
    description: string;
    origin: PendingChange["origin"];
  }>;
  driver: Array<{ answer: { rowsAffected?: number; error?: { code: string; message: string } } }>;
  outcome: { executed?: number; failed?: number; failedChangeId?: string };
  queueAfter: string[];
}

describe("the demo's TsEditService replays the DuckDB apply cases", () => {
  const cases = (applyCases as unknown as ApplyCase[])
    .filter((c) => c.engine === "duckdb")
    .map(withChanges);

  it("has the DuckDB cases", () => {
    expect(cases.map((c) => c.name)).toEqual([
      "apply/duckdb-catalog-atomic",
      "apply/duckdb-sidebar-catalog-truncate-and-drop",
      "apply/duckdb-stale-middle",
      "apply/duckdb-error-middle",
    ]);
  });

  it.each(cases.map((c) => [c.name, c] as const))("%s", async (_name, c) => {
    const answers = c.driver.map((d) => d.answer);
    const { providers, seen } = demoProvider((sql) => {
      if (/^(BEGIN|COMMIT|ROLLBACK)/.test(sql)) return [];
      const answer = answers.shift();
      if (!answer) throw new Error(`unscripted: ${sql}`);
      if (answer.error) return new Error(answer.error.message);
      return [{ Count: BigInt(answer.rowsAffected ?? 0) }];
    });
    const state = $state({
      connections: [connection],
      schemas: {},
      pendingChangesByConnection: {
        "conn-1": c.queue.map((q): PendingChange => ({
          id: q.id,
          connectionId: "conn-1",
          change: q.change,
          sql: q.sql,
          queryType: q.queryType,
          dml: q.dml,
          addedAt: new Date(),
          description: q.description,
          origin: q.origin,
        })),
      } as Record<string, PendingChange[]>,
      pendingChangesInterrupted: {},
      isPendingChangesOpen: false,
    });
    const manager = new PendingChangesManager(
      state as unknown as DatabaseState,
      providers,
      history,
    );
    const result = await manager.apply("conn-1", c.input.confirmed);

    expect(state.pendingChangesByConnection["conn-1"].map((q) => q.id)).toEqual(c.queueAfter);
    if (c.outcome.failed) {
      expect(result).toMatchObject({ kind: "failed", applied: 0, mode: c.mode });
    } else {
      expect(result).toMatchObject({ kind: "applied", applied: c.outcome.executed, mode: c.mode });
    }
    if (c.mode === "atomic") {
      expect(seen[0]).toBe("BEGIN TRANSACTION");
      expect(seen.at(-1)).toBe(c.outcome.failed ? "ROLLBACK" : "COMMIT");
    } else {
      expect(seen.some((sql) => sql.startsWith("BEGIN"))).toBe(false);
    }
    // History: one row per applied change, appended by the demo itself.
    expect(append).toHaveBeenCalledTimes(c.outcome.failed ? 0 : (c.outcome.executed ?? 0));
  });
});

// -------- Table page --------

interface PageCase {
  name: string;
  input: {
    tableQuery: { target: { schema: string; table: string } };
    page: number;
    pageSize: number;
    filters: DataTab["filters"];
    schemaCache: SchemaTable[];
    columns: string[];
    matching: unknown[][];
    countCell?: unknown;
  };
  driver: Array<{ op: "count" | "page"; sql: string; params: unknown[] }>;
  result: {
    columns: string[];
    rows: unknown[][];
    totalRows: number;
    totalPages: number;
    sourceTable: unknown;
  };
}

describe("the demo's TsEditService keeps today's data tab query", () => {
  const [, fullPage] = pageDuckdb as unknown as PageCase[];

  it(fullPage.name, async () => {
    const { input } = fullPage;
    const matching = decodeRows(structuredClone(input.matching));
    const asRows = (rows: unknown[][]) =>
      rows.map((r) => Object.fromEntries(input.columns.map((col, i) => [col, r[i]])));
    const { providers, seen } = demoProvider((sql) =>
      sql.startsWith("SELECT COUNT")
        ? [{ count: decodeCell(input.countCell ?? matching.length) }]
        : asRows(matching.slice(0, input.pageSize)),
    );
    const state = $state({
      activeProjectId: "p",
      activeConnectionId: "conn-1",
      activeConnection: connection,
      connections: [connection],
      schemas: { "conn-1": input.schemaCache },
      dataTabsByProject: {
        p: [
          {
            id: "data-1",
            connectionId: "conn-1",
            tableName: input.tableQuery.target.table,
            schemaName: input.tableQuery.target.schema,
            filters: input.filters,
            filterLogic: "AND",
            sortColumns: [],
            page: input.page,
            pageSize: input.pageSize,
            isLoading: false,
            pendingNewRows: [],
          } as DataTab,
        ],
      },
      activeDataTabIdByProject: {},
      queryTabsByProject: {},
      pendingChangesByConnection: {},
      pendingChangesInterrupted: {},
      isPendingChangesOpen: false,
    });
    const db = state as unknown as DatabaseState;
    const pending = new PendingChangesManager(db, providers, history);
    const queries = new QueryExecutionManager(db, history, providers, pending);
    const dataTabs = new DataTabManager(
      db,
      { add: vi.fn() } as unknown as TabOrderingManager,
      () => {},
      () => {},
      queries,
      providers,
    );
    await dataTabs.refresh("data-1");

    // The count, then the page, as the data tab sent them before 5c.
    expect(seen).toEqual(fullPage.driver.map((d) => d.sql));
    expect(state.dataTabsByProject.p[0].results).toMatchObject({
      columns: fullPage.result.columns,
      rows: decodeRows(structuredClone(fullPage.result.rows)),
      totalRows: fullPage.result.totalRows,
      totalPages: fullPage.result.totalPages,
      sourceTable: fullPage.result.sourceTable,
      isError: false,
    });
  });

  it("filters with the values inlined as escaped literals, since the demo's provider ignores binds", async () => {
    const { providers, seen } = demoProvider((sql) =>
      sql.startsWith("SELECT COUNT") ? [{ count: 1 }] : [{ id: 1, v: "a'b" }],
    );
    const t = table("t");
    const state = $state({
      activeProjectId: "p",
      activeConnectionId: "conn-1",
      activeConnection: connection,
      connections: [connection],
      schemas: { "conn-1": [t] },
      dataTabsByProject: {
        p: [
          {
            id: "data-1",
            connectionId: "conn-1",
            tableName: "t",
            schemaName: "main",
            filters: [
              { id: "f1", column: "v", operator: "=", value: "a'b", enabled: true },
              { id: "f2", column: "id", operator: "IN", value: " 1, 2 ,", enabled: true },
            ],
            filterLogic: "AND",
            sortColumns: [],
            page: 1,
            pageSize: 100,
            isLoading: false,
            pendingNewRows: [],
          } as DataTab,
        ],
      },
      activeDataTabIdByProject: {},
      queryTabsByProject: {},
      pendingChangesByConnection: {},
      pendingChangesInterrupted: {},
      isPendingChangesOpen: false,
    });
    const db = state as unknown as DatabaseState;
    const pending = new PendingChangesManager(db, providers, history);
    const queries = new QueryExecutionManager(db, history, providers, pending);
    const dataTabs = new DataTabManager(
      db,
      { add: vi.fn() } as unknown as TabOrderingManager,
      () => {},
      () => {},
      queries,
      providers,
    );
    await dataTabs.refresh("data-1");

    const where = `WHERE CAST("v" AS TEXT) = 'a''b' AND CAST("id" AS TEXT) IN ('1', '2')`;
    expect(seen).toEqual([
      `SELECT COUNT(*) FROM "main"."t" ${where}`,
      `SELECT * FROM "main"."t" ${where} LIMIT 100 OFFSET 0`,
    ]);
    expect(state.dataTabsByProject.p[0].results).toMatchObject({
      rows: [[1, "a'b"]],
      totalRows: 1,
      isError: false,
    });
  });
});
