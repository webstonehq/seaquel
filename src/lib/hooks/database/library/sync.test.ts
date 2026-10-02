/**
 * Other windows' library changes reaching this page (phase 5d-1, Decision
 * 18): `LibrarySync` + `ChangeFeed` + the view models, over a recording
 * library and a Core client whose event channel the test drives.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  CoreClient,
  EventsUnavailableReason,
  ResubscribedInfo,
  WorkspaceEvent,
} from "$lib/core/client";

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => true,
  isWeb: () => false,
  isDemo: () => false,
}));
vi.mock("$lib/storage", () => ({
  getStorage: () => ({
    appState: { get: async () => null, set: async () => {} },
    dashboards: { loadByProject: async () => [] },
    dashboardVersions: { loadByProject: async () => [] },
    queryHistory: { loadByConnection: async () => [] },
    aiChats: { loadByConnection: async () => [] },
  }),
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: async () => [] }),
  TsEngineClient: class {},
}));
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

const { DatabaseState } = await import("../state.svelte.js");
const { WindowStateManager } = await import("../window-state.svelte.js");
const { StateRestorationManager } = await import("../state-restoration.svelte.js");
const { ProjectManager } = await import("../project-manager.svelte.js");
const { ConnectionManager } = await import("../connection-manager.svelte.js");
const { SavedQueryManager } = await import("../saved-queries.svelte.js");
const { ChangeFeed } = await import("./change-feed");
const { LibrarySync } = await import("./sync");
const { RecordingLibrary } = await import("./recording-library");
const { setLibrary } = await import("./index");

/** A Core client whose event channel the test drives. */
function fakeClient() {
  const events = new Set<(e: WorkspaceEvent) => void>();
  const resubscribed = new Set<(i: ResubscribedInfo) => void>();
  const unavailable = new Set<(r: EventsUnavailableReason) => void>();
  const client = {
    call: vi.fn(),
    stream: vi.fn(),
    events: (h: (e: WorkspaceEvent) => void) => (events.add(h), () => events.delete(h)),
    onResubscribed: (h: (i: ResubscribedInfo) => void) => (
      resubscribed.add(h),
      () => resubscribed.delete(h)
    ),
    onEventsUnavailable: (h: (r: EventsUnavailableReason) => void) => (
      unavailable.add(h),
      () => unavailable.delete(h)
    ),
  } as unknown as CoreClient;
  return {
    client,
    emit: (e: WorkspaceEvent) => events.forEach((h) => h(e)),
    resubscribe: (initial: boolean) => resubscribed.forEach((h) => h({ initial })),
    unavailable: (r: EventsUnavailableReason) => unavailable.forEach((h) => h(r)),
  };
}

let library: InstanceType<typeof RecordingLibrary>;

/** One page: its managers, feed and sync over the shared recording library. */
async function openPage() {
  const channel = fakeClient();
  const state = new DatabaseState();
  // The view state is per window and not synced (Decision 22): off here.
  const windowState = new WindowStateManager(state, { enabled: false });
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
  const savedQueries = new SavedQueryManager(state, () => {});
  const feed = new ChangeFeed({
    client: () => channel.client,
    origin: () => "this-tab",
    seqs: state.librarySeqs,
  });
  const sharedRepos = { refreshRepos: vi.fn(async (_ids: readonly string[] | null) => {}) };
  const sync = new LibrarySync(state, feed, {
    connections,
    projects,
    savedQueries,
    history: restoration,
    projectsViewState: projects,
    windowId: () => "this-tab",
    sharedRepos,
  });
  sync.start();
  await projects.initialize();
  await connections.initializePersistedConnections();
  sync.markLoaded();
  return { ...channel, state, projects, connections, savedQueries, provider, sync, sharedRepos };
}

type Page = Awaited<ReturnType<typeof openPage>>;

/** Another window's write: Core's event for it, with the library's current seq. */
function changedElsewhere(
  page: Page,
  kind: "connection" | "project" | "label" | "savedQuery",
  scope: string | null,
  ids: string[] | null,
) {
  page.emit({
    type: "storageChanged",
    kind,
    scope,
    ids,
    origin: "other-tab",
    seq: library.seq(),
  });
}

