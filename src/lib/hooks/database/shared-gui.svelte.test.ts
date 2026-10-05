/**
 * The GUI over Core's shared projection and imports. The
 * managers run as `UseDatabase` wires them, over a recording library (whose
 * write answers can carry a `projection` outcome), a recording
 * `SharedService` and `ImportsService`, and a git stub. No manager under
 * `src/lib/hooks` imports the fs plugin (the last test scans them), so none
 * can touch a file.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RepoStatus } from "$lib/types";

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => true,
  isWeb: () => false,
  isDemo: () => false,
}));
const git = vi.hoisted(() => ({
  status: {} as Partial<RepoStatus>,
  statusFails: false,
  pull: { success: true, message: "", conflicts: [] as string[], filesChanged: [] as string[] },
  calls: [] as string[],
}));
vi.mock("$lib/services/git", () => ({
  getRepoStatus: vi.fn(async () => {
    git.calls.push("status");
    if (git.statusFails)
      throw Object.assign(new Error("GIT_ERROR: no repo"), { code: "GIT_ERROR" });
    return {
      isClean: true,
      pendingChanges: 0,
      aheadBy: 0,
      behindBy: 0,
      hasConflicts: false,
      currentBranch: "main",
      modifiedFiles: [],
      untrackedFiles: [],
      conflictFiles: [],
      ...git.status,
    };
  }),
  pullRepo: vi.fn(async () => {
    git.calls.push("pull");
    return git.pull;
  }),
  pushRepo: vi.fn(async () => ({ success: true, message: "", conflicts: [], filesChanged: [] })),
  commitChanges: vi.fn(async () => {
    git.calls.push("commit");
    return "abc";
  }),
  resolveConflict: vi.fn(async () => {
    git.calls.push("resolve");
  }),
  setRemote: vi.fn(async () => {}),
  getRemoteUrl: vi.fn(async () => null),
  cloneRepo: vi.fn(async () => {}),
  initRepo: vi.fn(async () => {}),
}));
vi.mock("$lib/storage", () => ({
  getStorage: () => ({ queryHistory: { loadByConnection: async () => [] } }),
}));
vi.mock("$lib/engine", () => ({
  getEngineClient: () => ({ schemaTables: async () => [] }),
}));
vi.mock("$lib/services/keyring", () => ({
  getKeyringService: () => ({ isAvailable: () => false }),
}));
const toasts = vi.hoisted(() => ({ errors: [] as string[], other: [] as string[] }));
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.errors.push(m) }));
vi.mock("svelte-sonner", () => ({
  toast: {
    success: (m: string) => toasts.other.push(m),
    info: (m: string) => toasts.other.push(m),
    warning: (m: string) => toasts.other.push(m),
  },
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { DatabaseState } = await import("./state.svelte.js");
const { WindowStateManager } = await import("./window-state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { ConnectionManager } = await import("./connection-manager.svelte.js");
const { SavedQueryManager } = await import("./saved-queries.svelte.js");
const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { SharedRepoManager } = await import("./shared-repo-manager.svelte.js");
const { RecordingLibrary } = await import("./library/recording-library");
const { setLibrary } = await import("./library/index");
const { setShared, setImports } = await import("./shared/index");
const { setProjectionHooks } = await import("./shared/projection");
const { linkDialogSelection } = await import("$lib/stores/link-project-dialog.svelte");
const { ConnectionImportStore } = await import("$lib/stores/connection-import.svelte");
const { noticeText, failureText } = await import("./shared/notices");
const { projectSyncStatus } = await import("$lib/components/shared-queries/project-sync-status");
const { isSharedConnection } = await import("$lib/components/sidebar/manage/share-link");
const { sharedProjectImportStore } = await import("$lib/stores/shared-project-import.svelte");
const { importSharedProjectsFrom } = await import("$lib/services/shared-project-import");
const { m } = await import("$lib/paraglide/messages.js");

import type {
  ImportCandidates,
  ImportsService,
  RepoPreview,
  SharedService,
  SyncReport,
} from "./shared/types";

const REPO = {
  id: "repo-1",
  name: "Team",
  path: "/repos/team",
  remoteUrl: "",
  branch: "main",
  lastSyncAt: null,
  syncStatus: "synced" as const,
};

function report(extra: Partial<SyncReport> = {}): SyncReport {
  return { conflicted: false, rowsChanged: 0, filesWritten: 0, notices: [], ...extra };
}

/** A `SharedService` that records each call and answers what the test sets. */
function recordingShared() {
  const seq = () => ({ value: null, seq: { epoch: "e", n: 1 } });
  const fake = {
    calls: [] as { method: string; args: unknown[] }[],
    syncReport: report(),
    linkReport: report(),
    unlink: {
      removedConnectionIds: [] as string[],
      keptConnectionIds: [] as string[],
      repoRemoved: false,
    },
    repos: [REPO],
    /** What Core says an unlink would remove. */
    preview: [] as string[],
    scanAnswer: { conflicted: false, projects: [] } as RepoPreview,
  };
  const rec =
    <T>(method: string, answer: (...args: unknown[]) => T) =>
    async (...args: unknown[]) => {
      fake.calls.push({ method, args });
      return answer(...args);
    };
  const service: SharedService = {
    listRepos: rec("listRepos", () => ({ ...seq(), value: fake.repos })),
    registerRepo: rec("registerRepo", () => ({ ...seq(), value: REPO })),
    updateRepo: rec("updateRepo", () => ({ ...seq(), value: REPO })),
    removeRepo: rec("removeRepo", () => seq()),
    linkProject: rec("linkProject", () => ({ ...seq(), value: fake.linkReport })),
    unlinkProject: rec("unlinkProject", () => ({ ...seq(), value: fake.unlink })),
    unlinkPreview: rec("unlinkPreview", () => ({ importedConnectionIds: fake.preview })),
    scan: rec("scan", () => fake.scanAnswer),
    importProjects: rec("importProjects", () => ({ ...seq(), value: { projectIds: [] } })),
    sync: rec("sync", () => ({ ...seq(), value: fake.syncReport })),
  };
  return { fake, service, of: (method: string) => fake.calls.filter((c) => c.method === method) };
}

