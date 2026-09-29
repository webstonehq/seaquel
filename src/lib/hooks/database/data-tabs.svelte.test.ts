/**
 * The data tab over the edits service (phase 5c): a page is a `db.tablePage`
 * stream on the tab's own connection, one refresh at a time per tab; edits
 * in a tab run on the tab's connection whatever is active in the sidebar.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DataFilter, DataTab, SchemaTable } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import type {
  ApplyChangesParams,
  PlanEditsParams,
  RunEvent,
  TablePageParams,
} from "./edit-service/types";

const settings = vi.hoisted(() => ({ enabled: false }));
vi.mock("$lib/stores/pending-changes-settings.svelte.js", () => ({
  pendingChangesSettingsStore: settings,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { info: vi.fn(), success: vi.fn() } }));

const { DataTabManager } = await import("./data-tabs.svelte.js");
const { QueryExecutionManager } = await import("./query-execution.svelte.js");
const { PendingChangesManager } = await import("./pending-changes.svelte.js");
const { CoreEditService, setEditService } = await import("./edit-service/index.js");
const { scriptedCore } = await import("./edit-service/scripted-core.js");
const { cancelledEvent } = await import("$lib/core/client");

function table(name: string, schema = "public"): SchemaTable {
  return {
    name,
    schema,
    type: "table",
    columns: [
      { name: "id", type: "integer", nullable: false, isPrimaryKey: true, isForeignKey: false },
      { name: "name", type: "text", nullable: true, isPrimaryKey: false, isForeignKey: false },
    ],
    indexes: [],
  } as unknown as SchemaTable;
}

const connA = { id: "conn-a", type: "postgres", name: "A", providerConnectionId: "pc-a" };
const connB = { id: "conn-b", type: "postgres", name: "B", providerConnectionId: "pc-b" };

/** A page's events as Core sends them. */
function pageEvents(
  p: TablePageParams,
  rows: unknown[][],
  opts: { total?: number; estimated?: boolean; columns?: string[] } = {},
): RunEvent[] {
  return [
    {
      type: "statementStart",
      index: 0,
      sql: "SELECT …",
      source: { sql: `SELECT * FROM "${p.query.target.table}"`, params: [] },
      queryType: "select",
      kind: "page",
      page: p.page,
      pageSize: p.pageSize,
    },
    { type: "batch", columns: opts.columns ?? ["id", "name"], rows, is_final: true },
    {
      type: "statementDone",
      index: 0,
      elapsedMs: 3,
      totalRows: opts.total ?? rows.length,
      totalPages: Math.max(1, Math.ceil((opts.total ?? rows.length) / p.pageSize)),
      countEstimated: opts.estimated ?? false,
    },
    { type: "done", statements: 1, succeeded: true },
  ];
}

/** Yields `events`, after `wait` if given; ends as cancelled when `signal` aborts. */
async function* stream(
  events: RunEvent[],
  signal?: AbortSignal,
  wait?: Promise<void>,
): AsyncGenerator<RunEvent> {
  if (wait) await wait;
  for (const event of events) {
    if (signal?.aborted) {
      yield cancelledEvent();
      return;
    }
    yield event;
  }
}

type PageHandler = (p: TablePageParams, signal?: AbortSignal) => AsyncIterable<RunEvent>;