/** Let the 100 ms grouping pass and the refetch land. */
async function settle() {
  await vi.advanceTimersByTimeAsync(150);
  await vi.runAllTimersAsync();
}

beforeEach(() => {
  vi.useFakeTimers();
  toasts.length = 0;
  library = new RecordingLibrary();
  library.seedProject("p1", "Main");
  library.seedProject("p2", "Other");
  setLibrary(library);
});

afterEach(() => {
  setLibrary(null);
  vi.useRealTimers();
});

describe("other windows' changes", () => {
  it("another window's repo write refreshes the status here", async () => {
    const page = await openPage();
    page.emit({
      type: "storageChanged",
      kind: "sharedRepo",
      scope: null,
      ids: ["repo-1"],
      origin: "other-tab",
      seq: library.seq(),
    });
    await settle();
    expect(page.sharedRepos.refreshRepos).toHaveBeenCalledWith(["repo-1"]);
  });

  it("another tab's saved query appears without a reload", async () => {
    const page = await openPage();
    expect(page.state.queriesByProject.p1).toEqual([]);

    library.seedSavedQuery("saved-9", { name: "From B" });
    library.n += 1;
    changedElsewhere(page, "savedQuery", "p1", ["saved-9"]);
    await settle();

    expect(page.state.queriesByProject.p1.map((q) => q.name)).toEqual(["From B"]);
  });

  it("this tab's own write doesn't trigger a refetch", async () => {
    const page = await openPage();
    const { value, seq } = await library.createSavedQuery({
      projectId: "p1",
      name: "Mine",
      query: "SELECT 1",
    });
    library.calls.length = 0;
    page.emit({
      type: "storageChanged",
      kind: "savedQuery",
      scope: "p1",
      ids: [value.id],
      origin: "this-tab",
      seq,
    });
    await settle();
    expect(library.calls).toEqual([]);
  });

  it("a refetch older than this tab's write answer is dropped", async () => {
    const page = await openPage();
    library.seedConnection("c1", { name: "Old" });
    await page.connections.refreshFromLibrary(null);
    // A list taken before this page's rename committed…
    const stale = await library.listConnections();
    const staleRows = stale.value.map((c) => ({ ...c }));
    await page.connections.patch("c1", { name: "New" });
    // …arrives after the rename's answer: it's older, so it's dropped.
    page.connections.applyConnections(staleRows, stale.seq, null, true);
    expect(page.state.connections[0].name).toBe("New");
  });

  it("a refetch waits for this page's write to that row", async () => {
    const page = await openPage();
    library.seedConnection("c1", { name: "Old" });
    await page.connections.refreshFromLibrary(null);
    let release!: () => void;
    const gate = new Promise<void>((r) => (release = r));
    const real = library.updateConnection.bind(library);
    library.updateConnection = async (...args) => {
      await gate;
      return real(...args);
    };
    const lists = library.callsOf("listConnections").length;
    const write = page.connections.patch("c1", { name: "Mine" });
    const refetch = page.connections.refreshFromLibrary(["c1"]);
    await Promise.resolve();
    // The refetch hasn't read yet: it waits for the write.
    expect(library.callsOf("listConnections")).toHaveLength(lists);
    release();
    await Promise.all([write, refetch]);
    expect(page.state.connections[0].name).toBe("Mine");
  });

  it("a new epoch reloads every list", async () => {
    const page = await openPage();
    library.seedConnection("c1");
    library.epoch = "epoch-2";
    library.n = 1;
    library.calls.length = 0;
    changedElsewhere(page, "project", null, ["p2"]);
    await settle();
    const methods = library.calls.map((c) => c.method);
    expect(methods).toEqual(expect.arrayContaining(["listProjects", "listConnections"]));
    expect(page.state.connections.map((c) => c.id)).toEqual(["c1"]);
  });

  it("a socket reconnect reloads every list", async () => {
    const page = await openPage();
    library.seedConnection("c1");
    library.calls.length = 0;
    page.resubscribe(false);
    await settle();
    expect(library.calls.map((c) => c.method)).toEqual(
      expect.arrayContaining(["listProjects", "listConnections"]),
    );
    expect(page.state.connections.map((c) => c.id)).toEqual(["c1"]);
  });

  it("a resubscription during the first load reloads once the load is done", async () => {
    const channel = fakeClient();
    const state = new DatabaseState();
    const feed = new ChangeFeed({
      client: () => channel.client,
      origin: () => "this-tab",
      seqs: state.librarySeqs,
    });
    const views = {
      connections: { refreshFromLibrary: vi.fn(async () => {}) },
      projects: { refreshFromLibrary: vi.fn(async () => {}) },
      savedQueries: { refreshFromLibrary: vi.fn(async () => {}) },
      history: { reloadHistory: vi.fn(async () => {}) },
    };
    const sync = new LibrarySync(state, feed, views);
    sync.start();
    channel.resubscribe(true);
    expect(views.connections.refreshFromLibrary).not.toHaveBeenCalled();
    sync.markLoaded();
    expect(views.connections.refreshFromLibrary).toHaveBeenCalledWith(null);
    sync.stop();
  });

  it("a connection removed elsewhere disconnects here, with a toast", async () => {
    library.seedConnection("c1", { name: "Prod" });
    const page = await openPage();
    page.state.connections = page.state.connections.map((c) => ({
      ...c,
      providerConnectionId: "core-7",
    }));
    page.state.schemaTabsByProject = { p1: [{ id: "t1", connectionId: "c1" }] } as never;

    library.connections.delete("c1");
    library.n += 1;
    changedElsewhere(page, "connection", "p1", ["c1"]);
    await settle();

    expect(page.state.connections).toEqual([]);
    expect(page.provider.disconnect).toHaveBeenCalledWith("core-7");
    expect(page.state.schemaTabsByProject.p1).toEqual([]);
    expect(toasts).toEqual(['"Prod" was deleted in another window.']);
  });

  it("the active project removed elsewhere switches project, with a toast", async () => {
    const page = await openPage();
    await page.projects.setActive("p2");
    library.seedConnection("c2", { projectId: "p2" });
    await page.connections.refreshFromLibrary(null);

    library.projects.delete("p2");
    library.connections.delete("c2");
    library.n += 1;
    changedElsewhere(page, "project", null, ["p2"]);
    await settle();

    expect(page.state.projects.map((p) => p.id)).toEqual(["p1"]);
    expect(page.state.activeProjectId).toBe("p1");
    expect(page.state.connections).toEqual([]);
    expect(toasts).toEqual(['The project "Other" was deleted in another window.']);
  });

  it("the connection order is saved at once and appears in another tab", async () => {
    library.seedConnection("c1");
    library.seedConnection("c2");
    const a = await openPage();
    const b = await openPage();
    expect(b.state.projectConnections.map((c) => c.id)).toEqual(["c1", "c2"]);
    library.calls.length = 0;

    // Dragged in tab A: stored at once, not with a debounced tab save.
    a.connections.reorder("p1", ["c2", "c1"]);
    await vi.advanceTimersByTimeAsync(0);
    expect(library.callsOf("setProjectSidebar")).toEqual([["p1", ["c2", "c1"]]]);

    // Core's `project` event reaches tab B, which reads the order again.
    changedElsewhere(b, "project", null, ["p1"]);
    await settle();
    expect(b.state.connectionOrderByProject.p1).toEqual(["c2", "c1"]);
    expect(b.state.projectConnections.map((c) => c.id)).toEqual(["c2", "c1"]);
    // Tab A's own event is skipped (its origin), and its order stays.
    expect(a.state.connectionOrderByProject.p1).toEqual(["c2", "c1"]);
  });

  it("an older order read doesn't undo this tab's newer order", async () => {
    library.seedConnection("c1");
    library.seedConnection("c2");
    const page = await openPage();
    const stale = await library.getProjectSidebar("p1");
    page.connections.reorder("p1", ["c2", "c1"]);
    await vi.advanceTimersByTimeAsync(0);
    const { applyConnectionOrder } = await import("./view");
    applyConnectionOrder(page.state, "p1", stale.value, stale.seq);
    expect(page.state.connectionOrderByProject.p1).toEqual(["c2", "c1"]);
  });

  it("a view-state event of another window changes nothing here", async () => {
    const page = await openPage();
    page.state.queryTabsByProject.p1 = [{ id: "mine", name: "q", query: "", isExecuting: false }];
    library.calls.length = 0;
    page.emit({
      type: "storageChanged",
      kind: "projectState",
      scope: "p1",
      ids: ["other-tab"],
      origin: "other-tab",
      seq: { ...library.seq(), n: library.n + 1 },
    });
    await settle();
    expect(page.state.queryTabsByProject.p1.map((t) => t.id)).toEqual(["mine"]);
    expect(library.calls).toEqual([]);
  });

  it("a view-state event naming this window's id from another page reloads it", async () => {
    const page = await openPage();
    page.state.queryTabsByProject.p1 = [{ id: "mine", name: "q", query: "", isExecuting: false }];
    library.calls.length = 0;
    // A page of this window that wrote before its duplicate check settled.
    page.emit({
      type: "storageChanged",
      kind: "projectState",
      scope: "p1",
      ids: ["this-tab"],
      origin: "an-earlier-page",
      seq: { ...library.seq(), n: library.n + 1 },
    });
    await settle();
    // It is read again; the view state is off in these pages, so the read
    // gets nothing and, as a reload, leaves the tabs shown.
    expect(library.callsOf("listWorkflows")).toEqual([["p1"]]);
    expect(page.state.queryTabsByProject.p1.map((t) => t.id)).toEqual(["mine"]);
  });

  it("a saved query deleted elsewhere unlinks its tab and keeps the text", async () => {
    library.seedSavedQuery("q1", { name: "Orders" });
    const page = await openPage();
    page.state.queryTabsByProject = {
      p1: [{ id: "t1", name: "Orders", query: "SELECT 42", queryId: "q1" } as never],
    };

    library.savedQueries.delete("q1");
    library.n += 1;
    changedElsewhere(page, "savedQuery", "p1", ["q1"]);
    await settle();

    expect(page.state.queriesByProject.p1).toEqual([]);
    expect(page.state.queryTabsByProject.p1[0]).toMatchObject({ query: "SELECT 42" });
    expect(page.state.queryTabsByProject.p1[0].queryId).toBeUndefined();
  });

  it("a form's row changed elsewhere is counted for its banner; its save sends only its fields", async () => {
    library.seedConnection("c1", { name: "Local", port: 5432 });
    const page = await openPage();
    const key = "connection:c1";
    const opened = page.state.libraryRemoteRevision[key] ?? 0;
    const baseline = { name: "Local", type: "postgres", port: 5432 } as never;

    library.connections.get("c1")!.name = "Renamed elsewhere";
    library.n += 1;
    changedElsewhere(page, "connection", "p1", ["c1"]);
    await settle();

    expect(page.state.libraryRemoteRevision[key]).toBeGreaterThan(opened);
    // The form (opened before the rename) changes the port only.
    await page.connections.update(
      "c1",
      { name: "Local", type: "postgres", port: 6543, password: "" } as never,
      baseline,
    );
    expect(library.callsOf("updateConnection").at(-1)).toEqual(["c1", { port: 6543 }]);
    expect(library.connections.get("c1")).toMatchObject({
      name: "Renamed elsewhere",
      port: 6543,
    });
  });

  it("a label removed elsewhere disappears from connections", async () => {
    library.projects.get("p1")!.customLabels = [
      { id: "label-a", name: "A", color: "#ff0000", isPredefined: false },
    ];
    library.seedConnection("c1", { labelIds: ["prod", "label-a"] });
    const page = await openPage();

    await library.removeLabel("p1", "label-a");
    changedElsewhere(page, "label", "p1", ["label-a"]);
    await settle();

    expect(page.state.projects.find((p) => p.id === "p1")?.customLabels).toEqual([]);
    expect(page.state.connections[0].labelIds).toEqual(["prod"]);
  });

  it("shows when updates stop, until the channel is back", async () => {
    const page = await openPage();
    page.unavailable("ACCESS_LOST");
    expect(page.state.libraryUpdatesUnavailable).toBe("ACCESS_LOST");
    page.resubscribe(false);
    expect(page.state.libraryUpdatesUnavailable).toBeNull();
  });
});

