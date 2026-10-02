/**
 * The demo's sample dashboard (phase 8 Task 1, bug 1). It is created on the
 * first load only. Every reload used to create it again, which Core's name
 * check refuses ("There's already a dashboard called …"), so each reload
 * showed that error toast; before 5d-2's check, each reload stored another
 * copy. Against Core in the browser module (phase 8), as the demo runs it.
 */
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { loadTestModule, testModuleMissing, type TestModule } from "$lib/core/browser/testing/node";
import { openModuleCore } from "$lib/core/browser/testing/meta";

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

const { DatabaseState } = await import("$lib/hooks/database/state.svelte.js");
const { DashboardManager } = await import("$lib/hooks/database/dashboard-manager.svelte.js");
const { StateRestorationManager } = await import("$lib/hooks/database/state-restoration.svelte.js");
const { CoreLibrary, setLibrary } = await import("$lib/hooks/database/library/index");
const { createDemoDashboard, DEMO_DASHBOARD_NAME } = await import("./sample-dashboard");

const missing = testModuleMissing();
let module: TestModule | null = null;
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
  const core = await openModuleCore(module);
  const now = new Date().toISOString();
  await core.execute(
    "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'proj', ?, ?)",
    [now, now],
  );
  const storage = core.storage();
  library = new CoreLibrary(() => storage);
  setLibrary(library);
});

/** One page load: what `createDemoDashboard` uses of `UseDatabase`. */
async function load() {
  const state = new DatabaseState();
  state.projects = [
    { id: "p", name: "proj", createdAt: new Date(), updatedAt: new Date(), customLabels: [] },
  ];
  state.activeProjectId = "p";
  state.activeConnectionIdByProject = { p: "demo-connection" };
  // The project's dashboards, as `projects.initialize` loads them.
  await new StateRestorationManager(state).loadDashboards("p");
  const runs: string[] = [];
  const dashboards = new DashboardManager(
    state,
    async (_connectionId, sql) => {
      runs.push(sql);
      return [];
    },
    () => {},
  );
  const opened: (string | undefined)[] = [];
  const db = {
    state,
    dashboards,
    dashboardTabs: {
      add: (dashboardId?: string) => {
        opened.push(dashboardId);
        return "tab";
      },
    },
  };
  return { db, opened, runs };
}

const stored = async () => (await library.listDashboards("p")).value;

describe.skipIf(missing)("the sample dashboard", () => {
  it("is created and opened on the first load", async () => {
    const { db, opened } = await load();
    await createDemoDashboard(db);
    const rows = await stored();
    expect(rows.map((d) => d.name)).toEqual([DEMO_DASHBOARD_NAME]);
    expect(opened).toEqual([rows[0].id]);
    expect(toasts).toEqual([]);
  });

  it("isn't created twice", async () => {
    const firstLoad = await load();
    await createDemoDashboard(firstLoad.db);
    const [first] = await stored();
    expect(firstLoad.runs.length).toBeGreaterThan(0);

    // A reload with the dashboard stored: no create, no toast, and no tab
    // opened (the reload restored the tabs the visitor had).
    const reload = await load();
    await createDemoDashboard(reload.db);
    expect((await stored()).map((d) => d.id)).toEqual([first.id]);
    expect(toasts).toEqual([]);
    expect(reload.opened).toEqual([]);
    // Its widgets run again: their rows aren't stored, and the sample
    // tables were seeded again.
    expect(reload.runs).toEqual(firstLoad.runs);
  });

  it("renamed by the visitor, is created once more under its name, and then not again", async () => {
    await createDemoDashboard((await load()).db);
    const [first] = await stored();
    const renamed = await load();
    await renamed.db.dashboards.renameDashboard(first.id, "My shop");

    await createDemoDashboard((await load()).db);
    await createDemoDashboard((await load()).db);
    expect((await stored()).map((d) => d.name).sort()).toEqual([DEMO_DASHBOARD_NAME, "My shop"]);
    expect(toasts).toEqual([]);
  });

  it("renamed only in case or spacing, is found as Core's name check finds it", async () => {
    await createDemoDashboard((await load()).db);
    const [first] = await stored();
    const renamed = await load();
    await renamed.db.dashboards.renameDashboard(first.id, " e-commerce overview ");

    // Core's check would refuse the sample's name (`NAME_TAKEN`): no create, no toast.
    const reload = await load();
    await createDemoDashboard(reload.db);
    expect((await stored()).map((d) => d.id)).toEqual([first.id]);
    expect(toasts).toEqual([]);
    expect(reload.runs.length).toBeGreaterThan(0);
  });
});