let library: InstanceType<typeof RecordingLibrary>;
let shared: ReturnType<typeof recordingShared>;

beforeEach(() => {
  toasts.errors.length = 0;
  toasts.other.length = 0;
  git.status = {};
  git.statusFails = false;
  git.pull = { success: true, message: "", conflicts: [], filesChanged: [] };
  git.calls.length = 0;
  library = new RecordingLibrary();
  library.seedProject("p1", "Team", { gitRepoPath: "/repos/team" });
  library.seedProject("p2", "Ops", { gitRepoPath: "/repos/team" });
  library.seedProject("p3", "Solo");
  setLibrary(library);
  shared = recordingShared();
  setShared(shared.service);
});

afterEach(() => {
  setLibrary(null);
  setShared(null);
  setImports(null);
  setProjectionHooks(null);
});

/** The managers as `UseDatabase` wires them. */
async function open() {
  const state = new DatabaseState();
  const windowState = new WindowStateManager(state, { enabled: false });
  const restoration = new StateRestorationManager(state);
  const dashboards = new DashboardManager(
    state,
    async () => [],
    () => {},
  );
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
  const savedQueries = new SavedQueryManager(state, () => {});
  const sharedRepos = new SharedRepoManager(state);
  sharedRepos.setViews({
    refreshProjectRows: async (projectId) => {
      await Promise.all([
        savedQueries.refreshFromLibrary(projectId, null),
        dashboards.refreshFromLibrary(projectId, null),
        connections.refreshFromLibrary(null, { remote: false }),
        projects.refreshFromLibrary([projectId]),
      ]);
    },
  });
  projects.setConnectionManager(connections);
  projects.setSharedRepoManager(sharedRepos);
  setProjectionHooks({
    rowsChanged: (projectId) => sharedRepos.refreshAfterProjection(projectId),
    filesChanged: (projectId) => sharedRepos.refreshProjectStatus(projectId),
  });
  await projects.initialize();
  await connections.initializePersistedConnections();
  await sharedRepos.loadRepos();
  return { state, projects, connections, savedQueries, dashboards, sharedRepos };
}

