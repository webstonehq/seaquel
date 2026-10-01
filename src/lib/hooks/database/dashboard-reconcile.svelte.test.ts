/**
 * The shared-dashboard reconcile on project activation (re-survey bug 9):
 * only dashboards whose stored form changes are saved, and one failed save
 * doesn't stop the others. Since 5d-2 a save is a `library` call: a new
 * file is `dashboardCreate` (Core's id) marked shared, a changed one a
 * patch.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Dashboard, SharedDashboard } from "$lib/types";

const saves: string[] = [];
let failing = new Set<string>();
vi.mock("@tauri-apps/plugin-fs", () => ({}));
vi.mock("@tauri-apps/api/path", () => ({}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { DatabaseState } = await import("./state.svelte.js");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { SharedDashboardManager } = await import("./shared-dashboard-manager.svelte.js");
const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { RecordingLibrary } = await import("./library/recording-library");
const { setLibrary, LibraryCallError } = await import("./library/index");
import type { DashboardDraft, DashboardPatch } from "./library/types";

/** The library: every dashboard write counts as a save of its name. */
class SavingLibrary extends RecordingLibrary {
  override async createDashboard(draft: DashboardDraft) {
    if (failing.has(draft.name)) throw new LibraryCallError("STORAGE_ERROR", "disk full");
    saves.push(draft.name);
    return super.createDashboard(draft);
  }
  override async updateDashboard(id: string, patch: DashboardPatch) {
    const name = this.dashboards.get(id)?.name ?? id;
    if (failing.has(name)) throw new LibraryCallError("STORAGE_ERROR", "disk full");
    saves.push(name);
    return super.updateDashboard(id, patch);
  }
}

const BASE = ".seaquel/projects/proj/dashboards";
const viewport = { x: 0, y: 0, zoom: 1 };
const widget = (id: string, title: string) =>
  ({ id, type: "chart", title, query: "SELECT 1", position: { x: 0, y: 0, w: 1, h: 1 } }) as never;
const at = new Date("2026-01-01T00:00:00.000Z");

function dashboard(
  id: string,
  name: string,
  shared: boolean,
  widgets = [widget("w", "A")],
): Dashboard {
  return {
    id,
    name,
    projectId: "p",
    widgets,
    viewport,
    dateFilter: null,
    createdAt: at,
    updatedAt: at,
    shared,
    starred: false,
  } as Dashboard;
}

function gitFile(name: string, widgets = [widget("w", "A")]): SharedDashboard {
  return {
    id: `r:${BASE}/${name}.json`,
    repoId: "r",
    filePath: `${BASE}/${name.toLowerCase()}.json`,
    name,
    widgets,
    viewport,
    dateFilter: null,
    updatedAt: new Date("2026-02-01T00:00:00.000Z"),
  } as SharedDashboard;
}

function setup() {
  const state = new DatabaseState();
  state.projects = [{ id: "p", name: "proj", createdAt: at, updatedAt: at, customLabels: [] }];
  state.activeProjectId = "p";
  state.activeRepoId = "r";
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
  const shared = new SharedDashboardManager(state, {} as never);
  projects.setSharedDashboardManager(shared);
  return { state, projects, shared };
}

let library: SavingLibrary;

/** The page's dashboards, stored as the library's rows too. */
function seed(state: InstanceType<typeof DatabaseState>, list: Dashboard[]) {
  state.dashboardsByProject = { p: list };
  for (const d of list) {
    library.dashboards.set(d.id, {
      id: d.id,
      projectId: "p",
      name: d.name,
      viewport: JSON.stringify(d.viewport),
      widgets: JSON.stringify(d.widgets),
      dateFilter: null,
      starred: false,
      shared: d.shared,
      createdAt: at.toISOString(),
      updatedAt: at.toISOString(),
    });
  }
}

beforeEach(() => {
  saves.length = 0;
  failing = new Set();
  library = new SavingLibrary();
  library.seedProject("p");
  setLibrary(library);
});

describe("the shared-dashboard reconcile", () => {
  it("the reconcile saves only changed dashboards", async () => {
    const { state, projects } = setup();
    seed(state, [
      dashboard("d1", "Sales", true),
      dashboard("d2", "Ops", true),
      dashboard("d3", "Local", false),
      dashboard("d4", "Gone", true),
    ]);
    state.sharedDashboardsByRepo = {
      r: [gitFile("Sales"), gitFile("Ops", [widget("w", "B")]), gitFile("New")],
    };

    await projects.reconcileGitState("p");

    expect(saves.sort()).toEqual(["Gone", "New", "Ops"]);
    const byName = Object.fromEntries(state.dashboardsByProject.p.map((d) => [d.name, d]));
    expect(byName.Gone.shared).toBe(false);
    expect((byName.Ops.widgets[0] as { title: string }).title).toBe("B");
    // Unchanged content keeps its row as it was, time included.
    expect(byName.Sales.updatedAt).toEqual(at);
    // The new file's dashboard has Core's id, shared.
    expect(byName.New.id).toMatch(/^dashboard-\d+$/);
    expect(library.dashboards.get(byName.New.id)?.shared).toBe(true);

    // A second activation finds nothing to change and saves nothing.
    saves.length = 0;
    await projects.reconcileGitState("p");
    expect(saves).toEqual([]);
  });

  it("returns the same list when nothing changed", () => {
    const { state, shared } = setup();
    const list = [dashboard("d1", "Sales", true), dashboard("d3", "Local", false)];
    state.sharedDashboardsByRepo = { r: [gitFile("Sales")] };
    expect(shared.reconcileWithGitFiles("p", list)).toBe(list);
  });

  it("a failed save doesn't stop the rest", async () => {
    failing = new Set(["Ops"]);
    const { state, projects } = setup();
    seed(state, [dashboard("d2", "Ops", true), dashboard("d4", "Gone", true)]);
    state.sharedDashboardsByRepo = { r: [gitFile("Ops", [widget("w", "B")]), gitFile("New")] };

    await projects.reconcileGitState("p");

    expect(saves.sort()).toEqual(["Gone", "New"]);
  });
});