describe("this page's writes record a seq only for what they read whole", () => {
  it("a local label create doesn't hide another tab's earlier rename", async () => {
    const page = await openPage();
    // Another tab renames p1 (n-1); its event hasn't arrived yet.
    library.projects.get("p1")!.name = "Renamed elsewhere";
    library.n += 1;
    const renameSeq = library.seq();
    // This page creates a label on p1 (n).
    await page.projects.addCustomLabel("p1", { name: "Mine", color: "#ff0000" });
    // Now the rename's event arrives.
    page.emit({
      type: "storageChanged",
      kind: "project",
      scope: null,
      ids: ["p1"],
      origin: "other-tab",
      seq: renameSeq,
    });
    await settle();
    const p1 = page.state.projects.find((p) => p.id === "p1")!;
    expect(p1.name).toBe("Renamed elsewhere");
    expect(p1.customLabels.map((l) => l.name)).toEqual(["Mine"]);
  });

  it("a local label remove doesn't hide another tab's earlier change to a stripped connection", async () => {
    library.projects.get("p1")!.customLabels = [
      { id: "label-a", name: "A", color: "#ff0000", isPredefined: false },
    ];
    library.seedConnection("c1", { name: "Local", labelIds: ["label-a"] });
    const page = await openPage();
    library.connections.get("c1")!.name = "Renamed elsewhere";
    library.n += 1;
    await page.projects.removeCustomLabel("p1", "label-a");
    expect(page.state.connections[0]).toMatchObject({ name: "Renamed elsewhere", labelIds: [] });
  });

  it("a local version doesn't hide another tab's earlier version of another query", async () => {
    library.seedSavedQuery("q1", { query: "SELECT 1" });
    library.seedSavedQuery("q2", { query: "SELECT a" });
    const page = await openPage();
    await page.projects.setActive("p2");
    await page.projects.setActive("p1");
    page.state.queryTabsByProject = {
      p1: [{ id: "t1", name: "q1", query: "", queryId: "q1" } as never],
    };
    // Another tab saves q2's text (a version at n-1); no event yet.
    await library.updateSavedQuery("q2", { query: "SELECT b" });
    // This page saves q1's text (a version at n).
    await page.savedQueries.saveQuery("q1", "SELECT 2", "t1");
    const shown = page.state.queryVersionsByProject.p1.map((v) => `${v.queryId}#${v.version}`);
    expect(shown.sort()).toEqual(["q1#1", "q2#1"]);
  });
});