describe("saved queries in a shared project", () => {
  it("sharing a query calls Core once and touches no file", async () => {
    library.seedSavedQuery("saved-1", { projectId: "p1", name: "Orders" });
    const page = await open();
    library.calls.length = 0;

    await page.savedQueries.shareQuery("saved-1");

    expect(library.callsOf("updateSavedQuery")).toEqual([["saved-1", { shared: true }]]);
    expect(library.calls.map((c) => c.method)).toEqual(["updateSavedQuery"]);
    expect(page.state.queriesByProject.p1.find((q) => q.id === "saved-1")?.shared).toBe(true);
  });

  it("a failed projection is said and the query stays saved", async () => {
    library.seedSavedQuery("saved-1", { projectId: "p1", name: "Orders", query: "SELECT 1" });
    const page = await open();
    const update = library.updateSavedQuery.bind(library);
    library.updateSavedQuery = async (id, patch) => ({
      ...(await update(id, patch)),
      projection: { status: "failed", code: "FILE_ERROR", message: "queries/orders.sql: denied" },
    });

    await page.savedQueries.shareQuery("saved-1");

    expect(toasts.errors).toEqual([
      m.shared_file_write_failed({ message: "queries/orders.sql: denied" }),
    ]);
    expect(library.savedQueries.get("saved-1")?.shared).toBe(true);
    expect(page.state.queriesByProject.p1.find((q) => q.id === "saved-1")?.shared).toBe(true);
  });

  it("a teammate's change on an edit says the repo's version is shown and reads the rows again", async () => {
    library.seedSavedQuery("saved-1", { projectId: "p1", name: "Orders", shared: true });
    const page = await open();
    const update = library.updateSavedQuery.bind(library);
    library.updateSavedQuery = async (id, patch) => ({
      ...(await update(id, patch)),
      projection: { status: "failed", code: "FILE_CHANGED", message: "changed" },
    });
    library.calls.length = 0;

    await page.savedQueries.toggleQueryStarred("saved-1");
    await vi.waitFor(() => expect(library.callsOf("listSavedQueries").length).toBeGreaterThan(0));

    expect(toasts.other).toContain(m.shared_file_changed_edit());
    expect(toasts.errors).toEqual([]);
  });

  it("a teammate's change on a delete says the repo's version comes back", async () => {
    library.seedSavedQuery("saved-1", { projectId: "p1", name: "Orders", shared: true });
    const page = await open();
    const remove = library.removeSavedQuery.bind(library);
    library.removeSavedQuery = async (id) => ({
      ...(await remove(id)),
      projection: { status: "failed", code: "FILE_CHANGED", message: "changed" },
    });

    await page.savedQueries.deleteQuery("saved-1");

    expect(toasts.other).toContain(m.shared_file_changed_removed());
    expect(toasts.errors).toEqual([]);
  });
});

