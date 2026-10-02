/**
 * The library against a real metadata database: the demo's `TsLibrary`
 * over sql.js, which keeps Core's rules (phase 5d-1), with the view models
 * on top. Two sets of managers on one database stand in for two web tabs.
 * - Every library change is one targeted call, written at once: no
 *   project save deletes another tab's saved query or undoes its label.
 * - Removing a custom label strips it from the connections that had it.
 * - Dashboards (stars, the version limit, failures) go through the library
 *   too since phase 5d-2: Core numbers and prunes their versions.
 */
import initSqlJs from "sql.js";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { StorageClient } from "$lib/storage/client";

const storage = vi.hoisted(() => ({ client: null as unknown }));
vi.mock("$lib/storage", () => ({ getStorage: () => storage.client }));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({ isAvailable: () => false }),
}));
const toasts: string[] = [];
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

const { bootstrapSqljsDatabase, createSqljsStorageClient } =
  await import("$lib/storage/sqljs-client");
const { WebSqliteDatabase } = await import("$lib/storage/web-sqlite");
const { projectsRepo } = await import("$lib/storage/repository");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { TsUi } = await import("./library/ts-ui");
const { DatabaseState } = await import("./state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { SavedQueryManager } = await import("./saved-queries.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { TsLibrary } = await import("./library/ts-library");
const { setLibrary, LibraryCallError } = await import("./library/index");
const { setShared, NoShared } = await import("./shared/index");

let SQL: Awaited<ReturnType<typeof initSqlJs>>;

beforeAll(async () => {
  const store = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => void store.set(k, v),
    removeItem: (k: string) => void store.delete(k),
  });
  SQL = await initSqlJs();
});

afterAll(() => vi.unstubAllGlobals());
afterEach(() => setLibrary(null));

let client: StorageClient;
let library: InstanceType<typeof TsLibrary>;
let database: InstanceType<typeof WebSqliteDatabase>;

beforeEach(async () => {
  toasts.length = 0;
  const db = new WebSqliteDatabase(new SQL.Database());
  database = db;
  await bootstrapSqljsDatabase(db);
  client = createSqljsStorageClient(db);
  storage.client = client;
  library = new TsLibrary(db);
  setLibrary(library);
  const now = new Date().toISOString();
  for (const id of ["p1", "p2"]) {
    await projectsRepo.save(db, { id, name: id, createdAt: now, updatedAt: now, customLabels: [] });
  }
});

/** One window or web tab: its own in-memory state over the shared database. */
async function openTab(projectId = "p1", windowId = `win-${crypto.randomUUID()}`) {
  const state = new DatabaseState();
  const ui = new TsUi(database, { origin: () => windowId });
  const windowState = new WindowStateManager(state, { ui: () => ui, windowId: () => windowId });
  const restoration = new StateRestorationManager(state);
  const projects = new ProjectManager(state, windowState, restoration);
  const provider = { connect: vi.fn(async () => "core-1"), disconnect: vi.fn(async () => {}) };
  const connections = new ConnectionManager(
    state,
    windowState,
    restoration,
    {} as never,
    { getForType: async () => provider } as never,
    async () => {},
    () => {},
  );
  projects.setConnectionManager(connections);
  await projects.initialize();
  await connections.initializePersistedConnections();
  await projects.setActive(projectId);
  const savedQueries = new SavedQueryManager(state, () => {});
  return {
    state,
    windowState,
    restoration,
    projects,
    savedQueries,
    connections,
    provider,
  };
}

const storedQueryNames = async (projectId = "p1") =>
  (await library.listSavedQueries(projectId)).value.map((q) => q.name).sort();

/** A stored connection in `projectId`, made through the library. */
async function storedConnection(
  projectId: string,
  name: string,
  extra: Record<string, unknown> = {},
): Promise<string> {
  const { value } = await library.createConnection({
    projectId,
    name,
    type: "postgres",
    host: "localhost",
    port: 5432,
    databaseName: "app",
    username: "me",
    ...extra,
  });
  return value.id;
}

