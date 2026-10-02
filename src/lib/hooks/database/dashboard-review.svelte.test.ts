/**
 * Dashboards through Core, against the demo's `TsLibrary` over sql.js
 * (Core's rules): new dashboards take the next free name, a refused edit
 * is taken back (fields and the tab's name) (5d-2 Task 6b review: I1, I7,
 * I8, M7, M8). The git reconcile and the shared file moved to Core in phase
 * 5e: its tests are Core's (`seaquel-core/tests/shared.rs`).
 */
import initSqlJs from "sql.js";
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

const toasts = vi.hoisted(() => [] as string[]);
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("svelte-sonner", () => ({
  toast: {
    success: vi.fn(),
    info: (m: string) => toasts.push(m),
    warning: (m: string) => toasts.push(m),
  },
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { bootstrapSqljsDatabase } = await import("$lib/storage/sqljs-client");
const { WebSqliteDatabase } = await import("$lib/storage/web-sqlite");
const { projectsRepo } = await import("$lib/storage/repository");
const { DatabaseState } = await import("./state.svelte.js");
const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { TsLibrary } = await import("./library/ts-library");
const { setLibrary, LibraryCallError } = await import("./library/index");

let SQL: Awaited<ReturnType<typeof initSqlJs>>;
let library: InstanceType<typeof TsLibrary>;

beforeAll(async () => {
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
  SQL = await initSqlJs();
});

afterAll(() => {
  setLibrary(null);
  vi.unstubAllGlobals();
});

beforeEach(async () => {
  toasts.length = 0;
  const db = new WebSqliteDatabase(new SQL.Database());
  await bootstrapSqljsDatabase(db);
  const now = new Date().toISOString();
  await projectsRepo.save(db, {
    id: "p",
    name: "proj",
    createdAt: now,
    updatedAt: now,
    customLabels: [],
  });
  library = new TsLibrary(db);
  setLibrary(library);
});

function setup() {
  const state = new DatabaseState();
  state.projects = [
    { id: "p", name: "proj", createdAt: new Date(), updatedAt: new Date(), customLabels: [] },
  ];
  state.activeProjectId = "p";
  state.dashboardsByProject = { p: [] };
  const dashboards = new DashboardManager(
    state,
    async () => [],
    () => {},
  );
  const projects = new ProjectManager(
    state,
    new WindowStateManager(state, { enabled: false }),
    new StateRestorationManager(state),
  );
  return { state, dashboards, projects };
}

const stored = async () => (await library.listDashboards("p")).value;

describe("new dashboards", () => {
  it("a second New Dashboard takes the next free name", async () => {
    const { dashboards } = setup();
    const first = await dashboards.createDashboard("New Dashboard", { renameIfTaken: true });
    const second = await dashboards.createDashboard("New Dashboard", { renameIfTaken: true });
    expect([first?.name, second?.name]).toEqual(["New Dashboard", "New Dashboard (2)"]);
    // A name the user typed is still refused, and said.
    expect(await dashboards.createDashboard("new dashboard")).toBeNull();
    expect(toasts).toHaveLength(1);
  });
});

describe("a refused edit", () => {
  it("is taken back, the tab's name too", async () => {
    const { state, dashboards } = setup();
    const sales = (await dashboards.createDashboard("Sales"))!;
    await dashboards.createDashboard("Ops");
    state.dashboardTabsByProject = { p: [{ id: "t1", name: "Sales", dashboardId: sales.id }] };

    // The header renames the tab, then the dashboard: Core refuses the name.
    state.dashboardTabsByProject = { p: [{ id: "t1", name: "Ops", dashboardId: sales.id }] };
    expect(await dashboards.renameDashboard(sales.id, "Ops")).toBe(false);

    expect(dashboards.getDashboard(sales.id)?.name).toBe("Sales");
    expect(state.dashboardTabsByProject.p[0].name).toBe("Sales");
    expect(toasts).toHaveLength(1);
  });

  it("a refused widget edit is taken back, and a later edit saves only its own change", async () => {
    const { dashboards } = setup();
    const d = (await dashboards.createDashboard("Sales"))!;
    const widget = { id: "w1", type: "chart", title: "A", query: "SELECT 1" } as never;
    vi.spyOn(library, "updateDashboard").mockRejectedValueOnce(
      new LibraryCallError("STORAGE_FULL", "full"),
    );

    await dashboards.addWidget(d.id, widget);
    expect(dashboards.getDashboard(d.id)?.widgets).toEqual([]);

    await dashboards.updateViewport(d.id, { x: 5, y: 5, zoom: 1 });
    const row = (await stored()).find((x) => x.id === d.id)!;
    expect(JSON.parse(row.widgets)).toEqual([]);
    expect(JSON.parse(row.viewport)).toEqual({ x: 5, y: 5, zoom: 1 });
  });
});

describe("two refused edits in flight", () => {
  it("two refused widget adds end on the stored widgets", async () => {
    const { dashboards } = setup();
    const d = (await dashboards.createDashboard("Sales"))!;
    vi.spyOn(library, "updateDashboard").mockRejectedValue(
      new LibraryCallError("STORAGE_FULL", "full"),
    );
    const w = (id: string) => ({ id, type: "chart", title: id, query: "SELECT 1" }) as never;

    await Promise.all([dashboards.addWidget(d.id, w("w1")), dashboards.addWidget(d.id, w("w2"))]);

    expect(dashboards.getDashboard(d.id)?.widgets).toEqual([]);
  });

  it("two refused renames end on the stored name, the tab's too", async () => {
    const { state, dashboards } = setup();
    const d = (await dashboards.createDashboard("Sales"))!;
    state.dashboardTabsByProject = { p: [{ id: "t1", name: "Sales", dashboardId: d.id }] };
    vi.spyOn(library, "updateDashboard").mockRejectedValue(
      new LibraryCallError("STORAGE_FULL", "full"),
    );

    await Promise.all([
      dashboards.renameDashboard(d.id, "A"),
      dashboards.renameDashboard(d.id, "B"),
    ]);

    expect(dashboards.getDashboard(d.id)?.name).toBe("Sales");
    expect(state.dashboardTabsByProject.p[0].name).toBe("Sales");
  });
});

describe("after a refused edit", () => {
  it("an edit made right after it isn't undone by the restore's read", async () => {
    const { dashboards } = setup();
    const d = (await dashboards.createDashboard("Sales"))!;
    const list = library.listDashboards.bind(library);
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    // The restore's read is taken now, but answers only later.
    const reads = vi.spyOn(library, "listDashboards").mockImplementationOnce(async (projectId) => {
      const answer = await list(projectId);
      await held;
      return answer;
    });
    vi.spyOn(library, "updateDashboard").mockRejectedValueOnce(
      new LibraryCallError("STORAGE_FULL", "full"),
    );

    const refused = dashboards.updateViewport(d.id, { x: 1, y: 1, zoom: 1 });
    await vi.waitFor(() => expect(reads).toHaveBeenCalled());
    await dashboards.updateViewport(d.id, { x: 7, y: 7, zoom: 1 });
    release();
    await refused;

    expect(dashboards.getDashboard(d.id)?.viewport).toEqual({ x: 7, y: 7, zoom: 1 });
  });
});