describe("git and the sync", () => {
  it("a pull syncs every project linked to the repo", async () => {
    const page = await open();
    shared.fake.calls.length = 0;

    await page.sharedRepos.pullRepo("repo-1");

    expect(git.calls).toContain("pull");
    expect(shared.of("sync").map((c) => c.args)).toEqual([[{ repoId: "repo-1" }]]);
  });

  it("a commit and a resolve sync the repo too", async () => {
    const page = await open();
    shared.fake.calls.length = 0;

    await page.sharedRepos.commitChanges("repo-1", "msg");
    await page.sharedRepos.resolveConflict("repo-1", "a.sql", "x");

    expect(shared.of("sync").map((c) => c.args)).toEqual([
      [{ repoId: "repo-1" }],
      [{ repoId: "repo-1" }],
    ]);
  });

  it("a conflicted sync opens the conflict dialog", async () => {
    const page = await open();
    shared.fake.syncReport = report({ conflicted: true });
    git.status = { hasConflicts: true, conflictFiles: [".seaquel/projects/team/queries/a.sql"] };

    await page.sharedRepos.syncProject("p1");

    expect(page.state.sharedConflict).toEqual({
      repoId: "repo-1",
      files: [".seaquel/projects/team/queries/a.sql"],
    });
  });

  it("activating a linked project syncs it, and an unlinked one doesn't", async () => {
    const page = await open();
    shared.fake.calls.length = 0;

    await page.projects.setActive("p2");
    await page.projects.setActive("p3");

    expect(shared.of("sync").map((c) => c.args)).toEqual([[{ projectId: "p2" }]]);
  });

  it("another process's write reads the repo list again but runs no git status", async () => {
    const page = await open();
    git.calls.length = 0;
    shared.fake.calls.length = 0;

    await page.sharedRepos.refreshRepos(null, { status: false });

    expect(git.calls).toEqual([]);
    expect(shared.of("listRepos").length).toBe(1);
    // Without the option, as for another window's repo write, it does.
    await page.sharedRepos.refreshRepos(null);
    expect(git.calls).toContain("status");
  });

  it("a full reload keeps an unchanged dashboard's object", async () => {
    const page = await open();
    page.state.dashboardsByProject = { ...page.state.dashboardsByProject, p1: [] };
    await library.createDashboard({
      projectId: "p1",
      name: "Sales",
      viewport: { x: 0, y: 0, zoom: 1 },
      widgets: [{ id: "w1", type: "table", title: "Orders", sql: "SELECT 1" }],
    } as never);
    await page.dashboards.refreshFromLibrary("p1", null);
    const before = page.state.dashboardsByProject.p1[0];
    expect(before.name).toBe("Sales");

    // Another process wrote something else: the reload's answer is newer.
    library.n += 1;
    await page.dashboards.refreshFromLibrary("p1", null);
    expect(page.state.dashboardsByProject.p1[0]).toBe(before);
    expect(page.state.dashboardsByProject.p1[0].widgets).toBe(before.widgets);
  });

  it("a status that can't be read shows on the repo", async () => {
    const page = await open();
    git.statusFails = true;

    await page.sharedRepos.refreshRepoStatus("repo-1");

    expect(page.state.syncStateByRepo["repo-1"]?.statusUnreadable).toBe(true);
    expect(page.state.syncStateByRepo["repo-1"]?.lastError).toContain("no repo");
  });

  it("a sync that changed rows reads the project's rows again", async () => {
    const page = await open();
    shared.fake.syncReport = report({ rowsChanged: 1 });
    library.seedSavedQuery("saved-9", { projectId: "p1", name: "From the repo", shared: true });

    await page.sharedRepos.syncProject("p1");

    expect(page.state.queriesByProject.p1?.map((q) => q.id)).toContain("saved-9");
  });
});

describe("a project skipped whole (probe fix 7)", () => {
  const skippedReport = () =>
    report({
      notices: [{ type: "skipped", path: "projects/team", why: "tooMany" }],
      skippedProjects: [{ projectId: "p1", why: "tooMany" }],
    });

  it("is said on every sync and marked on the project until a sync reads it", async () => {
    const page = await open();
    shared.fake.syncReport = skippedReport();

    await page.sharedRepos.syncProject("p1");
    await page.sharedRepos.syncProject("p1");

    const said = toasts.other.filter((t) => t.includes("projects/team"));
    expect(said).toHaveLength(2);
    expect(page.state.sharedSyncSkipped.p1).toBe("tooMany");
    expect(projectSyncStatus("synced", page.state.sharedSyncSkipped.p1)).toBe("skipped");

    shared.fake.syncReport = report();
    await page.sharedRepos.syncProject("p1");
    expect(page.state.sharedSyncSkipped.p1).toBeUndefined();
    expect(projectSyncStatus("synced", page.state.sharedSyncSkipped.p1)).toBe("synced");
  });
});

