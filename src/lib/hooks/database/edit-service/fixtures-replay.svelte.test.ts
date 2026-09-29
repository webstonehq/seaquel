/**
 * The TypeScript replay of `crates/seaquel-workspace/tests/fixtures/edits`
 * (phase 5c, Task 6; the README's "TypeScript replay"): the real view models
 * (`QueryExecutionManager`, `DataTabManager`, `PendingChangesManager`) over
 * `CoreEditService` and a Core scripted from each case.
 *
 * - Plan files: each step sends the case's intent (`edit`), the grid sees the
 *   recorded outcome (`success`, `queued`, `saved`, the no-row-matched text,
 *   the GUI's own refusals, `refreshed`, `reran`), and the queue ends as
 *   recorded: ids in order of first appearance, each entry's `change`,
 *   `origin`, `target` and `description`, with dedupe.
 * - Apply: the queue sent in order with the sheet's `confirmed`, and what the
 *   queue holds after the outcome (`queueAfter`).
 * - Summary: `describePendingChange` over each statement, through the real
 *   seaquel-wasm `change_summary`.
 *
 * `changes.json`'s `expected` fields replace the recorded ones first.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DataTab, PendingChange, SchemaTable, StatementResult } from "$lib/types";
import type { DatabaseType } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "../state.svelte.js";
import type { TabOrderingManager } from "../tab-ordering.svelte.js";
import type { QueryHistoryManager } from "../query-history.svelte.js";
import type { QueryRunner } from "../query-runner/types";
import type { ApplyOutcome, Change, Edit, PlannedChange, RunEvent } from "./types";
import { decodeCell, decodeRows, encodeParam } from "$lib/values";
import applyCases from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/apply.json";
import changesFile from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/changes.json";
import summaryCases from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/summary.json";
import planPostgres from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/plan-postgres.json";
import planMysql from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/plan-mysql.json";
import planMariadb from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/plan-mariadb.json";
import planSqlite from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/plan-sqlite.json";
import planMssql from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/plan-mssql.json";
import planDuckdb from "../../../../../crates/seaquel-workspace/tests/fixtures/edits/plan-duckdb.json";

const settings = vi.hoisted(() => ({ enabled: false }));
vi.mock("$lib/stores/pending-changes-settings.svelte.js", () => ({
  pendingChangesSettingsStore: settings,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));
vi.mock("$lib/stores/license-nudge.svelte.js", () => ({
  licenseNudgeStore: { recordQuery: vi.fn() },
}));

const { DataTabManager } = await import("../data-tabs.svelte.js");
const { QueryExecutionManager } = await import("../query-execution.svelte.js");
const { PendingChangesManager } = await import("../pending-changes.svelte.js");
const { describePendingChange } = await import("../pending-change-description.js");
const { CoreEditService, setEditService } = await import("./index.js");
const { scriptedCore, refusal } = await import("./scripted-core.js");

// -------- The fixture files --------

type Json = Record<string, unknown>;
type Changes = Record<string, { expected: Json }>;
const changes = changesFile as unknown as Changes;

/** A case with `changes.json`'s expected fields in place of the recorded ones. */
function withChanges<T extends { name: string }>(c: T): T {
  const expected = changes[c.name]?.expected;
  return expected ? ({ ...c, ...expected } as T) : c;
}

interface QueueEntry {
  id: string;
  change: Change;
  sql: string;
  params: unknown[];
  queryType: PlannedChange["queryType"];
  dml: boolean;
  summary: PlannedChange["summary"] | null;
  description: string;
  origin: PendingChange["origin"];
  target?: Json;
}

interface PlanStep {
  action: "updateCell" | "setDefault" | "deleteRow" | "insertRow";
  row?: number;
  column?: string;
  value?: unknown;
  values?: Json;
  edit: Edit | null;
  driver: Array<{ op: string; answer?: Json }>;
  outcome: { success?: boolean; queued?: boolean; saved?: boolean; error?: string; code?: string };
  refreshed?: boolean;
  reran?: boolean;
}

interface PlanCase {
  name: string;
  engine: DatabaseType;
  input: {
    via: "dataTab" | "queryTab";
    pending: boolean;
    table?: { schema: string; table: string };
    query?: string;
    result: {
      columns: string[];
      rows: unknown[][];
      sourceTable: { schema: string; name: string; primaryKeys: string[] } | null;
      columnSources: StatementResult["columnSources"] | null;
    };
    schemaCache: SchemaTable[];
  };
  steps: PlanStep[];
  queue: QueueEntry[];
}

