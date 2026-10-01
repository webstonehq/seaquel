/**
 * Dashboards through Core, against the demo's `TsLibrary` over sql.js
 * (Core's rules): new dashboards take the next free name, a refused edit
 * is taken back (fields and the tab's name), the git reconcile stores a
 * file's dashboard in one call and never shows a placeholder, and a
 * failed shared-file delete is said (5d-2 Task 6b review: I1, I7, I8, M7,
 * M8).
 */
import initSqlJs from "sql.js";
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { Dashboard, SharedDashboard } from "$lib/types";

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
vi.mock("@tauri-apps/plugin-fs", () => ({}));
vi.mock("@tauri-apps/api/path", () => ({}));

const { bootstrapSqljsDatabase } = await import("$lib/storage/sqljs-client");
const { WebSqliteDatabase } = await import("$lib/storage/web-sqlite");
const { projectsRepo } = await import("$lib/storage/repository");
const { DatabaseState } = await import("./state.svelte.js");
const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { SharedDashboardManager } = await import("./shared-dashboard-manager.svelte.js");
const { TsLibrary } = await import("./library/ts-library");
const { dashboardNameToFilename } = await import("$lib/services/dashboard-file-parser");
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
  state.activeRepoId = "r";
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
    dashboards,
  );
  projects.setSharedDashboardManager(new SharedDashboardManager(state, {} as never));
  return { state, dashboards, projects };
}

const stored = async () => (await library.listDashboards("p")).value;

const BASE = ".seaquel/projects/proj/dashboards";
function gitFile(name: string): SharedDashboard {
  return {
    id: `r:${BASE}/${name}.json`,
    repoId: "r",
    filePath: `${BASE}/${dashboardNameToFilename(name)}`,
    name,
    widgets: [],
    viewport: { x: 0, y: 0, zoom: 1 },
    dateFilter: null,
    updatedAt: new Date("2026-02-01T00:00:00.000Z"),
  } as SharedDashboard;
}

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