function setup(opts: { tablePage?: PageHandler } = {}) {
  const state = $state({
    activeProjectId: "p",
    activeConnectionId: "conn-a" as string | null,
    activeConnection: connA as typeof connA | typeof connB,
    connections: [connA, connB],
    schemas: { "conn-a": [table("users")], "conn-b": [table("accounts")] } as Record<
      string,
      SchemaTable[]
    >,
    dataTabsByProject: {} as Record<string, DataTab[]>,
    activeDataTabIdByProject: {} as Record<string, string | null>,
    queryTabsByProject: {},
    pendingChangesByConnection: {} as Record<string, unknown[]>,
    pendingChangesInterrupted: {} as Record<string, boolean>,
    isPendingChangesOpen: false,
  });
  const core = scriptedCore({
    tablePage: opts.tablePage ?? ((p, signal) => stream(pageEvents(p, [[1, "a"]]), signal)),
  });
  setEditService(new CoreEditService(() => core.client));
  const providers = {} as ProviderRegistry;
  const db = state as unknown as DatabaseState;
  const history = {
    contextFor: (id: string) => ({ connectionId: id, connectionName: "A", connectionLabels: [] }),
    insertRecorded: vi.fn(),
  } as unknown as QueryHistoryManager;
  const pending = new PendingChangesManager(db, providers, history);
  const queries = new QueryExecutionManager(db, history, providers, pending);
  const dataTabs = new DataTabManager(
    db,
    {
      add: vi.fn(),
      removeTabGeneric: (
        getTabs: () => Record<string, DataTab[]>,
        setTabs: (r: Record<string, DataTab[]>) => void,
        _getActive: unknown,
        _setActive: unknown,
        id: string,
      ) => {
        const tabs = getTabs();
        setTabs({ ...tabs, p: (tabs.p ?? []).filter((t) => t.id !== id) });
      },
    } as unknown as TabOrderingManager,
    () => {},
    () => {},
    queries,
    providers,
  );
  const tab = (id: string) => state.dataTabsByProject.p?.find((t) => t.id === id);
  const makeBActive = () => {
    state.activeConnectionId = "conn-b";
    state.activeConnection = connB;
  };
  const pages = () => core.of("tablePage") as TablePageParams[];
  const applies = () => core.of("applyChanges") as ApplyChangesParams[];
  const plans = () => core.of("planEdits") as PlanEditsParams[];
  return { state, dataTabs, core, tab, makeBActive, pages, applies, plans };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  settings.enabled = false;
});
afterEach(() => {
  setEditService(null);
});