const planCases = [planPostgres, planMysql, planMariadb, planSqlite, planMssql, planDuckdb]
  .flat()
  .map((c) => withChanges(c as unknown as PlanCase));

/** A target's values in the cell wire format, as the fixtures hold them. */
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

/** Ids renamed `c1`, `c2`, … in order of first appearance, as the recorder named them. */
function renamer() {
  const names = new Map<string, string>();
  return (id: string) => {
    if (!names.has(id)) names.set(id, `c${names.size + 1}`);
    return names.get(id)!;
  };
}

// -------- The plan replay --------

async function* events(list: RunEvent[]): AsyncGenerator<RunEvent> {
  yield* list;
}

/** The table as it was loaded: a refresh after an immediate edit shows the same rows. */
function samePage(columns: string[], rows: unknown[][]): AsyncIterable<RunEvent> {
  return events([
    { type: "batch", columns, rows: structuredClone(rows), is_final: true },
    {
      type: "statementDone",
      index: 0,
      elapsedMs: 0,
      totalRows: rows.length,
      totalPages: 1,
      countEstimated: false,
    },
    { type: "done", statements: 1, succeeded: true },
  ]);
}

function replayPlan(c: PlanCase) {
  settings.enabled = c.input.pending;
  const connection = { id: "conn-1", type: c.engine, name: "db", providerConnectionId: "pc-1" };
  const rows = decodeRows(structuredClone(c.input.result.rows));
  const sourceTable = c.input.result.sourceTable ?? undefined;
  const result: StatementResult = {
    columns: c.input.result.columns,
    rows,
    rowCount: rows.length,
    totalRows: rows.length,
    executionTime: 0,
    page: 1,
    pageSize: 100,
    totalPages: 1,
    queryType: "select",
    statementIndex: 0,
    statementSql: c.input.query ?? "",
    connectionId: "conn-1",
    isError: false,
    ...(sourceTable ? { sourceTable } : {}),
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
      p:
        c.input.via === "queryTab"
          ? [{ id: "tab-1", query: c.input.query, results: [result] }]
          : [],
    },
    pendingChangesByConnection: {} as Record<string, PendingChange[]>,
    pendingChangesInterrupted: {},
    isPendingChangesOpen: false,
  });

  // The step running now: Core answers it from its recording.
  let step: PlanStep | null = null;
  const planned = (edit: Edit): PlannedChange => {
    const entry = c.queue.find((q) => q.change.type === "edit" && sameEdit(q.change.edit, edit));
    if (entry) {
      const { sql, params, queryType, dml, summary } = entry;
      return { sql, params, queryType, dml, ...(summary ? { summary } : {}) };
    }
    const build = step?.driver.find((d) => d.op === "build")?.answer as
      | { sql: string; bindValues?: unknown[] }
      | undefined;
    const queryType = { updateCell: "update", setDefault: "update", insertRow: "insert" }[
      edit.type as string
    ] as PlannedChange["queryType"] | undefined;
    return {
      sql: build?.sql ?? "",
      params: build?.bindValues ?? [],
      queryType: queryType ?? "delete",
      dml: true,
    };
  };
  const failure = () => {
    const execute = step?.driver.find((d) => d.op === "execute")?.answer as
      | { error?: { code: string; message: string } }
      | undefined;
    return {
      code: step!.outcome.code ?? execute?.error?.code ?? "EXECUTE_ERROR",
      message: execute?.error?.message ?? "refused",
    };
  };
  const core = scriptedCore({
    planEdits: (p) => {
      if (step?.outcome.success === false || step?.outcome.saved === false) {
        const { code, message } = failure();
        throw refusal(code, message);
      }
      return p.edits.map(planned);
    },
    applyChanges: (p): ApplyOutcome => {
      const execute = step?.driver.find((d) => d.op === "execute")?.answer as
        | { rowsAffected?: number; lastInsertId?: number }
        | undefined;
      const ok = step?.outcome.success === true || step?.outcome.saved === true;
      return {
        outcome: "applied",
        mode: "single",
        applied: ok ? 1 : 0,
        results: ok
          ? [
              {
                id: p.changes[0].id,
                rowsAffected: execute?.rowsAffected ?? 1,
                ...(execute?.lastInsertId === undefined
                  ? {}
                  : { lastInsertId: execute.lastInsertId }),
              },
            ]
          : [],
        ...(ok ? {} : { failed: { id: p.changes[0].id, index: 0, ...failure() } }),
        ddl: false,
        history: [],
      };
    },
    tablePage: () => samePage(c.input.result.columns, c.input.result.rows),
  });
  setEditService(new CoreEditService(() => core.client));

  const runs: unknown[] = [];
  const runner: QueryRunner = {
    run: (params) => {
      runs.push(params);
      return events([{ type: "done", statements: 0, succeeded: true }]);
    },
    page: () => events([{ type: "done", statements: 0, succeeded: true }]),
  };
  const db = state as unknown as DatabaseState;
  const history = {
    contextFor: (id: string) => ({ connectionId: id, connectionName: "db", connectionLabels: [] }),
    insertRecorded: vi.fn(),
  } as unknown as QueryHistoryManager;
  const providers = {} as ProviderRegistry;
  const pending = new PendingChangesManager(db, providers, history);
  const queries = new QueryExecutionManager(db, history, providers, pending, async () => runner);
  const dataTabs = new DataTabManager(
    db,
    { add: vi.fn() } as unknown as TabOrderingManager,
    () => {},
    () => {},
    queries,
    providers,
  );
  const fallback = sourceTable ?? { schema: "", name: "", primaryKeys: [] };
  const sent = () =>
    core.calls
      .filter((call) => call.method === "planEdits" || call.method === "applyChanges")
      .map((call) => {
        const params = call.params as { edits?: Edit[]; changes?: Array<{ edit: Edit }> };
        return params.edits ? params.edits[0] : params.changes![0].edit;
      });

  async function run(s: PlanStep) {
    step = s;
    const before = { calls: sent().length, pages: core.of("tablePage").length, runs: runs.length };
    let outcome: Json;
    if (dataTab) {
      switch (s.action) {
        case "updateCell":
          outcome = await dataTabs.updateCell("data-1", s.row!, s.column!, decodeCell(s.value));
          break;
        case "setDefault":
          outcome = await dataTabs.setCellDefault("data-1", s.row!, s.column!);
          break;
        case "deleteRow":
          outcome = await dataTabs.deleteRow("data-1", rows[s.row!]);
          break;
        case "insertRow": {
          const values = Object.fromEntries(
            Object.entries(s.values!).map(([k, v]) => [k, decodeCell(v)]),
          );
          outcome = { saved: await dataTabs.saveNewRow("data-1", 0, values) };
          break;
        }
      }
    } else {
      switch (s.action) {
        case "updateCell":
          outcome = await queries.updateCell(
            "tab-1",
            0,
            s.row!,
            s.column!,
            decodeCell(s.value),
            fallback,
          );
          break;
        case "setDefault":
          outcome = await queries.setCellDefault("tab-1", 0, s.row!, s.column!, fallback);
          break;
        case "deleteRow":
          outcome = await queries.deleteRowAt("tab-1", 0, s.row!, fallback);
          break;
        default:
          throw new Error(`no query tab ${s.action}`);
      }
    }
    await new Promise((resolve) => setTimeout(resolve, 0));
    return {
      outcome,
      sent: sent().slice(before.calls),
      refreshed: core.of("tablePage").length > before.pages,
      reran: runs.length > before.runs,
    };
  }

  return { run, state };
}