describe("the git reconcile", () => {
  it("a file whose name a local dashboard has is skipped and said once; its neighbour is stored once", async () => {
    const { state, dashboards, projects } = setup();
    await dashboards.createDashboard("X");
    state.sharedDashboardsByRepo = { r: [gitFile("X"), gitFile("X (2)")] };

    await projects.reconcileGitState("p");
    await projects.reconcileGitState("p");

    const rows = await stored();
    expect(rows.map((r) => [r.name, r.shared])).toEqual([
      ["X", false],
      ["X (2)", true],
    ]);
    expect(state.dashboardsByProject.p.map((d) => d.name)).toEqual(["X", "X (2)"]);
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toContain('"X"');
  });

  it("a user's own unshared dashboard is never overwritten by a file of its name", async () => {
    const { state, dashboards, projects } = setup();
    const own = (await dashboards.createDashboard("X (2)"))!;
    const widget = { id: "mine", type: "chart", title: "Mine", query: "SELECT 1" } as never;
    await dashboards.addWidget(own.id, widget);
    state.sharedDashboardsByRepo = {
      r: [{ ...gitFile("X (2)"), widgets: [{ id: "theirs" }] } as never],
    };

    await projects.reconcileGitState("p");

    const row = (await stored()).find((r) => r.id === own.id)!;
    expect([row.name, row.shared]).toEqual(["X (2)", false]);
    expect(JSON.parse(row.widgets).map((w: { id: string }) => w.id)).toEqual(["mine"]);
    expect(await stored()).toHaveLength(1);
  });

  it("deleting and unsharing a shared dashboard touch its own file", async () => {
    const { dashboards } = setup();
    const files: string[] = [];
    dashboards.setFileProjection({
      writeDashboardFile: async (d) => void files.push(`write ${d.name}`),
      deleteDashboardFile: async (d) => void files.push(`delete ${d.name}`),
    });
    const x = (await dashboards.createDashboard("X"))!;
    const y = (await dashboards.createDashboard("Y"))!;
    await dashboards.shareDashboardById(x.id);
    await dashboards.shareDashboardById(y.id);
    await dashboards.unshareDashboardById(y.id);
    await dashboards.deleteDashboard(x.id);
    expect(files).toEqual(["write X", "write Y", "delete Y", "delete X"]);
  });

  it("a case-only rename of a shared dashboard stays shared and matched", async () => {
    const { state, dashboards, projects } = setup();
    const d = (await dashboards.createDashboard("Sales"))!;
    await dashboards.shareDashboardById(d.id);
    await dashboards.renameDashboard(d.id, "SALES");
    state.sharedDashboardsByRepo = { r: [gitFile("Sales")] };

    await projects.reconcileGitState("p");

    expect((await stored()).map((r) => [r.name, r.shared])).toEqual([["SALES", true]]);
    expect(toasts).toEqual([]);
  });

  it("a file whose name differs only in case stays matched", async () => {
    const { state, dashboards, projects } = setup();
    const d = (await dashboards.createDashboard("Sales"))!;
    await dashboards.shareDashboardById(d.id);
    state.sharedDashboardsByRepo = { r: [{ ...gitFile("Sales"), name: "sales" }] };

    await projects.reconcileGitState("p");

    expect((await stored()).map((r) => [r.name, r.shared])).toEqual([["Sales", true]]);
    expect(toasts).toEqual([]);
  });

  it("two shared dashboards whose names slug to one file keep their own content", async () => {
    const { state, dashboards, projects } = setup();
    const a = (await dashboards.createDashboard("Отчёт"))!;
    const b = (await dashboards.createDashboard("Продажи"))!;
    await dashboards.shareDashboardById(a.id);
    await dashboards.shareDashboardById(b.id);
    // Both names slug to `untitled.json`; each file names its dashboard.
    const file = (name: string, widget: string) =>
      ({ ...gitFile(name), filePath: `${BASE}/untitled.json`, widgets: [{ id: widget }] }) as never;
    // Listed in the other order than the dashboards were made.
    state.sharedDashboardsByRepo = { r: [file("Продажи", "wb"), file("Отчёт", "wa")] };

    await projects.reconcileGitState("p");

    const rows = new Map((await stored()).map((r) => [r.name, r]));
    expect(rows.get("Отчёт")?.shared).toBe(true);
    expect(rows.get("Продажи")?.shared).toBe(true);
    expect(JSON.parse(rows.get("Отчёт")!.widgets)).toEqual([{ id: "wa" }]);
    expect(JSON.parse(rows.get("Продажи")!.widgets)).toEqual([{ id: "wb" }]);
    expect(rows.size).toBe(2);
  });

  it("never shows a placeholder, and a create that fails leaves nothing", async () => {
    const { state, projects } = setup();
    state.sharedDashboardsByRepo = { r: [gitFile("Board"), gitFile("Fails")] };
    const create = library.createDashboard.bind(library);
    let seen: Dashboard[] = [];
    vi.spyOn(library, "createDashboard").mockImplementation(async (draft) => {
      // Whatever the page shows while a create is on its way.
      seen = [...state.dashboardsByProject.p];
      if (draft.name === "Fails") throw new LibraryCallError("STORAGE_FULL", "full");
      return create(draft);
    });

    await projects.reconcileGitState("p");

    expect(seen.every((d) => !d.id.startsWith("file:"))).toBe(true);
    expect(state.dashboardsByProject.p.map((d) => [d.name, d.shared])).toEqual([["Board", true]]);
    expect((await stored()).map((r) => r.name)).toEqual(["Board"]);
  });
});

describe("deleting a shared dashboard", () => {
  it("a failed file delete is said, and the dashboard is still removed", async () => {
    const { dashboards } = setup();
    const d = (await dashboards.createDashboard("Sales"))!;
    await dashboards.shareDashboardById(d.id);
    dashboards.setFileProjection({
      writeDashboardFile: async () => {},
      deleteDashboardFile: async () => {
        throw new Error("EACCES");
      },
    });

    await expect(dashboards.deleteDashboard(d.id)).resolves.toBeUndefined();

    expect(dashboards.getDashboard(d.id)).toBeUndefined();
    expect(await stored()).toEqual([]);
    expect(toasts).toEqual([expect.stringContaining("EACCES")]);
  });
});