describe("linking", () => {
  it("linking a project sends one call", async () => {
    const page = await open();
    library.calls.length = 0;
    shared.fake.calls.length = 0;
    library.projects.set("p3", { ...library.projects.get("p3")!, gitRepoPath: "/repos/team" });
    library.n += 1;

    await page.projects.linkProject("p3", "/repos/team", ["conn-1"]);

    expect(
      shared.fake.calls.filter((c) => c.method !== "listRepos" && c.method !== "sync"),
    ).toEqual([{ method: "linkProject", args: ["p3", "/repos/team", ["conn-1"]] }]);
    expect(library.callsOf("updateProject")).toEqual([]);
    expect(page.state.projects.find((p) => p.id === "p3")?.gitRepoPath).toBe("/repos/team");
  });

  it("linking sends only the ticked connections", () => {
    const selection = linkDialogSelection([
      { id: "conn-1", name: "Warehouse", isLocalOnly: false },
      { id: "conn-2", name: "Billing", isLocalOnly: false },
      { id: "conn-3", name: "Mine", isLocalOnly: true },
    ]);
    expect(selection.ticked()).toEqual(["conn-1", "conn-2"]);
    selection.toggle("conn-2");
    selection.toggle("conn-3");
    expect(selection.ticked()).toEqual(["conn-1", "conn-3"]);
  });

  it("unlinking removes the imported connections from the page", async () => {
    library.seedConnection("conn-1", {
      projectId: "p1",
      sharedConnectionId: "repo-1:.seaquel/projects/team/connections/w.yaml",
    });
    library.seedConnection("conn-2", { projectId: "p1" });
    const page = await open();
    shared.fake.unlink = {
      removedConnectionIds: ["conn-1"],
      keptConnectionIds: [],
      repoRemoved: false,
    };
    library.connections.delete("conn-1");
    library.projects.set("p1", { ...library.projects.get("p1")!, gitRepoPath: undefined });
    library.n += 1;

    await page.projects.unlinkProject("p1", true);

    expect(shared.of("unlinkProject").map((c) => c.args)).toEqual([["p1", true]]);
    expect(page.state.connections.map((c) => c.id)).toEqual(["conn-2"]);
    expect(page.state.projects.find((p) => p.id === "p1")?.gitRepoPath).toBeUndefined();
  });
});