describe("reconnect from a form", () => {
  it("stores only the fields the form changed since it opened", async () => {
    library.seedConnection("c1", { name: "Local", host: "localhost" });
    const page = await openPage();
    const baseline = { name: "Local", type: "postgres", host: "localhost", port: 5432 } as never;
    // Another tab renames it; this page's form (opened before) changes the host.
    library.connections.get("c1")!.name = "Theirs";
    await page.connections.reconnect(
      "c1",
      { name: "Local", type: "postgres", host: "db2", port: 5432, password: "" } as never,
      baseline,
    );
    expect(library.callsOf("updateConnection").at(-1)).toEqual([
      "c1",
      { host: "db2", connected: true },
    ]);
    expect(page.state.connections[0]).toMatchObject({ name: "Theirs", host: "db2" });
  });

  it("a refused save shows the stored fields again and keeps the connection open", async () => {
    library.seedConnection("c1", { name: "Local", host: "localhost" });
    const page = await openPage();
    library.failures.set("updateConnection", {
      error: Object.assign(new Error("NAME_TAKEN: taken"), { code: "NAME_TAKEN" }),
    });
    await page.connections.reconnect("c1", {
      name: "Taken",
      type: "postgres",
      host: "db2",
      port: 5432,
      databaseName: "app",
      username: "me",
      password: "",
    } as never);
    expect(page.state.connections[0]).toMatchObject({
      name: "Local",
      host: "localhost",
      providerConnectionId: "core-1",
    });
    expect(toasts).toHaveLength(1);
  });
});