describe("a data tab page is a tablePage stream", () => {
  it("sends the tab's table, enabled filters, logic, sort and page, on the tab's connection", async () => {
    const { dataTabs, pages, tab, makeBActive } = setup();
    const tabId = dataTabs.add(table("users"))!;
    await settle();
    makeBActive();
    const filters: DataFilter[] = [
      { id: "1", column: "id", operator: "IN", value: " 1, 2 ,", enabled: true },
      { id: "2", column: "name", operator: "LIKE", value: "a%", enabled: false },
      { id: "3", column: "", operator: "=", value: "x", enabled: true },
      { id: "4", column: "name", operator: "IS NULL", value: "", enabled: true },
    ];
    dataTabs.setFilters(tabId, filters, "OR");
    dataTabs.setSorting(tabId, [{ column: "name", direction: "DESC" }]);
    dataTabs.setPage(tabId, 3);
    await settle();

    const last = pages().at(-1)!;
    expect(last).toEqual({
      connectionId: "pc-a",
      streamId: expect.any(String),
      query: {
        target: { schema: "public", table: "users" },
        filters: [
          { column: "id", op: "IN", value: " 1, 2 ," },
          { column: "name", op: "IS NULL", value: "" },
        ],
        logic: "OR",
        sort: [{ column: "name", direction: "DESC" }],
      },
      page: 3,
      pageSize: 100,
    });
    expect(new Set(pages().map((p) => p.streamId)).size).toBe(pages().length);
    expect(tab(tabId)?.results).toMatchObject({
      columns: ["id", "name"],
      rows: [[1, "a"]],
      connectionId: "conn-a",
      statementSql: 'SELECT * FROM "users"',
      sourceTable: { schema: "public", name: "users", primaryKeys: ["id"] },
      isError: false,
    });
  });

  it("decodes rows and shows Core's totals", async () => {
    const { dataTabs, tab } = setup({
      tablePage: (p, s) =>
        stream(pageEvents(p, [[{ $sq: "bigint", v: "9007199254740993" }, "a"]], { total: 250 }), s),
    });
    const tabId = dataTabs.add(table("users"))!;
    await settle();
    expect(tab(tabId)?.results).toMatchObject({
      rows: [[9007199254740993n, "a"]],
      totalRows: 250,
      totalPages: 3,
    });
    expect(tab(tabId)?.results?.countEstimated).toBeUndefined();
    expect(tab(tabId)?.isLoading).toBe(false);
  });

  it("a data tab with a failed count shows an estimate", async () => {
    const { dataTabs, tab } = setup({
      tablePage: (p, s) => stream(pageEvents(p, [[1, "a"]], { total: 101, estimated: true }), s),
    });
    const tabId = dataTabs.add(table("users"))!;
    await settle();
    expect(tab(tabId)?.results).toMatchObject({ totalRows: 101, countEstimated: true });
  });

  it("an empty page takes its columns from Core, else the schema cache", async () => {
    const withColumns = setup({
      tablePage: (p, s) => stream(pageEvents(p, [], { columns: ["id", "name", "extra"] }), s),
    });
    const a = withColumns.dataTabs.add(table("users"))!;
    await settle();
    expect(withColumns.tab(a)?.results?.columns).toEqual(["id", "name", "extra"]);
    setEditService(null);

    const without = setup({
      tablePage: (p, s) =>
        stream(
          pageEvents(p, []).map((e) => (e.type === "batch" ? { ...e, columns: null } : e)),
          s,
        ),
    });
    const b = without.dataTabs.add(table("users"))!;
    await settle();
    expect(without.tab(b)?.results?.columns).toEqual(["id", "name"]);
  });

  it("a failed page shows the error and stops the spinner", async () => {
    const { dataTabs, tab } = setup({
      tablePage: (p, s) =>
        stream(
          [
            pageEvents(p, [])[0],
            {
              type: "statementError",
              index: 0,
              code: "INVALID_ARGUMENT",
              message: "empty IN",
              elapsedMs: 1,
            },
            { type: "done", statements: 1, succeeded: false },
          ],
          s,
        ),
    });
    const tabId = dataTabs.add(table("users"))!;
    await settle();
    expect(tab(tabId)).toMatchObject({
      isLoading: false,
      results: { isError: true, error: "INVALID_ARGUMENT: empty IN" },
    });
  });

  it("a new refresh cancels the old; a slower earlier page can't overwrite a later one", async () => {
    const signals: AbortSignal[] = [];
    let release!: () => void;
    const slow = new Promise<void>((resolve) => (release = resolve));
    const { dataTabs, tab } = setup({
      tablePage: (p, s) => {
        signals.push(s!);
        const rows = [[p.page, `page ${p.page}`]];
        return stream(pageEvents(p, rows), s, p.page === 1 ? slow : undefined);
      },
    });
    const tabId = dataTabs.addWithoutRefresh(table("users"))!;
    const first = dataTabs.refresh(tabId);
    dataTabs.setPage(tabId, 2);
    await settle();
    expect(signals[0].aborted).toBe(true);
    expect(signals[1].aborted).toBe(false);
    release();
    await first;
    await settle();
    expect(tab(tabId)?.results?.rows).toEqual([[2, "page 2"]]);
    expect(tab(tabId)?.isLoading).toBe(false);
  });

  it("closing the tab cancels its page", async () => {
    const signals: AbortSignal[] = [];
    const { dataTabs, state } = setup({
      tablePage: (p, s) => {
        signals.push(s!);
        return stream(pageEvents(p, []), s, new Promise(() => {}));
      },
    });
    const tabId = dataTabs.add(table("users"))!;
    await settle();
    dataTabs.remove(tabId);
    expect(signals[0].aborted).toBe(true);
    expect(state.dataTabsByProject.p).toEqual([]);
  });

  it("a project reload cancels its tabs' pages", async () => {
    const signals: AbortSignal[] = [];
    const { dataTabs } = setup({
      tablePage: (p, s) => {
        signals.push(s!);
        return stream(pageEvents(p, []), s, new Promise(() => {}));
      },
    });
    dataTabs.add(table("users"));
    await settle();
    dataTabs.cancelProject("other");
    expect(signals[0].aborted).toBe(false);
    dataTabs.cancelProject("p");
    expect(signals[0].aborted).toBe(true);
  });

  it("a disconnected tab sends nothing and leaves no spinner", async () => {
    const { dataTabs, state, pages, tab } = setup();
    const tabId = dataTabs.addWithoutRefresh(table("users"))!;
    state.connections[0] = { ...state.connections[0], providerConnectionId: undefined } as never;
    await dataTabs.refresh(tabId);
    expect(pages()).toEqual([]);
    expect(tab(tabId)?.isLoading).toBe(false);
  });
});