describe("unlinking (Q31)", () => {
  const TPL = "repo-1:.seaquel/projects/team/connections";
  function seedLinked() {
    library.seedConnection("conn-own", {
      projectId: "p1",
      name: "Own",
      sharedConnectionId: `${TPL}/own.yaml`,
      sharedOrigin: "exported",
    });
    library.seedConnection("conn-imp", {
      projectId: "p1",
      name: "Theirs",
      sharedConnectionId: `${TPL}/theirs.yaml`,
      sharedOrigin: "imported",
    });
    // Linked by an older release: it came from the repo.
    library.seedConnection("conn-old", {
      projectId: "p1",
      name: "Old",
      sharedConnectionId: `${TPL}/old.yaml`,
    });
    library.seedConnection("conn-local", { projectId: "p1", name: "Mine", isLocalOnly: true });
  }

  it("the dialog lists the imported connections; cancel does nothing", async () => {
    seedLinked();
    // Core's list: what an unlink would remove (another directory's or
    // repo's links aren't this project's to remove).
    shared.fake.preview = ["conn-imp", "conn-old"];
    const page = await open();
    let listed: string[] = [];

    const done = await page.projects.unlinkWithConfirmation("p1", async (_project, imported) => {
      listed = imported.map((c) => c.name);
      return null;
    });

    expect(done).toBe(false);
    expect(shared.of("unlinkPreview").map((c) => c.args)).toEqual([["p1"]]);
    expect(listed).toEqual(["Theirs", "Old"]);
    expect(shared.of("unlinkProject")).toEqual([]);
    expect(page.state.connections).toHaveLength(4);
  });

  it("removing them sends the choice and keeps the user's own, unlinked", async () => {
    seedLinked();
    shared.fake.preview = ["conn-imp", "conn-old"];
    const page = await open();
    shared.fake.unlink = {
      removedConnectionIds: ["conn-imp", "conn-old"],
      keptConnectionIds: ["conn-own"],
      repoRemoved: false,
    };
    // As Core leaves the rows.
    library.connections.delete("conn-imp");
    library.connections.delete("conn-old");
    const own = library.connections.get("conn-own")!;
    library.connections.set("conn-own", {
      ...own,
      sharedConnectionId: undefined,
      sharedOrigin: undefined,
      isLocalOnly: true,
    });
    library.n += 1;

    const done = await page.projects.unlinkWithConfirmation("p1", async () => "remove");

    expect(done).toBe(true);
    expect(shared.of("unlinkProject").map((c) => c.args)).toEqual([["p1", true]]);
    expect(page.state.connections.map((c) => c.id).sort()).toEqual(["conn-local", "conn-own"]);
    const kept = page.state.connections.find((c) => c.id === "conn-own")!;
    expect(kept.sharedConnectionId).toBeUndefined();
    expect(kept.isLocalOnly).toBe(true);
  });

  it("with nothing imported, no dialog and nothing removed", async () => {
    library.seedConnection("conn-own", {
      projectId: "p1",
      name: "Own",
      sharedConnectionId: `${TPL}/own.yaml`,
      sharedOrigin: "exported",
    });
    const page = await open();
    const ask = vi.fn(async () => "remove" as const);

    expect(await page.projects.unlinkWithConfirmation("p1", ask)).toBe(true);

    expect(ask).not.toHaveBeenCalled();
    expect(shared.of("unlinkProject").map((c) => c.args)).toEqual([["p1", false]]);
  });
});

describe("the unlink dialog's list (re-review minor 1)", () => {
  it("lists only what Core says it would remove", async () => {
    library.seedConnection("conn-elsewhere", {
      projectId: "p1",
      name: "Elsewhere",
      sharedConnectionId: "repo-z:.seaquel/projects/other/connections/x.yaml",
    });
    library.seedConnection("conn-imp", {
      projectId: "p1",
      name: "Theirs",
      sharedConnectionId: "repo-1:.seaquel/projects/team/connections/theirs.yaml",
      sharedOrigin: "imported",
    });
    shared.fake.preview = ["conn-imp"];
    const page = await open();
    let listed: string[] = [];

    await page.projects.unlinkWithConfirmation("p1", async (_name, imported) => {
      listed = imported.map((c) => c.name);
      return "keep";
    });

    expect(listed).toEqual(["Theirs"]);
    expect(shared.of("unlinkProject").map((c) => c.args)).toEqual([["p1", false]]);
  });
});

describe("the shared switch (I2)", () => {
  it("shares an unshared connection in one click, and unshares a linked one", async () => {
    library.seedConnection("conn-a", { projectId: "p1", name: "A" });
    library.seedConnection("conn-b", {
      projectId: "p1",
      name: "B",
      sharedConnectionId: "repo-1:.seaquel/projects/team/connections/b.yaml",
    });
    const page = await open();
    expect(isSharedConnection(page.state.connections.find((c) => c.id === "conn-a")!)).toBe(false);
    expect(isSharedConnection(page.state.connections.find((c) => c.id === "conn-b")!)).toBe(true);
    library.calls.length = 0;

    await page.connections.toggleLocalOnly("conn-a");
    await page.connections.toggleLocalOnly("conn-b");

    expect(library.callsOf("updateConnection")).toEqual([
      ["conn-a", { isLocalOnly: false }],
      ["conn-b", { isLocalOnly: true }],
    ]);
  });
});

