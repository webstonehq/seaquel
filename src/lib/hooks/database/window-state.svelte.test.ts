/**
 * A window's view state (phase 5d-2, Decision 22), against a real metadata
 * database: the demo's `TsUi` and `TsLibrary` over sql.js, which keep
 * Core's `ui` and library rules, with the view models on top. Each window
 * (a desktop window or a web tab) is its own set of managers under its own
 * window id, on one database.
 *
 * - Each window keeps its own tabs, layout, active ids and active
 *   connection; a new one starts with the most recently used window's, and
 *   the first one after the upgrade with today's rows.
 * - Nothing is saved for a project before this window's load of it answered.
 * - `rev` orders the saves: the `pagehide` save isn't overwritten by an
 *   older one still queued, and a stale answer moves the counter past it.
 * - The connection order and the saved workflows aren't view state.
 */
import initSqlJs from "sql.js";
import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import type { PersistedProjectState } from "$lib/types";

const storage = vi.hoisted(() => ({ client: null as unknown }));
vi.mock("$lib/storage", () => ({ getStorage: () => storage.client }));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({ isAvailable: () => false }),
}));
const toasts: string[] = [];
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("svelte-sonner", () => ({
  toast: { success: vi.fn(), info: (m: string) => toasts.push(m), warning: vi.fn() },
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { bootstrapSqljsDatabase, createSqljsStorageClient } =
  await import("$lib/storage/sqljs-client");
const { WebSqliteDatabase } = await import("$lib/storage/web-sqlite");
const { projectsRepo, projectStateRepo } = await import("$lib/storage/repository");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { StarterTabManager } = await import("./starter-tabs.svelte.js");
const { TabOrderingManager } = await import("./tab-ordering.svelte.js");
const { PaneManager } = await import("./pane-manager.svelte.js");
const { TsLibrary } = await import("./library/ts-library");
const { TsUi } = await import("./library/ts-ui");
const { setLibrary } = await import("./library/index");
import type { LibraryService } from "./library/types";
const { LibraryCallError } = await import("./library/types");
const { resetLoadGuardToast } = await import("$lib/storage/load-guard");
import type { UiService, ViewState } from "./library/types";

let SQL: Awaited<ReturnType<typeof initSqlJs>>;
let db: InstanceType<typeof WebSqliteDatabase>;
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
  resetLoadGuardToast();
  db = new WebSqliteDatabase(new SQL.Database());
  await bootstrapSqljsDatabase(db);
  storage.client = createSqljsStorageClient(db);
  library = new TsLibrary(db);
  setLibrary(library);
  const now = new Date().toISOString();
  for (const id of ["p1", "p2"]) {
    await projectsRepo.save(db, { id, name: id, createdAt: now, updatedAt: now, customLabels: [] });
  }
});

/** A window's `ui` calls, recorded, with saves that can be held. */
class WindowUi implements UiService {
  readonly calls: { method: string; projectId?: string; rev?: number }[] = [];
  /** While set, `windowStateLoad` waits for it. */
  holdLoads: Promise<void> | null = null;
  /** While set, `windowStateSave` waits for it before reaching the database. */
  holdSaves: Promise<void> | null = null;
  /** A save throws this instead (a web limit, say). */
  failSaves: Error | null = null;
  /** A load throws this instead. */
  failLoads: Error | null = null;
  readonly inner: InstanceType<typeof TsUi>;
  constructor(readonly windowId: string) {
    this.inner = new TsUi(db, { origin: () => windowId });
  }
  windowGet(windowId: string) {
    this.calls.push({ method: "windowGet" });
    return this.inner.windowGet(windowId);
  }
  windowActivate(windowId: string, projectId: string) {
    this.calls.push({ method: "windowActivate", projectId });
    return this.inner.windowActivate(windowId, projectId);
  }
  async windowStateLoad(windowId: string, projectId: string) {
    this.calls.push({ method: "windowStateLoad", projectId });
    if (this.holdLoads) await this.holdLoads;
    if (this.failLoads) throw this.failLoads;
    return this.inner.windowStateLoad(windowId, projectId);
  }
  async windowStateSave(windowId: string, projectId: string, rev: number, state: ViewState) {
    this.calls.push({ method: "windowStateSave", projectId, rev });
    if (this.failSaves) throw this.failSaves;
    if (this.holdSaves) await this.holdSaves;
    return this.inner.windowStateSave(windowId, projectId, rev, state);
  }
  /** Web's `pagehide` save: straight to the database, outside any queue. */
  windowStateSaveKeepalive(windowId: string, projectId: string, rev: number, state: ViewState) {
    this.calls.push({ method: "keepalive", projectId, rev });
    void this.inner.windowStateSave(windowId, projectId, rev, state);
    return true;
  }
  saves(projectId = "p1") {
    return this.calls.filter((c) => c.method === "windowStateSave" && c.projectId === projectId);
  }
}

/** One window: its managers, under `windowId`, over the shared database. */
async function openWindow(
  windowId: string,
  options: { ui?: WindowUi; start?: boolean; project?: string } = {},
) {
  const ui = options.ui ?? new WindowUi(windowId);
  const state = new DatabaseState();
  const windowState = new WindowStateManager(state, { ui: () => ui, windowId: () => windowId });
  const restoration = new StateRestorationManager(state);
  const projects = new ProjectManager(state, windowState, restoration);
  const connections = new ConnectionManager(
    state,
    windowState,
    restoration,
    {} as never,
    { getForType: async () => ({}) } as never,
    async () => {},
    () => {},
  );
  projects.setConnectionManager(connections);
  const schedule = (id: string | null) => windowState.scheduleProject(id);
  const tabs = new TabOrderingManager(state, schedule, new PaneManager(state, schedule));
  projects.setStarterTabManager(new StarterTabManager(state, tabs, schedule));
  if (options.start !== false) {
    await connections.initializePersistedConnections();
    await projects.initialize();
    if (options.project) await projects.setActive(options.project);
  }
  return { ui, state, windowState, projects, connections };
}

type Window = Awaited<ReturnType<typeof openWindow>>;

/** Give `projectId` query tabs `ids` in `w`, as opening them would. */
function openTabs(w: Window, ids: string[], projectId = "p1") {
  w.state.queryTabsByProject[projectId] = ids.map((id) => ({
    id,
    name: id,
    query: `SELECT '${id}'`,
    isExecuting: false,
  }));
  w.state.tabOrderByProject[projectId] = [...ids];
  w.state.activeQueryTabIdByProject[projectId] = ids.at(-1) ?? null;
  w.state.starterTabsByProject[projectId] = [];
}

const tabIds = (w: Window, projectId = "p1") =>
  (w.state.queryTabsByProject[projectId] ?? []).map((t) => t.id);

/** The stored view state of `windowId`'s project. */
async function storedState(windowId: string, projectId = "p1") {
  const [row] = await db.query<{ state: string; rev: number }>(
    "SELECT state, rev FROM window_state WHERE window_id = ? AND project_id = ?",
    [windowId, projectId],
  );
  return row ? { state: JSON.parse(row.state) as ViewState, rev: Number(row.rev) } : null;
}

function legacyState(tabs: string[]): PersistedProjectState {
  return {
    projectId: "p1",
    queryTabs: tabs.map((id) => ({ id, name: id, query: `SELECT '${id}'` })),
    schemaTabs: [],
    explainTabs: [],
    erdTabs: [],
    tabOrder: tabs,
    connectionOrder: [],
    activeQueryTabId: tabs[0] ?? null,
    activeSchemaTabId: null,
    activeExplainTabId: null,
    activeErdTabId: null,
    activeView: "query",
    activeConnectionId: null,
  };
}

describe("each window keeps its own view state", () => {
  it("a tab's layout isn't moved by another tab", async () => {
    const a = await openWindow("win-a");
    const b = await openWindow("win-b");
    openTabs(a, ["a1", "a2"]);
    a.state.paneLayoutByProject.p1 = {
      panes: [
        { id: "left", tabIds: ["a1"], activeTabId: "a1" },
        { id: "right", tabIds: ["a2"], activeTabId: "a2" },
      ],
      activePaneId: "right",
    };
    await a.windowState.saveNow("p1");
    openTabs(b, ["b1"]);
    await b.windowState.saveNow("p1");
    // A saves again after B: B's row stays B's.
    await a.windowState.saveNow("p1");

    const b2 = await openWindow("win-b");
    expect(tabIds(b2)).toEqual(["b1"]);
    expect(b2.state.paneLayoutByProject.p1).toBeUndefined();
    const a2 = await openWindow("win-a");
    expect(tabIds(a2)).toEqual(["a1", "a2"]);
    expect(a2.state.paneLayoutByProject.p1.activePaneId).toBe("right");
  });

  it("a new tab starts with the most recent window's tabs", async () => {
    const a = await openWindow("win-a");
    const b = await openWindow("win-b");
    openTabs(a, ["a1"]);
    await a.windowState.saveNow("p1");
    openTabs(b, ["b1", "b2"]);
    await b.windowState.saveNow("p1");

    const c = await openWindow("win-c");
    expect(tabIds(c)).toEqual(["b1", "b2"]);
    // The copy is C's own from now on.
    expect((await storedState("win-c"))?.state.queryTabs.map((t) => t.id)).toEqual(["b1", "b2"]);
    openTabs(c, ["c1"]);
    await c.windowState.saveNow("p1");
    expect((await storedState("win-b"))?.state.queryTabs.map((t) => t.id)).toEqual(["b1", "b2"]);
  });

  it("a reload keeps the tab's own tabs", async () => {
    const a = await openWindow("win-a");
    const b = await openWindow("win-b");
    openTabs(a, ["a1"]);
    await a.windowState.saveNow("p1");
    // B is used after A: the most recent window.
    openTabs(b, ["b1"]);
    await b.windowState.saveNow("p1");

    const reloaded = await openWindow("win-a");
    expect(tabIds(reloaded)).toEqual(["a1"]);
  });

  it("the first window after the upgrade gets today's tabs", async () => {
    await projectStateRepo.save(db, legacyState(["today-1", "today-2"]));
    const ui = new WindowUi("main");
    const w = await openWindow("main", { ui });
    expect(tabIds(w)).toEqual(["today-1", "today-2"]);
    expect(w.state.starterTabsByProject.p1 ?? []).toEqual([]);
    // Stored as the window's own at once.
    expect((await storedState("main"))?.state.queryTabs.map((t) => t.id)).toEqual([
      "today-1",
      "today-2",
    ]);
  });

  it("a project with no stored state gets the starter tabs", async () => {
    const w = await openWindow("win-a");
    expect(tabIds(w)).toEqual([]);
    expect(w.state.starterTabsByProject.p1?.map((t) => t.id)).toEqual([
      "getting-started",
      "migration-tips",
    ]);
  });

  it("a call naming another window's id is refused", async () => {
    const ui = new TsUi(db, { origin: () => "win-a" });
    const state = legacyState(["x"]) as ViewState;
    await expect(ui.windowStateSave("win-b", "p1", 1, state)).rejects.toMatchObject({
      code: "INVALID_ARGUMENT",
    });
    await expect(ui.windowStateLoad("win-b", "p1")).rejects.toMatchObject({
      code: "INVALID_ARGUMENT",
    });
    await expect(ui.windowActivate("win-b", "p1")).rejects.toMatchObject({
      code: "INVALID_ARGUMENT",
    });
    expect(await storedState("win-b")).toBeNull();
    // The page only ever names its own window.
    const a = await openWindow("win-a");
    openTabs(a, ["a1"]);
    await a.windowState.saveNow("p1");
    expect(await storedState("win-a")).not.toBeNull();
  });

  it("each window keeps its own active connection", async () => {
    const c1 = (await library.createConnection(connectionDraft("One"))).value.id;
    const c2 = (await library.createConnection(connectionDraft("Two"))).value.id;
    const a = await openWindow("win-a");
    const b = await openWindow("win-b");
    a.connections.setActiveForProject(c1, "p1");
    b.connections.setActiveForProject(c2, "p1");
    await a.windowState.flush();
    await b.windowState.flush();

    expect((await openWindow("win-a")).state.activeConnectionIdByProject.p1).toBe(c1);
    expect((await openWindow("win-b")).state.activeConnectionIdByProject.p1).toBe(c2);
  });

  it("each project keeps its own activeView, in the stored state", async () => {
    const w = await openWindow("win-a");
    w.state.activeView = "erd";
    openTabs(w, ["a1"]);
    w.windowState.scheduleProject("p1");
    await w.projects.setActive("p2");
    // As `UIStateManager.setActiveView` does.
    w.state.activeView = "dashboard";
    w.state.activeViewByProject.p2 = "dashboard";
    openTabs(w, ["p2-tab"], "p2");
    await w.windowState.saveNow("p2");

    expect((await storedState("win-a", "p1"))?.state.activeView).toBe("erd");
    expect((await storedState("win-a", "p2"))?.state.activeView).toBe("dashboard");
    await w.projects.setActive("p1");
    expect(w.state.activeView).toBe("erd");
  });

  it("DuckDB extensions tabs are kept in the window's state", async () => {
    const c1 = (await library.createConnection(connectionDraft("Duck"))).value.id;
    const a = await openWindow("win-a");
    a.state.extensionsDuckdbTabsByProject.p1 = [
      { id: "ext", name: "Extensions", connectionId: c1, isLoading: false },
    ];
    await a.windowState.saveNow("p1");
    const again = await openWindow("win-a");
    expect(again.state.extensionsDuckdbTabsByProject.p1.map((t) => t.id)).toEqual(["ext"]);
  });

  it("the stored state carries no saved workflows or connection order", async () => {
    const a = await openWindow("win-a");
    openTabs(a, ["a1"]);
    a.state.connectionOrderByProject.p1 = ["c9"];
    a.state.savedWorkflowsByProject.p1 = [{ id: "w1" } as never];
    await a.windowState.saveNow("p1");
    const stored = (await storedState("win-a"))!.state as Record<string, unknown>;
    expect(Object.keys(stored)).not.toContain("savedWorkflows");
    expect(Object.keys(stored)).not.toContain("connectionOrder");
    expect(Object.keys(stored)).not.toContain("starredSharedQueryIds");
  });

  it("saved workflows load from the library, not the view state", async () => {
    await db.execute("INSERT INTO saved_canvases (id, project_id, data) VALUES (?, ?, ?)", [
      "workflow-1",
      "p1",
      JSON.stringify({ id: "workflow-1", name: "Flow", nodes: [], edges: [] }),
    ]);
    const w = await openWindow("win-a");
    expect(w.state.savedWorkflowsByProject.p1.map((wf) => wf.id)).toEqual(["workflow-1"]);
  });
});

describe("the window's active project", () => {
  it("switching records it for the window and as lastActiveProjectId", async () => {
    const a = await openWindow("win-a");
    await a.projects.setActive("p2");
    expect(a.ui.calls.filter((c) => c.method === "windowActivate").at(-1)?.projectId).toBe("p2");
    const [row] = await db.query<{ value: string }>(
      "SELECT value FROM app_state WHERE key = 'lastActiveProjectId'",
    );
    expect(row.value).toBe("p2");
    // A reload opens on it; a new window too (the most recent window's).
    expect((await openWindow("win-a")).state.activeProjectId).toBe("p2");
    expect((await openWindow("win-new")).state.activeProjectId).toBe("p2");
  });

  it("each window opens on its own active project", async () => {
    const a = await openWindow("win-a", { project: "p2" });
    const b = await openWindow("win-b", { project: "p1" });
    expect(a.state.activeProjectId).toBe("p2");
    expect(b.state.activeProjectId).toBe("p1");
    expect((await openWindow("win-a")).state.activeProjectId).toBe("p2");
  });
});

describe("no view-state save before its load answers", () => {
  it("the startup case", async () => {
    const ui = new WindowUi("win-a");
    let release!: () => void;
    ui.holdLoads = new Promise((r) => (release = r));
    const w = await openWindow("win-a", { ui, start: false });
    const starting = w.projects.initialize();
    await vi.waitFor(() => expect(ui.calls.some((c) => c.method === "windowStateLoad")).toBe(true));
    // A tab change while the load is out: its save is refused.
    openTabs(w, ["early"]);
    await w.windowState.saveNow("p1");
    expect(ui.saves()).toEqual([]);

    await projectStateRepo.save(db, legacyState(["stored"]));
    release();
    await starting;
    expect(tabIds(w)).toEqual(["stored"]);
    await w.windowState.saveNow("p1");
    expect(ui.saves()).toHaveLength(1);
    expect((await storedState("win-a"))?.state.queryTabs.map((t) => t.id)).toEqual(["stored"]);
  });

  it("the switch case", async () => {
    await projectStateRepo.save(db, { ...legacyState(["stored-p2"]), projectId: "p2" });
    // The window switches to p2 with a slow load.
    const ui = new WindowUi("win-a");
    const w = await openWindow("win-a", { ui });
    expect(w.state.activeProjectId).toBe("p1");
    let release!: () => void;
    ui.holdLoads = new Promise((r) => (release = r));
    const switching = w.projects.setActive("p2");
    await vi.waitFor(() => expect(w.state.activeProjectId).toBe("p2"));
    w.windowState.scheduleProject("p2");
    await w.windowState.saveNow("p2");
    expect(ui.saves("p2")).toEqual([]);

    release();
    await switching;
    expect(tabIds(w, "p2")).toEqual(["stored-p2"]);
    await w.windowState.saveNow("p2");
    expect(ui.saves("p2")).toHaveLength(1);
    expect((await storedState("win-a", "p2"))?.state.queryTabs[0].id).toBe("stored-p2");
  });

  it("a removed project isn't saved by flush", async () => {
    const w = await openWindow("win-a");
    await w.projects.setActive("p2");
    await w.projects.setActive("p1");
    w.windowState.scheduleProject("p2");
    w.windowState.forgetProject("p2");
    await w.windowState.flush();
    await w.windowState.saveNow("p2");
    expect(w.ui.saves("p2").filter((c) => c.rev! > 0)).toHaveLength(1); // the switch's, before
  });

  it("flush saves only projects with a pending change, the active one first", async () => {
    const w = await openWindow("win-a");
    await w.projects.setActive("p2");
    w.ui.calls.length = 0;
    w.windowState.scheduleProject("p1");
    w.windowState.scheduleProject("p2");
    await w.windowState.flush();
    expect(
      w.ui.calls.filter((c) => c.method === "windowStateSave").map((c) => c.projectId),
    ).toEqual(["p2", "p1"]);
    w.ui.calls.length = 0;
    await w.windowState.flush();
    expect(w.ui.calls).toEqual([]);
  });
});

describe("rev", () => {
  it("counts on from the rev the load answered", async () => {
    const a = await openWindow("win-a");
    openTabs(a, ["a1"]);
    await a.windowState.saveNow("p1");
    await a.windowState.saveNow("p1");
    const stored = await storedState("win-a");
    const again = await openWindow("win-a");
    openTabs(again, ["a2"]);
    await again.windowState.saveNow("p1");
    expect(again.ui.saves().map((c) => c.rev)).toEqual([stored!.rev + 1]);
    expect((await storedState("win-a"))?.state.queryTabs.map((t) => t.id)).toEqual(["a2"]);
  });

  it("an older queued save doesn't overwrite the pagehide save", async () => {
    const w = await openWindow("win-a");
    w.state.activeProjectId = "p1";
    openTabs(w, ["older"]);
    let release!: () => void;
    w.ui.holdSaves = new Promise((r) => (release = r));
    // A save leaves and waits in the write queue…
    const queued = w.windowState.saveNow("p1");
    // …the user types on, and the page goes away before it lands.
    openTabs(w, ["newest"]);
    w.windowState.scheduleProject("p1");
    w.windowState.saveOnPageHide();
    await vi.waitFor(async () =>
      expect((await storedState("win-a"))?.state.queryTabs.map((t) => t.id)).toEqual(["newest"]),
    );
    w.ui.holdSaves = null;
    release();
    await queued;

    const keepalive = w.ui.calls.find((c) => c.method === "keepalive")!;
    const older = w.ui.saves()[0];
    expect(keepalive.rev).toBeGreaterThan(older.rev!);
    expect((await storedState("win-a"))?.state.queryTabs.map((t) => t.id)).toEqual(["newest"]);
  });

  it("a stale answer moves the counter past the stored rev", async () => {
    // This page loaded its window's state; another page of the same window
    // (the tab after a reload, whose old page's save came late) saved on.
    const page = await openWindow("win-a");
    const other = new TsUi(db, { origin: () => "win-a" });
    const view = legacyState(["from-the-other-page"]) as ViewState;
    for (let rev = 1; rev <= 5; rev++) await other.windowStateSave("win-a", "p1", rev, view);

    // A change on this page: its state is saved over the other page's.
    openTabs(page, ["this-page"]);
    page.windowState.scheduleProject("p1");
    await page.windowState.saveNow("p1");
    expect((await storedState("win-a"))?.state.queryTabs[0].id).toBe("from-the-other-page");
    // The next save goes past the stored rev and lands.
    await page.windowState.saveNow("p1");
    expect(page.ui.saves().map((c) => c.rev)).toEqual([1, 6]);
    expect((await storedState("win-a"))?.state.queryTabs[0].id).toBe("this-page");
  });
});

/** The library with `method` held until `until` resolves. */
function holding(method: string, until: Promise<void>): LibraryService {
  return new Proxy(library, {
    get(target, key) {
      const value: unknown = Reflect.get(target, key);
      if (typeof value !== "function") return value;
      return async (...args: unknown[]) => {
        if (key === method) await until;
        return (value as (...a: unknown[]) => unknown).apply(target, args);
      };
    },
  }) as LibraryService;
}

describe("review fixes", () => {
  it("I1: no save between the load's answer and the restore (switch)", async () => {
    await projectStateRepo.save(db, { ...legacyState(["stored-p2"]), projectId: "p2" });
    const w = await openWindow("win-a");
    let release!: () => void;
    setLibrary(holding("listWorkflows", new Promise((r) => (release = r))));
    const switching = w.projects.setActive("p2");
    // The load answered (its copy is stored), the restore waits on the workflows.
    await vi.waitFor(async () => expect(await storedState("win-a", "p2")).not.toBeNull());
    w.windowState.scheduleProject("p2");
    await w.windowState.saveNow("p2");
    expect(w.ui.saves("p2")).toEqual([]);

    release();
    await switching;
    setLibrary(library);
    expect(tabIds(w, "p2")).toEqual(["stored-p2"]);
    openTabs(w, ["stored-p2", "new"], "p2");
    w.windowState.scheduleProject("p2");
    await w.windowState.saveNow("p2");
    expect((await storedState("win-a", "p2"))?.state.queryTabs.map((t) => t.id)).toEqual([
      "stored-p2",
      "new",
    ]);
  });

  it("I1: no save between the load's answer and the restore (startup)", async () => {
    await projectStateRepo.save(db, legacyState(["stored"]));
    const w = await openWindow("win-a", { start: false });
    let release!: () => void;
    setLibrary(holding("getProjectSidebar", new Promise((r) => (release = r))));
    const starting = w.projects.initialize();
    await vi.waitFor(async () => expect(await storedState("win-a")).not.toBeNull());
    await w.windowState.saveNow("p1");
    expect(w.ui.saves()).toEqual([]);
    release();
    await starting;
    setLibrary(library);
    expect(tabIds(w)).toEqual(["stored"]);
    expect((await storedState("win-a"))?.state.queryTabs.map((t) => t.id)).toEqual(["stored"]);
  });

  it("M4: flush waits for a save already on its way", async () => {
    const w = await openWindow("win-a");
    openTabs(w, ["a1"]);
    w.windowState.scheduleProject("p1");
    let release!: () => void;
    w.ui.holdSaves = new Promise((r) => (release = r));
    const onItsWay = w.windowState.saveNow("p1");
    let flushed = false;
    const flushing = w.windowState.flush().then(() => (flushed = true));
    await new Promise((r) => setTimeout(r, 10));
    expect(flushed).toBe(false);
    w.ui.holdSaves = null;
    release();
    await flushing;
    await onItsWay;
    expect((await storedState("win-a"))?.state.queryTabs[0].id).toBe("a1");
  });

  it("M4: flush saves again after a stale answer, a bounded number of times", async () => {
    const w = await openWindow("win-a");
    const other = new TsUi(db, { origin: () => "win-a" });
    await other.windowStateSave("win-a", "p1", 5, legacyState(["other"]) as ViewState);
    openTabs(w, ["mine"]);
    w.windowState.scheduleProject("p1");
    await w.windowState.flush();
    expect((await storedState("win-a"))?.state.queryTabs[0].id).toBe("mine");

    // A store that answers every save stale: flush still ends.
    const always = new WindowUi("win-b");
    always.windowStateSave = async (_w, projectId, rev) => {
      always.calls.push({ method: "windowStateSave", projectId, rev });
      return { value: { stale: true, rev: rev + 10 }, seq: { epoch: "e", n: 0 } };
    };
    const b = await openWindow("win-b", { ui: always });
    openTabs(b, ["b1"]);
    b.windowState.scheduleProject("p1");
    await b.windowState.flush();
    expect(always.saves().length).toBeLessThanOrEqual(3);
    b.windowState.cancelPending();
  });

  it("M5: a stale save with nothing changed reloads instead of overwriting", async () => {
    await projectStateRepo.save(db, legacyState(["loaded"]));
    const page = await openWindow("win-a");
    const loadedRev = (await storedState("win-a"))!.rev;
    // The old page's keepalive lands after this page's load.
    const old = new TsUi(db, { origin: () => "win-a" });
    await old.windowStateSave("win-a", "p1", loadedRev + 1, legacyState(["newest"]) as ViewState);

    // This page saves without having changed anything (a switch, say).
    await page.windowState.saveNow("p1");
    expect((await storedState("win-a"))?.state.queryTabs[0].id).toBe("newest");
    await vi.waitFor(() => expect(tabIds(page)).toEqual(["newest"]));
    await page.windowState.flush();
    expect((await storedState("win-a"))?.state.queryTabs[0].id).toBe("newest");
  });

  it("M6: a tab naming a connection this page hasn't heard of yet is kept", async () => {
    const w = await openWindow("win-a");
    // Another window creates a connection and opens a data tab on it in p2.
    const fresh = (await library.createConnection({ ...connectionDraft("Fresh"), projectId: "p2" }))
      .value.id;
    const other = new TsUi(db, { origin: () => "win-other" });
    await other.windowStateSave("win-other", "p2", 1, {
      ...legacyState([]),
      projectId: "p2",
      dataTabs: [
        { id: "d-fresh", connectionId: fresh, tableName: "t", schemaName: "s" },
        { id: "d-gone", connectionId: "conn-gone", tableName: "t", schemaName: "s" },
      ],
    } as ViewState);

    await w.projects.setActive("p2");
    expect(w.state.dataTabsByProject.p2.map((t) => t.id)).toEqual(["d-fresh"]);
    expect(w.state.connections.some((c) => c.id === fresh)).toBe(true);
  });
});

describe("re-review follow-ups", () => {
  it("N1: a stale-triggered reload whose load fails leaves the tabs and keeps saving", async () => {
    await projectStateRepo.save(db, legacyState(["loaded"]));
    const page = await openWindow("win-a");
    const rev = (await storedState("win-a"))!.rev;
    const old = new TsUi(db, { origin: () => "win-a" });
    await old.windowStateSave("win-a", "p1", rev + 1, legacyState(["newest"]) as ViewState);
    page.ui.failLoads = new Error("STORAGE_ERROR: upstream unavailable");

    await page.windowState.saveNow("p1");
    await vi.waitFor(() =>
      expect(page.ui.calls.filter((c) => c.method === "windowStateLoad")).toHaveLength(2),
    );
    await new Promise((r) => setTimeout(r, 10));
    expect(tabIds(page)).toEqual(["loaded"]);

    page.ui.failLoads = null;
    openTabs(page, ["loaded", "edited"]);
    page.windowState.scheduleProject("p1");
    await page.windowState.saveNow("p1");
    expect((await storedState("win-a"))?.state.queryTabs.map((t) => t.id)).toEqual([
      "loaded",
      "edited",
    ]);
  });

  it("N2: a stale answer to the save before a switch doesn't reload the old project", async () => {
    await projectStateRepo.save(db, legacyState(["loaded"]));
    await projectStateRepo.save(db, { ...legacyState(["p2-tab"]), projectId: "p2" });
    const page = await openWindow("win-a");
    const rev = (await storedState("win-a"))!.rev;
    const old = new TsUi(db, { origin: () => "win-a" });
    await old.windowStateSave("win-a", "p1", rev + 1, legacyState(["newest"]) as ViewState);

    await page.projects.setActive("p2");
    await new Promise((r) => setTimeout(r, 20));
    expect(page.state.activeProjectId).toBe("p2");
    expect(tabIds(page, "p2")).toEqual(["p2-tab"]);
    const p1Loads = page.ui.calls.filter(
      (c) => c.method === "windowStateLoad" && c.projectId === "p1",
    );
    expect(p1Loads).toHaveLength(1);
    expect((await storedState("win-a"))?.state.queryTabs[0].id).toBe("newest");
  });
});

describe("a save refused for a web limit", () => {
  it("is shown once per project and limit, and saving goes on", async () => {
    const w = await openWindow("win-a");
    await w.projects.setActive("p2");
    const tooLong = new LibraryCallError(
      "INVALID_ARGUMENT",
      "The text of tab q1 is larger than allowed here (max_tab_text_bytes: 2097152 bytes).",
    );
    w.ui.failSaves = tooLong;
    await w.windowState.saveNow("p1");
    await w.windowState.saveNow("p1");
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toContain("max_tab_text_bytes");
    expect(toasts[0]).toContain("tab q1");
    expect(toasts[0]).not.toContain("INVALID_ARGUMENT:");
    // Another project, or another limit, is said once too.
    await w.windowState.saveNow("p2");
    w.ui.failSaves = new LibraryCallError(
      "INVALID_ARGUMENT",
      "There are more tabs than allowed here (max_tabs: 500).",
    );
    await w.windowState.saveNow("p1");
    await w.windowState.saveNow("p1");
    expect(toasts).toHaveLength(3);
    // Once under it, the saves land again.
    w.ui.failSaves = null;
    openTabs(w, ["trimmed"]);
    await w.windowState.saveNow("p1");
    expect((await storedState("win-a"))?.state.queryTabs[0].id).toBe("trimmed");
  });
});

function connectionDraft(name: string) {
  return {
    projectId: "p1",
    name,
    type: "postgres" as const,
    host: "localhost",
    port: 5432,
    databaseName: "app",
    username: "me",
  };
}

describe("5d-2 Task 7 probe fixes", () => {
  /**
   * A `ui` whose saves refuse a lone surrogate as Core's JSON reader does
   * (`JSON.stringify` writes one as a `\udXXX` escape, and only a lone one).
   */
  function strictUi(windowId: string): WindowUi {
    const ui = new WindowUi(windowId);
    const save = ui.windowStateSave.bind(ui);
    ui.windowStateSave = async (w, p, rev, state) => {
      if (/\\ud[89a-f][0-9a-f]{2}/i.test(JSON.stringify(state))) {
        throw new Error("INVALID_ARGUMENT: lone surrogate");
      }
      return save(w, p, rev, state);
    };
    const keepalive = ui.windowStateSaveKeepalive.bind(ui);
    ui.windowStateSaveKeepalive = (w, p, rev, state) => {
      if (/\\ud[89a-f][0-9a-f]{2}/i.test(JSON.stringify(state))) {
        throw new Error("INVALID_ARGUMENT: lone surrogate");
      }
      return keepalive(w, p, rev, state);
    };
    return ui;
  }

  it("a lone surrogate in a tab's text is replaced, so the view state still saves", async () => {
    const w = await openWindow("win-s", { ui: strictUi("win-s") });
    openTabs(w, ["t1"]);
    w.state.queryTabsByProject.p1[0].query = "SELECT '\ud800' -- \u{1F600}";
    w.state.queryTabsByProject.p1[0].name = "half \udc00";
    await w.windowState.saveNow("p1");
    const stored = await storedState("win-s");
    const tab = (stored?.state.queryTabs ?? [])[0];
    expect(tab?.query).toBe("SELECT '\uFFFD' -- \u{1F600}");
    expect(tab?.name).toBe("half \uFFFD");
    // The page keeps what it shows.
    expect(w.state.queryTabsByProject.p1[0].query).toBe("SELECT '\ud800' -- \u{1F600}");
  });

  it("the pagehide save is made well-formed too", async () => {
    const w = await openWindow("win-k", { ui: strictUi("win-k") });
    openTabs(w, ["t1"]);
    w.state.queryTabsByProject.p1[0].query = "x\udbff";
    w.windowState.scheduleProject("p1");
    w.windowState.saveOnPageHide();
    await w.windowState.flush();
    expect(w.ui.calls.some((c) => c.method === "keepalive")).toBe(true);
    // The keepalive send isn't awaited: wait for it to land.
    await vi.waitFor(async () => {
      const stored = await storedState("win-k");
      expect((stored?.state.queryTabs ?? [])[0]?.query).toBe("x\uFFFD");
    });
  });
});