describe("a connection removed elsewhere", () => {
  it("closes its tabs in every project, and its edit tab", async () => {
    library.seedConnection("c1", { name: "Prod" });
    const page = await openPage();
    const t = (id: string, extra = {}) => ({ id, connectionId: "c1", ...extra }) as never;
    page.state.schemaTabsByProject = { p1: [t("s1")], p2: [t("s2")] };
    page.state.dataTabsByProject = { p2: [t("d1"), { id: "d2", connectionId: "other" } as never] };
    page.state.erdTabsByProject = { p1: [t("e1")] };
    page.state.statisticsTabsByProject = { p1: [t("st1")] };
    page.state.connectionTabsByProject = { p1: [t("ct1")] };
    page.state.activeDataTabIdByProject = { p2: "d1" };
    page.state.tabOrderByProject = { p1: ["s1", "e1", "st1", "ct1", "q"], p2: ["s2", "d1", "d2"] };
    page.state.paneLayoutByProject = {
      p2: {
        panes: [
          { id: "a", tabIds: ["s2", "d1"], activeTabId: "d1" },
          { id: "b", tabIds: ["d2"], activeTabId: "d2" },
        ],
        activePaneId: "a",
      },
    };

    library.connections.delete("c1");
    library.n += 1;
    changedElsewhere(page, "connection", "p1", ["c1"]);
    await settle();

    const s = page.state;
    expect(s.schemaTabsByProject).toEqual({ p1: [], p2: [] });
    expect(s.dataTabsByProject.p2.map((x) => x.id)).toEqual(["d2"]);
    expect(s.erdTabsByProject.p1).toEqual([]);
    expect(s.statisticsTabsByProject.p1).toEqual([]);
    expect(s.connectionTabsByProject.p1).toEqual([]);
    expect(s.activeDataTabIdByProject.p2).toBe("d2");
    expect(s.tabOrderByProject).toEqual({ p1: ["q"], p2: ["d2"] });
    expect(s.paneLayoutByProject.p2).toEqual({
      panes: [{ id: "b", tabIds: ["d2"], activeTabId: "d2" }],
      activePaneId: "b",
    });
  });

  it("a local removal closes them too", async () => {
    library.seedConnection("c1");
    const page = await openPage();
    page.state.dataTabsByProject = { p2: [{ id: "d1", connectionId: "c1" } as never] };
    await page.connections.remove("c1");
    expect(page.state.dataTabsByProject.p2).toEqual([]);
  });
});