describe("two tabs on one database", () => {
  it("a saved query is written at once", async () => {
    const a = await openTab();
    await a.savedQueries.saveQuery("mine", "SELECT 1");
    expect(await storedQueryNames()).toEqual(["mine"]);
  });

  it("two tabs' new saved queries both stay, whatever either saves later", async () => {
    const a = await openTab();
    const b = await openTab();
    await a.savedQueries.saveQuery("from A", "SELECT 1");
    await b.savedQueries.saveQuery("from B", "SELECT 2");
    // Tab B saves its project for an unrelated reason (a tab switch, say).
    await b.windowState.saveNow("p1");

    expect(await storedQueryNames()).toEqual(["from A", "from B"]);
  });

  it("a tab's own delete reaches storage", async () => {
    const a = await openTab();
    const id = (await a.savedQueries.saveQuery("mine", "SELECT 1"))!;
    await a.savedQueries.deleteQuery(id);
    expect(await storedQueryNames()).toEqual([]);
  });

  it("a stale tab's project edit doesn't undo another tab's label", async () => {
    const a = await openTab();
    const b = await openTab();
    await a.projects.addCustomLabel("p1", { name: "Mine", color: "#ff0000" });
    await b.projects.update("p2", { name: "Renamed" });

    const stored = (await library.listProjects()).value;
    expect(stored.find((p) => p.id === "p1")?.customLabels.map((l) => l.name)).toEqual(["Mine"]);
    expect(stored.find((p) => p.id === "p2")?.name).toBe("Renamed");
  });

  it("a name another saved query has is refused with its name, and nothing is saved", async () => {
    const a = await openTab();
    await a.savedQueries.saveQuery("Orders", "SELECT 1");
    await expect(a.savedQueries.saveQuery(" ORDERS ", "SELECT 2")).rejects.toThrow(
      'There\'s already a saved query called "Orders" in this folder.',
    );
    expect(await storedQueryNames()).toEqual(["Orders"]);
    expect(a.state.queriesByProject.p1.map((q) => q.name)).toEqual(["Orders"]);
  });
});

describe("saved query versions", () => {
  it("a changed text adds a keyframe of the previous text; an unchanged one adds none", async () => {
    const tab = await openTab();
    tab.state.queryTabsByProject = { p1: [{ id: "t1", name: "Q", query: "" } as never] };
    const id = (await tab.savedQueries.saveQuery("Q", "SELECT 1", "t1"))!;
    await tab.savedQueries.saveQuery("Q", "SELECT 2", "t1");
    await tab.savedQueries.saveQuery("Q", "SELECT 2", "t1");

    const versions = (await library.listQueryVersions("p1")).value;
    expect(versions.map((v) => [v.version, v.snapshot, v.diff])).toEqual([[1, "SELECT 1", null]]);
    expect(tab.savedQueries.getResolvedVersionsForQuery(id).map((v) => v.query)).toEqual([
      "SELECT 1",
    ]);
  });

  it("stored diff versions from before still resolve next to Core's keyframes", async () => {
    const tab = await openTab();
    tab.state.queryTabsByProject = { p1: [{ id: "t1", name: "Q", query: "" } as never] };
    const id = (await tab.savedQueries.saveQuery("Q", "SELECT a", "t1"))!;
    await tab.savedQueries.saveQuery("Q", "SELECT b", "t1"); // v1: keyframe "SELECT a"
    // A diff row the TypeScript wrote before 5d-1: v2 = "SELECT b" as a patch on v1.
    const { default: DiffMatchPatch } = await import("diff-match-patch");
    const dmp = new DiffMatchPatch();
    const diff = dmp.patch_toText(dmp.patch_make("SELECT a", "SELECT b"));
    await (
      library as unknown as { db: { execute(s: string, p: unknown[]): Promise<void> } }
    ).db.execute(
      "INSERT INTO query_versions (id, saved_query_id, version, snapshot, diff, created_at) VALUES (?, ?, 2, NULL, ?, ?)",
      ["ver-old", id, diff, new Date().toISOString()],
    );
    await tab.savedQueries.saveQuery("Q", "SELECT c", "t1"); // v3: keyframe "SELECT b"

    // Another window reads what's stored: the diff row resolves as before.
    const other = await openTab();
    expect(other.savedQueries.getResolvedVersionsForQuery(id).map((v) => v.query)).toEqual([
      "SELECT a",
      "SELECT b",
      "SELECT b",
    ]);
  });
});

describe("custom labels", () => {
  it("removing a custom label strips it from the connections that had it", async () => {
    const tab = await openTab();
    const label = await tab.projects.addCustomLabel("p1", { name: "Mine", color: "#ff0000" });
    const id = await storedConnection("p1", "Local", { labelIds: ["prod", label.id] });
    await tab.connections.refreshFromLibrary(null);

    await tab.projects.removeCustomLabel("p1", label.id);

    const stored = (await library.listConnections()).value;
    expect(stored.find((c) => c.id === id)?.labelIds).toEqual(["prod"]);
    expect(tab.state.connections.find((c) => c.id === id)?.labelIds).toEqual(["prod"]);
    expect(tab.state.projects.find((p) => p.id === "p1")?.customLabels).toEqual([]);
  });
});