describe("a data tab edits its own connection", () => {
  it("a data tab on connection A edits A while B is active in the sidebar", async () => {
    const { dataTabs, makeBActive, tab, applies, pages } = setup();
    const tabId = dataTabs.add(table("users"))!;
    await settle();
    expect(tab(tabId)?.results?.rows).toEqual([[1, "a"]]);
    makeBActive();

    expect(await dataTabs.updateCell(tabId, 0, "name", "b")).toMatchObject({ success: true });
    expect(await dataTabs.setCellDefault(tabId, 0, "name")).toMatchObject({ success: true });
    expect(await dataTabs.deleteRow(tabId, [1, "a"])).toMatchObject({ success: true });
    expect(await dataTabs.saveNewRow(tabId, 0, { name: "n" })).toBe(true);
    await settle();

    expect(
      applies().map((p) => [
        p.connectionId,
        p.changes[0].type === "edit" && p.changes[0].edit.type,
        p.changes[0].type === "edit" && p.changes[0].edit.target,
      ]),
    ).toEqual([
      ["pc-a", "updateCell", { schema: "public", table: "users" }],
      ["pc-a", "setDefault", { schema: "public", table: "users" }],
      ["pc-a", "deleteRow", { schema: "public", table: "users" }],
      ["pc-a", "insertRow", { schema: "public", table: "users" }],
    ]);
    // The refreshes after each edit read A too.
    expect(pages().length).toBeGreaterThan(1);
    for (const p of pages()) expect(p.connectionId).toBe("pc-a");
  });

  it("with pending changes on, a data tab's changes queue under its connection", async () => {
    const { dataTabs, makeBActive, state, applies, plans } = setup();
    const tabId = dataTabs.add(table("users"))!;
    await settle();
    makeBActive();
    settings.enabled = true;

    await dataTabs.updateCell(tabId, 0, "name", "b");
    await dataTabs.setCellDefault(tabId, 0, "name");
    await dataTabs.deleteRow(tabId, [1, "a"]);
    await dataTabs.saveNewRow(tabId, 0, { name: "n" });

    expect(applies()).toEqual([]);
    expect(plans().map((p) => p.connectionId)).toEqual(["pc-a", "pc-a", "pc-a", "pc-a"]);
    expect(state.pendingChangesByConnection["conn-b"]).toBeUndefined();
    // The cell edit and Set default of one cell dedupe into one change.
    expect(
      (state.pendingChangesByConnection["conn-a"] as { origin: string }[]).map((c) => c.origin),
    ).toEqual(["set-default", "delete-row", "insert-row"]);
  });

  it("a data tab whose connection disconnected refuses edits and sends nothing", async () => {
    const { dataTabs, makeBActive, state, applies, plans } = setup();
    const tabId = dataTabs.add(table("users"))!;
    await settle();
    makeBActive();
    state.connections[0] = { ...state.connections[0], providerConnectionId: undefined } as never;

    expect(await dataTabs.updateCell(tabId, 0, "name", "b")).toMatchObject({
      success: false,
      error: "No connection established",
    });
    expect(await dataTabs.saveNewRow(tabId, 0, { name: "n" })).toBe(false);
    expect(applies()).toEqual([]);
    expect(plans()).toEqual([]);
  });
});
