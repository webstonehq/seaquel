/**
 * Dashboards through Core, against the browser module (phase 8: Core in
 * the demo's page): new dashboards take the next free name, a refused edit
 * is taken back (fields and the tab's name) (5d-2 Task 6b review: I1, I7,
 * I8, M7, M8). The git reconcile and the shared file moved to Core in phase
 * 5e: its tests are Core's (`seaquel-core/tests/shared.rs`).
 */
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { loadTestModule, testModuleMissing, type TestModule } from "$lib/core/browser/testing/node";
import { openModuleCore, type ModuleCore } from "$lib/core/browser/testing/meta";

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

const { DatabaseState } = await import("./state.svelte.js");
const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { CoreLibrary, setLibrary, LibraryCallError } = await import("./library/index");

const missing = testModuleMissing();
let module: TestModule | null = null;
let core: ModuleCore;
let library: InstanceType<typeof CoreLibrary>;

beforeAll(async () => {
  module = await loadTestModule();
});

afterAll(() => {
  setLibrary(null);
});

beforeEach(async () => {
  toasts.length = 0;
  if (!module) return;
  core = await openModuleCore(module);
  const now = new Date().toISOString();
  await core.execute(
    "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'proj', ?, ?)",
    [now, now],
  );
  const storage = core.storage();
  library = new CoreLibrary(() => storage);
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

describe.skipIf(missing)("new dashboards", () => {
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

describe.skipIf(missing)("a refused edit", () => {
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

describe.skipIf(missing)("two refused edits in flight", () => {
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

describe.skipIf(missing)("after a refused edit", () => {
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