describe("follow-ups", () => {
  it("closing a removed connection's tabs moves the active view to the tab now shown", async () => {
    const { closeConnectionTabs } = await import("../connection-tabs-cleanup");
    const state = new DatabaseState();
    state.activeProjectId = "p1";
    state.schemaTabsByProject = { p1: [{ id: "s1", connectionId: "c1" } as never] };
    state.tabOrderByProject = { p1: ["s1", "q1"] };
    state.paneLayoutByProject = {
      p1: { panes: [{ id: "a", tabIds: ["s1", "q1"], activeTabId: "s1" }], activePaneId: "a" },
    };
    const sync = vi.fn();
    closeConnectionTabs(state, "c1", sync);
    expect(state.paneLayoutByProject.p1.panes[0].activeTabId).toBe("q1");
    expect(sync).toHaveBeenCalledWith("q1");
  });

  it("the connection manager wires that step to the pane manager", async () => {
    library.seedConnection("c1");
    const page = await openPage();
    const syncGlobalActiveState = vi.fn();
    (page.connections as unknown as { tabOrdering: unknown }).tabOrdering = {
      paneManager: { syncGlobalActiveState },
    };
    page.state.activeProjectId = "p1";
    page.state.schemaTabsByProject = { p1: [{ id: "s1", connectionId: "c1" } as never] };
    page.state.paneLayoutByProject = {
      p1: { panes: [{ id: "a", tabIds: ["s1", "q1"], activeTabId: "s1" }], activePaneId: "a" },
    };
    await page.connections.remove("c1");
    expect(syncGlobalActiveState).toHaveBeenCalledWith("q1");
  });

  it("a refused reconnect save ends with the newest stored row", async () => {
    library.seedConnection("c1", { name: "Local", host: "localhost", username: "me" });
    const page = await openPage();
    library.updateConnection = async () => {
      // Another tab changes the user while this save is refused.
      library.connections.get("c1")!.username = "theirs";
      library.n += 1;
      throw Object.assign(new Error("NAME_TAKEN: taken"), { code: "NAME_TAKEN" });
    };
    await page.connections.reconnect("c1", {
      name: "Taken",
      type: "postgres",
      host: "localhost",
      port: 5432,
      databaseName: "app",
      username: "me",
      password: "",
    } as never);
    expect(page.state.connections[0]).toMatchObject({
      name: "Local",
      username: "theirs",
      providerConnectionId: "core-1",
    });
  });
});