describe("pull and background refresh", () => {
  it("a pull that conflicts says so, not that the repo was updated", async () => {
    const page = await open();
    git.pull = { success: false, message: "", conflicts: ["a.sql"], filesChanged: [] };

    expect(await page.sharedRepos.pullRepo("repo-1")).toBe("conflicted");
    expect(page.state.sharedConflict).toEqual({ repoId: "repo-1", files: ["a.sql"] });

    git.pull = { success: true, message: "", conflicts: [], filesChanged: [] };
    expect(await page.sharedRepos.pullRepo("repo-1")).toBe("updated");
  });

  it("linking starts the background refresh", async () => {
    const page = await open();
    expect(page.sharedRepos.refreshing).toBe(false);
    library.projects.set("p3", { ...library.projects.get("p3")!, gitRepoPath: "/repos/team" });
    library.n += 1;

    await page.projects.linkProject("p3", "/repos/team", []);

    expect(page.sharedRepos.refreshing).toBe(true);
    page.sharedRepos.stopBackgroundRefresh();
  });
});

describe("the shared-project import dialog", () => {
  it("marks directories already linked here, unticked and not tickable", () => {
    const preview = (dir: string, linked: string[]) => ({
      dir,
      name: dir,
      queries: 0,
      dashboards: 0,
      templates: [],
      skipped: 0,
      linkedProjectIds: linked,
    });
    sharedProjectImportStore.openWithResults("/repos/team", [
      preview("team", ["p1"]),
      preview("ops", []),
    ]);
    const shown = sharedProjectImportStore.discoveredProjects;
    expect(shown.map((p) => [p.dir, p.selected, p.alreadyLinked])).toEqual([
      ["team", false, true],
      ["ops", true, false],
    ]);
    sharedProjectImportStore.toggleProject(0);
    sharedProjectImportStore.selectAll();
    expect(sharedProjectImportStore.discoveredProjects[0].selected).toBe(false);
    sharedProjectImportStore.reset();
  });
});

describe("a scan that skipped a symlinked folder (probe fix 8)", () => {
  it("names the folder", async () => {
    const page = await open();
    shared.fake.scanAnswer = {
      conflicted: false,
      projects: [],
      skippedDirs: [{ dir: "elsewhere", why: "symlink" }],
    };

    await importSharedProjectsFrom(page as never, "/repos/team");

    expect(toasts.other).toContain(
      m.shared_import_skipped_dirs({ dirs: "elsewhere", reason: m.shared_skip_symlink() }),
    );
    expect(toasts.other).toContain(m.shared_import_none_found());
  });
});

