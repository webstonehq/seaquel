import { afterEach, describe, expect, it, vi } from "vitest";
import type { Dashboard, DashboardWidget } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";

vi.mock("$lib/storage", () => ({
  getDatabase: vi.fn(async () => ({})),
  dashboardsRepo: { save: vi.fn(async () => {}), remove: vi.fn(async () => {}) },
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/engine", () => ({ getEngineClient: vi.fn(), usesRustEngine: () => false }));

const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { QueryCrudManager } = await import("./query-crud.svelte.js");

function widget(overrides: Partial<DashboardWidget> = {}): DashboardWidget {
  return {
    id: "w-1",
    title: "n",
    x: 0,
    y: 0,
    width: 220,
    height: 140,
    querySource: "custom",
    query: "SELECT 1 AS n",
    widgetType: "kpi",
    ...overrides,
  };
}

/**
 * A dashboard manager wired to the real `QueryCrudManager` and a fake
 * provider, as `database.svelte.ts` wires them.
 */
function setup(
  w: DashboardWidget,
  opts: {
    dateFilter?: Dashboard["dateFilter"];
    activeConnectionId?: string | null;
    /** The fake query never finishes on its own; only an abort ends it. */
    hold?: boolean;
  } = {},
) {
  const connection = {
    id: "conn-1",
    type: "postgres",
    name: "Local",
    providerConnectionId: "pc-1",
  };
  const dashboard: Dashboard = {
    id: "d-1",
    name: "D",
    projectId: "p",
    widgets: [w],
    viewport: { x: 0, y: 0, zoom: 1 },
    dateFilter: opts.dateFilter ?? null,
    createdAt: new Date(),
    updatedAt: new Date(),
    shared: false,
  };
  const activeConnectionId =
    opts.activeConnectionId === undefined ? "conn-1" : opts.activeConnectionId;
  const state = {
    activeProjectId: "p",
    activeConnectionId,
    activeConnection: activeConnectionId ? connection : null,
    connections: [connection],
    schemas: {},
    dashboardsByProject: { p: [dashboard] },
    queriesByProject: { p: [{ id: "sq-1", query: "SELECT count(*) AS n FROM saved" }] },
  } as unknown as DatabaseState;
  const provider = {
    select: vi.fn(async () => [{ n: 0 }]),
    // Honours the signal, as the real providers do: an abort rejects it.
    selectReadOnly: vi.fn(
      (_id: string, _sql: string, signal?: AbortSignal) =>
        new Promise<Record<string, unknown>[]>((resolve, reject) => {
          const abort = () => reject(new DOMException("Aborted", "AbortError"));
          if (signal?.aborted) return abort();
          signal?.addEventListener("abort", abort, { once: true });
          if (!opts.hold) resolve([{ n: 1 }]);
        }),
    ),
  };
  const providers = { getForType: async () => provider } as unknown as ProviderRegistry;
  const crud = new QueryCrudManager(state, providers, {
    isEnabled: () => false,
  } as unknown as PendingChangesManager);
  const manager = new DashboardManager(
    state,
    (connectionId, sql, signal) => crud.executeReadOnly(connectionId, sql, signal),
    () => {},
  );
  const current = () => manager.getDashboard("d-1")!.widgets[0];
  return { manager, provider, current };
}

describe("DashboardManager.executeWidget", () => {
  it("runs a custom query read-only on the active connection", async () => {
    const { manager, provider, current } = setup(widget());
    await manager.executeWidget("d-1", "w-1");
    expect(provider.selectReadOnly).toHaveBeenCalledWith(
      "pc-1",
      "SELECT 1 AS n",
      expect.any(AbortSignal),
    );
    expect(provider.select).not.toHaveBeenCalled();
    expect(current().result).toEqual([{ n: 1 }]);
    expect(current().error).toBeUndefined();
  });

  it("runs a saved query read-only", async () => {
    const { manager, provider } = setup(widget({ querySource: "saved", savedQueryId: "sq-1" }));
    await manager.executeWidget("d-1", "w-1");
    expect(provider.selectReadOnly).toHaveBeenCalledWith(
      "pc-1",
      "SELECT count(*) AS n FROM saved",
      expect.any(AbortSignal),
    );
  });

  it("runs a query with date filters read-only", async () => {
    const { manager, provider } = setup(
      widget({ query: "SELECT 1 AS n WHERE d BETWEEN {{start_date}} AND {{end_date}}" }),
      { dateFilter: { start: "2026-01-01", end: "2026-02-01" } },
    );
    await manager.executeWidget("d-1", "w-1");
    expect(provider.selectReadOnly).toHaveBeenCalledWith(
      "pc-1",
      "SELECT 1 AS n WHERE d BETWEEN '2026-01-01' AND '2026-02-01'",
      expect.any(AbortSignal),
    );
  });

  it("shows the token check's refusal as the widget's error", async () => {
    const { manager, provider, current } = setup(widget({ query: "DELETE FROM t" }));
    await manager.executeWidget("d-1", "w-1");
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
    expect(current().error).toBe("Only read-only SELECT queries are permitted");
    expect(current().isLoading).toBe(false);
  });

  it("shows the database's read-only refusal as the widget's error", async () => {
    const { manager, provider, current } = setup(widget());
    provider.selectReadOnly.mockRejectedValueOnce(
      new Error("cannot execute INSERT in a read-only transaction"),
    );
    await manager.executeWidget("d-1", "w-1");
    expect(current().error).toBe("cannot execute INSERT in a read-only transaction");
  });

  it("shows an error without an active connection", async () => {
    const { manager, provider, current } = setup(widget(), { activeConnectionId: null });
    await manager.executeWidget("d-1", "w-1");
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
    expect(current().error).toBe("Not connected to database");
  });

  it("runs editor previews and version diffs read-only too", async () => {
    const { manager, provider } = setup(widget());
    expect(await manager.runWidgetQuery("SELECT 2 AS n")).toEqual([{ n: 1 }]);
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-1", "SELECT 2 AS n", undefined);
    await expect(manager.runWidgetQuery("DROP TABLE t")).rejects.toThrow(
      "Only read-only SELECT queries are permitted",
    );
  });
});

describe("widget runs: overlap, close and remove", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  const signalOf = (provider: ReturnType<typeof setup>["provider"], call: number) =>
    provider.selectReadOnly.mock.calls[call][2]!;

  it("auto-refresh skips a widget whose last run is still in flight", async () => {
    vi.useFakeTimers();
    const { manager, provider } = setup(widget({ autoRefreshSeconds: 1 }), { hold: true });
    manager.startAutoRefresh("d-1", "w-1");
    await vi.advanceTimersByTimeAsync(1000);
    expect(provider.selectReadOnly).toHaveBeenCalledOnce();
    // A blocked query (say on a row lock) doesn't queue another waiter per tick.
    await vi.advanceTimersByTimeAsync(5000);
    expect(provider.selectReadOnly).toHaveBeenCalledOnce();
    manager.stopAllAutoRefresh();
  });

  it("auto-refresh runs again once the last run finished", async () => {
    vi.useFakeTimers();
    const { manager, provider } = setup(widget({ autoRefreshSeconds: 1 }));
    manager.startAutoRefresh("d-1", "w-1");
    await vi.advanceTimersByTimeAsync(3000);
    expect(provider.selectReadOnly).toHaveBeenCalledTimes(3);
    manager.stopAllAutoRefresh();
  });

  it("closing the dashboard aborts its runs and stops its timers", async () => {
    vi.useFakeTimers();
    const { manager, provider, current } = setup(widget({ autoRefreshSeconds: 1 }), {
      hold: true,
    });
    manager.startAutoRefresh("d-1", "w-1");
    await vi.advanceTimersByTimeAsync(1000);
    expect(current().isLoading).toBe(true);

    manager.closeDashboard("d-1");
    await vi.advanceTimersByTimeAsync(0);
    expect(signalOf(provider, 0).aborted).toBe(true);
    expect(current().isLoading).toBe(false);
    expect(current().error).toBe("Query cancelled");

    await vi.advanceTimersByTimeAsync(5000);
    expect(provider.selectReadOnly).toHaveBeenCalledOnce();
  });

  it("removing a widget aborts its run", async () => {
    const { manager, provider } = setup(widget(), { hold: true });
    const run = manager.executeWidget("d-1", "w-1");
    await vi.waitFor(() => expect(provider.selectReadOnly).toHaveBeenCalledOnce());
    await manager.removeWidget("d-1", "w-1");
    await run;
    expect(signalOf(provider, 0).aborted).toBe(true);
    expect(manager.getDashboard("d-1")!.widgets).toEqual([]);
  });

  it("a manual run replaces the one in flight and keeps only its own result", async () => {
    const { manager, provider, current } = setup(widget(), { hold: true });
    const first = manager.executeWidget("d-1", "w-1");
    await vi.waitFor(() => expect(provider.selectReadOnly).toHaveBeenCalledOnce());
    provider.selectReadOnly.mockImplementationOnce(async () => [{ n: 2 }]);
    await manager.executeWidget("d-1", "w-1");
    await first;
    expect(signalOf(provider, 0).aborted).toBe(true);
    expect(current().result).toEqual([{ n: 2 }]);
    expect(current().error).toBeUndefined();
    expect(current().isLoading).toBe(false);
  });

  it("a caller's signal still cancels the run", async () => {
    const { manager, provider, current } = setup(widget(), { hold: true });
    const stop = new AbortController();
    const run = manager.executeWidget("d-1", "w-1", stop.signal);
    await vi.waitFor(() => expect(provider.selectReadOnly).toHaveBeenCalledOnce());
    stop.abort();
    await run;
    expect(signalOf(provider, 0).aborted).toBe(true);
    expect(current().error).toBe("Query cancelled");
  });
});