describe("dashboards", () => {
  function dashboards(state: InstanceType<typeof DatabaseState>) {
    return new DashboardManager(
      state,
      async () => [],
      () => {},
    );
  }

  /** A setting as stored (the UI clamps; a hand-set or older value can be anything). */
  const storeSetting = (key: string, value: string) =>
    database.execute("INSERT INTO app_state (key, value) VALUES (?, ?)", [key, value]);

  it("starring a dashboard survives a reload", async () => {
    const tab = await openTab();
    const manager = dashboards(tab.state);
    const dashboard = (await manager.createDashboard("Sales"))!;

    await manager.toggleDashboardStarred(dashboard.id);

    const stored = (await library.listDashboards("p1")).value;
    expect(stored.find((d) => d.id === dashboard.id)?.starred).toBe(true);
  });

  it("a failed save is shown as an error", async () => {
    const tab = await openTab();
    const manager = dashboards(tab.state);
    const dashboard = (await manager.createDashboard("Sales"))!;
    vi.spyOn(library, "updateDashboard").mockRejectedValueOnce(
      new LibraryCallError("STORAGE_ERROR", "full"),
    );

    await manager.renameDashboard(dashboard.id, "Renamed");

    expect(toasts).toEqual([expect.stringContaining("full")]);
  });

  it("a failed delete keeps the dashboard and is shown as an error", async () => {
    const tab = await openTab();
    const manager = dashboards(tab.state);
    const dashboard = (await manager.createDashboard("Sales"))!;
    vi.spyOn(library, "removeDashboard").mockRejectedValueOnce(
      new LibraryCallError("STORAGE_ERROR", "locked"),
    );

    await manager.deleteDashboard(dashboard.id);

    expect(toasts).toEqual([expect.stringContaining("locked")]);
    expect(manager.getDashboard(dashboard.id)).toBeDefined();
    expect((await library.listDashboards("p1")).value).toHaveLength(1);
  });

  it("a negative version limit counts as unset", async () => {
    await storeSetting("dashboard_version_limit", "-5");
    const tab = await openTab();
    const manager = dashboards(tab.state);
    const dashboard = (await manager.createDashboard("Sales"))!;

    for (let i = 0; i < 12; i++) await manager.renameDashboard(dashboard.id, `Sales ${i}`);

    expect((await library.listDashboardVersions("p1")).value).toHaveLength(12);
  });

  it("the dashboard version limit setting is used", async () => {
    await storeSetting("dashboard_version_limit", "10");
    const tab = await openTab();
    const manager = dashboards(tab.state);
    const dashboard = (await manager.createDashboard("Sales"))!;

    for (let i = 0; i < 12; i++) await manager.renameDashboard(dashboard.id, `Sales ${i}`);

    const stored = (await library.listDashboardVersions("p1")).value;
    expect(stored.map((v) => v.version)).toEqual([3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    expect(manager.getVersionsForDashboard(dashboard.id).map((v) => v.id)).toEqual(
      stored.map((v) => v.id),
    );
  });
});

/**
 * 5d-2 Task 7 probe fix: `dashboardVersionsList` answers no snapshots, so
 * the history lists versions without them and reads one when it's picked.
 */
describe("dashboard version history", () => {
  const widget = (id: string) => ({
    id,
    title: id,
    x: 0,
    y: 0,
    width: 100,
    height: 100,
    querySource: "custom" as const,
    query: "SELECT 1",
    widgetType: "kpi" as const,
  });

  async function withHistory() {
    const tab = await openTab();
    const manager = new DashboardManager(
      tab.state,
      async () => [],
      () => {},
    );
    const d = (await manager.createDashboard("Sales"))!;
    await manager.addWidget(d.id, widget("w1"));
    await manager.renameDashboard(d.id, "Revenue");
    return { tab, manager, id: d.id };
  }

  it("lists versions without snapshots and loads one when it's opened", async () => {
    const { manager, id } = await withHistory();
    const versions = manager.getVersionsForDashboard(id);
    expect(versions.map((v) => [v.version, v.widgetCount])).toEqual([
      [1, 0],
      [2, 1],
    ]);
    expect(versions[0]).not.toHaveProperty("snapshot");
    const get = vi.spyOn(library, "getDashboardVersion");

    const loaded = await manager.loadVersion(id, versions[1].id);

    expect(get).toHaveBeenCalledTimes(1);
    expect(get).toHaveBeenCalledWith(id, versions[1].id);
    expect(loaded?.dashboard.name).toBe("Sales");
    expect(loaded?.dashboard.widgets.map((w) => w.id)).toEqual(["w1"]);
  });

  it("restores a loaded version", async () => {
    const { manager, id } = await withHistory();
    const first = manager.getVersionsForDashboard(id)[0];
    const loaded = (await manager.loadVersion(id, first.id))!;

    await manager.restoreVersion(id, loaded);

    expect(manager.getDashboard(id)?.name).toBe("Sales");
    expect(manager.getDashboard(id)?.widgets).toEqual([]);
    const stored = (await library.listDashboards("p1")).value.find((d) => d.id === id)!;
    expect([stored.name, stored.widgets]).toEqual(["Sales", "[]"]);
    // The state it had is versioned first, and the history shows it.
    expect(manager.getVersionsForDashboard(id).map((v) => [v.version, v.widgetCount])).toEqual([
      [1, 0],
      [2, 1],
      [3, 1],
    ]);
  });

  it("a version pruned elsewhere says so and leaves the history", async () => {
    const { manager, id } = await withHistory();
    const first = manager.getVersionsForDashboard(id)[0];
    await database.execute("DELETE FROM dashboard_versions WHERE id = ?", [first.id]);

    expect(await manager.loadVersion(id, first.id)).toBeNull();

    expect(toasts).toEqual([expect.stringContaining("dashboard version")]);
    await vi.waitFor(() =>
      expect(manager.getVersionsForDashboard(id).map((v) => v.version)).toEqual([2]),
    );
  });
});

describe("shared saved queries", () => {
  // Core writes, moves and deletes a shared query's file inside the library
  // call (phase 5e, Decision 36); the GUI's side is `shared-gui.svelte.test.ts`.
  it("renaming a shared query is one library call", async () => {
    const tab = await openTab();
    const id = (await tab.savedQueries.saveQuery("Orders", "SELECT 1"))!;
    await tab.savedQueries.shareQuery(id);
    await tab.savedQueries.renameQuery(id, "Big orders");
    expect(await storedQueryNames()).toEqual(["Big orders"]);
    expect(toasts).toEqual([]);
  });
});

describe("project removal", () => {
  it("removes the project and its connections in one call, and forgets them", async () => {
    const tab = await openTab();
    await storedConnection("p2", "c1");
    await storedConnection("p2", "c2");
    await tab.connections.refreshFromLibrary(null);

    expect(await tab.projects.remove("p2")).toBe(true);

    expect(tab.state.projects.map((p) => p.id)).toEqual(["p1"]);
    expect(tab.state.connections).toEqual([]);
    expect((await library.listProjects()).value.map((p) => p.id)).toEqual(["p1"]);
    expect((await library.listConnections()).value).toEqual([]);
  });

  it("keeps the project and its connections when the removal fails", async () => {
    const tab = await openTab();
    await storedConnection("p2", "c1");
    await tab.connections.refreshFromLibrary(null);
    vi.spyOn(library, "removeProject").mockRejectedValueOnce(new Error("STORAGE_ERROR: locked"));

    await expect(tab.projects.remove("p2")).rejects.toThrow("locked");

    expect(tab.state.projects.map((p) => p.id)).toContain("p2");
    expect(tab.state.connections.map((c) => c.name)).toEqual(["c1"]);
  });

  it("the last project isn't removed", async () => {
    const tab = await openTab();
    await tab.projects.remove("p2");
    expect(await tab.projects.remove("p1")).toBe(false);
    expect((await library.listProjects()).value.map((p) => p.id)).toEqual(["p1"]);
  });
});

describe("unlinking a project", () => {
  it("takes the connections Core removed out of the page and the project's order", async () => {
    const tab = await openTab();
    const kept = await storedConnection("p1", "Kept");
    const imported = await storedConnection("p1", "Prod", {
      sharedConnectionId: "repo-1:.seaquel/projects/p1/connections/prod.yaml",
    });
    await tab.connections.refreshFromLibrary(null);
    await library.setProjectSidebar("p1", [kept, imported]);
    tab.state.connectionOrderByProject = { p1: [kept, imported] };
    // Core removes the template's connection and stores the order without
    // it, in one transaction (phase 5e, Decision 40).
    const none = new NoShared();
    setShared({
      listRepos: none.listRepos,
      registerRepo: none.registerRepo,
      updateRepo: none.updateRepo,
      removeRepo: none.removeRepo,
      linkProject: none.linkProject,
      scan: none.scan,
      importProjects: none.importProjects,
      sync: none.sync,
      unlinkPreview: async () => ({ importedConnectionIds: [imported] }),
      unlinkProject: async () => {
        await library.removeConnection(imported);
        await library.setProjectSidebar("p1", [kept]);
        return {
          value: { removedConnectionIds: [imported], keptConnectionIds: [], repoRemoved: true },
          seq: (await library.listConnections()).seq,
        };
      },
    });
    try {
      await tab.projects.unlinkProject("p1", true);
    } finally {
      setShared(null);
    }

    expect(tab.state.connectionOrderByProject.p1).toEqual([kept]);
    expect(tab.state.connections.map((c) => c.id)).toEqual([kept]);
  });
});
