/**
 * The demo's start on a reload (phase 8 Task 1, bug 2). The page restores
 * the project's tabs before the demo connects (`projects.initialize`, awaited
 * by `whenReady`), so `addDemoConnection` must not open a query tab of its
 * own then: a reload used to add "Query 2", then "Query 3", and make it the
 * active tab. A first load, with no query tab, still gets one, as a connect
 * on desktop and web does. The row is Core's (`ensureDemoConnection`, the
 * browser module's test build), as the demo's start hands it over.
 */
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderRegistry } from "$lib/providers";
import type { QueryTab } from "$lib/types";
import { loadTestModule, testModuleMissing, type TestModule } from "$lib/core/browser/testing/node";
import { openModuleCore } from "$lib/core/browser/testing/meta";
import type { WindowStateManager } from "./window-state.svelte.js";
import type { StateRestorationManager } from "./state-restoration.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import type { ChangeSeq, WireConnection } from "./library/index.js";

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => false,
  isDemo: () => true,
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: async () => [] }),
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));
vi.mock("$lib/utils/toast", () => ({ errorToast: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast: { success: vi.fn(), info: vi.fn() } }));

const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { CoreLibrary, setLibrary } = await import("./library/index");

const missing = testModuleMissing();
let module: TestModule | null = null;

beforeAll(async () => {
  module = await loadTestModule();
});

afterAll(() => {
  setLibrary(null);
});

/** Core's answer to the demo's start (`ensureDemoConnection`), as the page gets it. */
let stored: () => Promise<{ value: WireConnection; seq: ChangeSeq }>;

beforeEach(async () => {
  if (!module) return;
  const core = await openModuleCore(module);
  const now = new Date().toISOString();
  await core.execute(
    "INSERT INTO projects (id, name, created_at, updated_at) VALUES ('p', 'proj', ?, ?)",
    [now, now],
  );
  const storage = core.storage();
  setLibrary(new CoreLibrary(() => storage));
  stored = async () => {
    await core.query("SELECT 1"); // the seed above, applied
    return JSON.parse(await module!.ensureDemoConnection()) as {
      value: WireConnection;
      seq: ChangeSeq;
    };
  };
});

/** One page load: the managers, with the project's tabs as restored. */
function load(restored: QueryTab[]) {
  const state = new DatabaseState();
  state.projects = [
    { id: "p", name: "proj", createdAt: new Date(), updatedAt: new Date(), customLabels: [] },
  ];
  state.activeProjectId = "p";
  state.queryTabsByProject = { p: restored };
  state.activeQueryTabIdByProject = { p: restored[0]?.id ?? null };
  let next = restored.length + 1;
  // What `UseDatabase` passes: `queryTabs.add()` (a tab, made active).
  const createInitialTab = () => {
    const tab: QueryTab = {
      id: `tab-${next}`,
      name: `Query ${next++}`,
      query: "",
      isExecuting: false,
    };
    state.queryTabsByProject = {
      ...state.queryTabsByProject,
      p: [...state.queryTabsByProject.p, tab],
    };
    state.activeQueryTabIdByProject = { ...state.activeQueryTabIdByProject, p: tab.id };
  };
  const manager = new ConnectionManager(
    state,
    { scheduleProject: vi.fn() } as unknown as WindowStateManager,
    {
      initializeConnectionMaps: vi.fn(),
      ensureConnectionMapsExist: vi.fn(),
      loadConnectionData: vi.fn(async () => {}),
    } as unknown as StateRestorationManager,
    {} as TabOrderingManager,
    {} as ProviderRegistry,
    vi.fn(async () => {}),
    createInitialTab,
  );
  return { state, manager };
}

const names = (state: InstanceType<typeof DatabaseState>) =>
  state.queryTabsByProject.p.map((t) => t.name);

describe.skipIf(missing)("the demo connection on a reload", () => {
  it("a first load opens one query tab", async () => {
    const { state, manager } = load([]);
    await manager.addDemoConnection(await stored(), "duck-1");
    expect(names(state)).toEqual(["Query 1"]);
  });

  it("a reload restores the tabs it had and opens no new one", async () => {
    // First load.
    const first = load([]);
    await first.manager.addDemoConnection(await stored(), "duck-1");
    const tabs = first.state.queryTabsByProject.p;

    // The reload: the same tabs come back from the window's view state.
    const { state, manager } = load(tabs.map((t) => ({ ...t })));
    await manager.addDemoConnection(await stored(), "duck-2");
    expect(names(state)).toEqual(["Query 1"]);
    expect(state.activeQueryTabIdByProject.p).toBe(tabs[0].id);
    // The connection is still the active one, connected on the new DuckDB id.
    expect(state.activeConnectionIdByProject.p).toBe("demo-connection");
    expect(state.connections.find((c) => c.id === "demo-connection")?.providerConnectionId).toBe(
      "duck-2",
    );
  });
});