function sameEdit(a: Edit, b: Edit): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

beforeEach(() => {
  settings.enabled = false;
});
afterEach(() => {
  setEditService(null);
});

describe("the plan fixtures through the view models", () => {
  it("reads every plan case", () => {
    expect(planCases).toHaveLength(49);
  });

  it.each(planCases.map((c) => [c.name, c] as const))("%s", async (_name, c) => {
    const { run, state } = replayPlan(c);
    for (const [i, s] of c.steps.entries()) {
      const got = await run(s);
      const where = `${c.name} step ${i}`;
      // The intent sent, once, or nothing for a step the GUI refused itself.
      expect(got.sent, where).toEqual(s.edit ? [s.edit] : []);
      const o = s.outcome;
      if ("saved" in o) {
        expect(got.outcome.saved, where).toBe(o.saved);
      } else {
        expect(got.outcome.success, where).toBe(o.success);
        expect(!!got.outcome.queued, where).toBe(!!o.queued);
        if (o.code === "NO_ROWS_AFFECTED" || (o.success === false && !s.edit)) {
          expect(got.outcome.error, where).toBe(o.error);
        }
      }
      expect(got.refreshed, where).toBe(!!s.refreshed);
      expect(got.reran, where).toBe(!!s.reran);
    }

    const rename = renamer();
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
});

// -------- The apply replay --------

interface ApplyCase {
  name: string;
  engine: DatabaseType;
  mode: "single" | "atomic" | "inOrder";
  input: { confirmed: boolean };
  queue: QueueEntry[];
  outcome: {
    executed?: number;
    failed?: number;
    failedAt?: number;
    failedChangeId?: string;
    error?: string;
    code?: string;
    hasDdl?: boolean;
    confirmRequired?: boolean;
    destructive?: Array<{ index: number; sql: string; reason: string }>;
    destructiveTotal?: number;
  };
  queueAfter: string[];
}

const applyReplay = (applyCases as unknown as ApplyCase[]).map((c) => withChanges(c));

/** A recorded failure's code: its `CODE: ` prefix, or the no-row-matched text's. */
function codeOf(o: ApplyCase["outcome"]): string {
  if (o.code) return o.code;
  if (o.error?.startsWith("No row in ")) return "NO_ROWS_AFFECTED";
  return /^([A-Z][A-Z0-9_]*): /.exec(o.error ?? "")?.[1] ?? "EXECUTE_ERROR";
}

describe("the apply fixtures through the pending-changes queue", () => {
  it("reads every apply case", () => {
    expect(applyReplay).toHaveLength(29);
  });

  it.each(applyReplay.map((c) => [c.name, c] as const))("%s", async (_name, c) => {
    const connection = { id: "conn-1", type: c.engine, name: "db", providerConnectionId: "pc-1" };
    const state = $state({
      connections: [connection],
      pendingChangesByConnection: {
        "conn-1": c.queue.map((q): PendingChange => ({
          id: q.id,
          connectionId: "conn-1",
          change: q.change,
          sql: q.sql,
          bindValues: q.params.map(decodeCell),
          queryType: q.queryType,
          dml: q.dml,
          addedAt: new Date(),
          description: q.description,
          origin: q.origin,
          ...(q.target
            ? {
                target: JSON.parse(JSON.stringify(q.target), (_k, v) =>
                  typeof v === "object" && v && "$sq" in v ? decodeCell(v) : v,
                ),
              }
            : {}),
        })),
      } as Record<string, PendingChange[]>,
      pendingChangesInterrupted: {},
      isPendingChangesOpen: false,
    });
    const o = c.outcome;
    const core = scriptedCore({
      applyChanges: (): ApplyOutcome =>
        o.confirmRequired
          ? {
              outcome: "confirmRequired",
              destructive: o.destructive as never,
              destructiveTotal: o.destructiveTotal ?? 0,
            }
          : {
              outcome: "applied",
              mode: c.mode,
              applied: o.executed ?? 0,
              results: [],
              ...(o.failed
                ? {
                    failed: {
                      ...(o.failedChangeId ? { id: o.failedChangeId } : {}),
                      ...(o.failedAt === undefined ? {} : { index: o.failedAt }),
                      code: codeOf(o),
                      message: o.error?.replace(/^[A-Z][A-Z0-9_]*: /, "") ?? "refused",
                    },
                  }
                : {}),
              ddl: !!o.hasDdl,
              history: [],
            },
    });
    const history = {
      contextFor: (id: string) => ({
        connectionId: id,
        connectionName: "db",
        connectionLabels: [],
      }),
      insertRecorded: vi.fn(),
    } as unknown as QueryHistoryManager;
    const service = new CoreEditService(() => core.client);
    const manager = new PendingChangesManager(
      state as unknown as DatabaseState,
      {} as ProviderRegistry,
      history,
      async () => service,
    );
    const result = await manager.apply("conn-1", c.input.confirmed);

    const [sent] = core.of("applyChanges") as Array<{ changes: Change[]; confirmed?: boolean }>;
    expect(sent.changes).toEqual(c.queue.map((q) => q.change));
    expect(!!sent.confirmed).toBe(c.input.confirmed);
    expect(state.pendingChangesByConnection["conn-1"].map((q) => q.id)).toEqual(c.queueAfter);
    if (o.confirmRequired) {
      expect(result.kind).toBe("confirmRequired");
    } else if (o.failed) {
      expect(result).toMatchObject({ kind: "failed", applied: o.executed ?? 0 });
      if (o.error) expect(result.kind === "failed" && result.error).toBe(o.error);
    } else {
      expect(result).toMatchObject({ kind: "applied", applied: o.executed });
    }
  });
});

// -------- Descriptions --------

interface SummaryCase {
  name: string;
  engine: DatabaseType;
  origin: PendingChange["origin"];
  sql: string;
  description: string;
}

describe("descriptions from Core's change summary", () => {
  const cases = (summaryCases as unknown as SummaryCase[]).map((c) => withChanges(c));

  it("reads every summary case", () => {
    expect(cases).toHaveLength(52);
  });

  it.each(cases.map((c) => [c.name, c] as const))("%s", (_name, c) => {
    expect(describePendingChange(c.sql, c.origin, c.engine)).toBe(c.description);
  });
});