describe("imports", () => {
  function recordingImports(answer: ImportCandidates) {
    const calls: unknown[][] = [];
    const service: ImportsService = {
      candidates: async (...args) => {
        calls.push(["candidates", ...args]);
        return answer;
      },
      create: async (...args) => {
        calls.push(["create", ...args]);
        return {
          value: {
            results: [
              { key: "id:1", status: "imported", id: "conn-new" },
              { key: "id:2", status: "notFound" },
            ],
          },
          seq: { epoch: "e", n: 9 },
        };
      },
    };
    return { calls, service };
  }

  it("the TablePlus dialog says when nothing is found", async () => {
    const imports = recordingImports({ found: false });
    setImports(imports.service);
    const store = new ConnectionImportStore("tableplus");

    await store.checkAndShowDialog("p1");

    expect(store.isOpen).toBe(false);
    expect(toasts.other).toEqual([m.import_nothing_found({ tool: "TablePlus" })]);
  });

  it("a file that can't be read is said", async () => {
    const imports = recordingImports({ found: true, unreadable: "not a list" });
    setImports(imports.service);
    const store = new ConnectionImportStore("dbeaver");

    await store.checkAndShowDialog("p1");

    expect(store.isOpen).toBe(false);
    expect(toasts.errors).toEqual([
      m.import_unreadable({ tool: "DBeaver", message: "not a list" }),
    ]);
  });

  it("an import names each failure with its reason", async () => {
    const candidate = {
      host: "h",
      port: 5432,
      databaseName: "d",
      username: "u",
      type: "postgres" as const,
    };
    const imports = recordingImports({
      found: true,
      candidates: [
        { key: "id:1", name: "Good", ...candidate },
        { key: "id:2", name: "Gone", ...candidate },
        { key: "id:3", name: "Bad port", ...candidate, port: 0, problem: "invalidPort" },
      ],
    });
    setImports(imports.service);
    library.seedConnection("conn-new", { projectId: "p1", name: "Good" });
    const page = await open();
    const store = new ConnectionImportStore("tableplus");
    await store.checkAndShowDialog("p1");
    // A candidate with a problem can't be ticked.
    expect(store.candidates.map((c) => c.selected)).toEqual([true, true, false]);

    const result = await page.connections.importConnections("tableplus", "p1", store.selected());

    expect(imports.calls.at(-1)).toEqual(["create", "tableplus", "p1", ["id:1", "id:2"]]);
    expect(result.imported).toBe(1);
    expect(result.failures).toEqual([{ name: "Gone", reason: m.import_failure_not_found() }]);
    expect(page.state.connections.map((c) => c.id)).toContain("conn-new");
    expect(store.problemText(store.candidates[2])).toBe(m.import_problem_invalid_port());
  });
});

describe("an import of a folder already linked here (probe fix 8)", () => {
  it("names the folder with the already-linked wording", async () => {
    const page = await open();
    const text = failureText(page.state, {
      dir: "team",
      code: "PROJECT_ALREADY_LINKED",
      message: "A project here is already linked to this folder.",
    });
    expect(text).toBe(
      m.shared_project_sync_failed({
        name: "team",
        message: m.shared_import_dir_already_linked(),
      }),
    );
  });
});

describe("notices", () => {
  it("a taken name names the right kind of row", async () => {
    const page = await open();
    page.state.queriesByProject = {
      p1: [
        {
          id: "saved-1",
          name: "Orders",
          query: "",
          projectId: "p1",
          shared: false,
          createdAt: new Date(),
          updatedAt: new Date(),
        },
      ],
    };
    const text = noticeText(page.state, {
      type: "nameTaken",
      path: "projects/team/queries/orders.sql",
      takenBy: "saved-1",
    });
    expect(text).toBe(
      m.shared_notice_name_taken_saved_query({
        path: "projects/team/queries/orders.sql",
        name: "Orders",
      }),
    );
  });
});

describe("a template's replaced values", () => {
  it("are named in the user's language", async () => {
    const page = await open();
    const text = noticeText(page.state, {
      type: "conflict",
      kind: "connection",
      id: "conn-x",
      replaced: { host: "old.internal", port: 5433 },
    });
    expect(text).toBe(
      m.shared_notice_conflict_connection({
        name: "conn-x",
        values: `${m.shared_field_host()} old.internal, ${m.shared_field_port()} 5433`,
      }),
    );
  });
});

describe("the main window's file access", () => {
  it("no manager under src/lib/hooks imports the fs plugin", () => {
    const root = join(process.cwd(), "src/lib/hooks");
    const plugin = ["@tauri-apps", "plugin-fs"].join("/");
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const name of readdirSync(dir)) {
        const path = join(dir, name);
        if (statSync(path).isDirectory()) {
          if (name !== "node_modules") walk(path);
        } else if (/\.(ts|svelte)$/.test(name) && !name.endsWith(".test.ts")) {
          if (readFileSync(path, "utf8").includes(plugin)) offenders.push(path);
        }
      }
    };
    walk(root);
    expect(offenders).toEqual([]);
  });
});
