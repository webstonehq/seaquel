/**
 * Never save what failed to load. Each case fails a load, triggers the save
 * that would follow, and checks no replacing write reached storage: those
 * saves delete every stored row the in-memory copy lacks, and after a failed
 * load the in-memory copy is empty.
 */
import { describe, it, expect, vi, beforeEach } from "vitest";

/** Every storage call, as `repo.method`. */
const calls: string[] = [];
/** Loads (`load*`, `get`) throw while this is true. */
let failLoads = true;
/** While set, loads wait for it before answering. */
let holdLoads: Promise<void> | null = null;

vi.mock("$lib/storage", () => {
  const repo = (name: string) =>
    new Proxy(
      {},
      {
        get: (_t, method: string) =>
          vi.fn(async () => {
            calls.push(`${name}.${method}`);
            if (/^(load|get)/.test(method)) {
              if (holdLoads) await holdLoads;
              if (failLoads) throw new Error("STORAGE_ERROR: upstream unavailable");
              if (method === "loadAll" && name === "sharedRepos") {
                return { repos: [], activeRepoId: null };
              }
              return method === "load" || method === "get" ? null : [];
            }
            return undefined;
          }),
      },
    );
  const storage = new Proxy({}, { get: (_t, name: string) => repo(name) });
  return { getStorage: () => storage };
});
const toasts: string[] = [];
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));
vi.mock("$lib/services/keyring", () => ({ getKeyringService: () => ({}) }));
vi.mock("$lib/stores/license-nudge.svelte.js", () => ({
  licenseNudgeStore: { recordQuery: () => {} },
}));

const { PersistenceManager } = await import("./persistence-manager.svelte.js");
const { DatabaseState } = await import("./state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { resetLoadGuardToast } = await import("$lib/storage/load-guard");
const { QueryHistoryManager } = await import("./query-history.svelte.js");

/** Writes that replace or delete stored rows. */
const REPLACING = [
  "projects.saveAll",
  "appState.set",
  "projectState.save",
  "savedQueries.saveAll",
  "sharedRepos.saveAll",
  "queryHistory.replaceAll",
  "aiChats.replaceAllMessages",
];
const replacingWrites = () => calls.filter((c) => REPLACING.includes(c));

function setup() {
  const state = new DatabaseState();
  const persistence = new PersistenceManager(state);
  const restoration = new StateRestorationManager(state, persistence);
  return { state, persistence, restoration };
}

beforeEach(() => {
  calls.length = 0;
  toasts.length = 0;
  failLoads = true;
  holdLoads = null;
  resetLoadGuardToast();
});

describe("a failed load blocks the save that would replace it", () => {
  it("project state: a tab change doesn't wipe the project's tabs and canvases", async () => {
    const { state, persistence } = setup();
    expect(await persistence.loadProjectState("p1")).toBeNull();
    state.queryTabsByProject["p1"] = [];

    await persistence.persistProjectState("p1");

    expect(calls).not.toContain("projectState.save");
    expect(replacingWrites()).toEqual([]);
    expect(toasts).toHaveLength(1);
  });

  it("saved queries: saving one query doesn't delete the project's others", async () => {
    const { state, persistence, restoration } = setup();
    await restoration.loadProjectData("p1"); // saved queries fail
    failLoads = false;
    await persistence.loadProjectState("p1"); // project state loads fine
    calls.length = 0;
    // The user saves a new query: it's now the only one in memory.
    const now = new Date();
    state.queriesByProject = {
      p1: [
        {
          id: "q-new",
          name: "new",
          query: "select 1",
          projectId: "p1",
          createdAt: now,
          updatedAt: now,
        } as never,
      ],
    };

    await persistence.persistProjectState("p1");

    expect(calls).toContain("projectState.save");
    expect(calls).not.toContain("savedQueries.saveAll");
  });

  it("saved queries: a later successful load lets saves through again", async () => {
    const { state, persistence, restoration } = setup();
    await restoration.loadProjectData("p1");
    failLoads = false;
    await restoration.loadProjectData("p1");
    await persistence.loadProjectState("p1");
    state.queriesByProject = { p1: [] };
    calls.length = 0;

    await persistence.persistProjectState("p1");

    expect(calls).toContain("savedQueries.saveAll");
  });

  it("shared repos: a failed load isn't saved back as none", async () => {
    const { persistence } = setup();
    expect(await persistence.loadSharedRepos()).toEqual({ repos: [], activeRepoId: null });

    await persistence.persistSharedRepos();

    expect(replacingWrites()).toEqual([]);
  });

  it("AI chat messages: a new message doesn't replace the chat's history", async () => {
    const { state, persistence } = setup();
    await persistence.loadAIChatMessages("chat-1");
    state.aiMessagesByChat = { "chat-1": [] };

    await persistence.persistAIChatMessages("chat-1");

    expect(replacingWrites()).toEqual([]);
  });

  it("projects: no default project is stored and the active project isn't overwritten", async () => {
    const { state, persistence, restoration } = setup();
    const projects = new ProjectManager(state, persistence, restoration);

    await projects.initialize();

    // The app still gets a project to work in, in memory only.
    expect(state.projects).toHaveLength(1);
    expect(calls).not.toContain("projects.saveAll");
    await persistence.persistAppState();
    await persistence.persistProjects();
    await persistence.persistProjectState(state.projects[0].id);
    expect(replacingWrites()).toEqual([]);
  });

  it("projects: an empty successful load still creates and stores the default project", async () => {
    failLoads = false;
    const { state, persistence, restoration } = setup();
    const projects = new ProjectManager(state, persistence, restoration);

    await projects.initialize();

    expect(state.projects).toHaveLength(1);
    expect(calls).toContain("projects.saveAll");
  });
});

describe("a load that is still running blocks the save too", () => {
  it("refuses AI-message saves until their load succeeds", async () => {
    failLoads = false;
    let release!: () => void;
    holdLoads = new Promise((r) => (release = r));
    const { state, persistence } = setup();
    state.aiMessagesByChat = { "chat-1": [] };

    const messages = persistence.loadAIChatMessages("chat-1");
    await persistence.persistAIChatMessages("chat-1");
    expect(replacingWrites()).toEqual([]);
    // Still loading, not failed: nothing to tell the user.
    expect(toasts).toEqual([]);

    release();
    holdLoads = null;
    await messages;
    await persistence.persistAIChatMessages("chat-1");
    expect(replacingWrites()).toEqual(["aiChats.replaceAllMessages"]);
  });
});

describe("query history has no replacing save to guard", () => {
  it("a failed history load doesn't stop appends", async () => {
    const { state, persistence } = setup();
    state.activeConnectionId = "c1";
    const history = new QueryHistoryManager(
      state,
      () => [],
      () => "c1",
    );

    expect(await persistence.loadConnectionData("c1")).toEqual({ queryHistory: [] });
    history.addToHistory("SELECT 1", {
      columns: [],
      rows: [],
      rowCount: 0,
      totalRows: 0,
      executionTime: 1,
      page: 1,
      pageSize: 100,
      totalPages: 1,
    });
    await new Promise((r) => setTimeout(r, 0));

    expect(calls).toContain("queryHistory.append");
    expect(replacingWrites()).toEqual([]);
    // Nothing is refused, so nothing is toasted.
    expect(toasts).toEqual([]);
  });

  it("flush writes no history", async () => {
    failLoads = false;
    const { state, persistence } = setup();
    state.queryHistoryByConnection = { c1: [] };
    await persistence.loadConnectionData("c1");
    calls.length = 0;

    await persistence.flush();

    expect(calls.filter((c) => c.startsWith("queryHistory."))).toEqual([]);
  });
});

describe("a chat whose messages failed to load", () => {
  it("stays unloaded, so switching to it loads it again", async () => {
    const { state, persistence, restoration } = setup();

    await restoration.loadAIChatMessages("chat-1");
    expect("chat-1" in state.aiMessagesByChat).toBe(false);
    expect(persistence.loadFailed("aiMessages:chat-1")).toBe(true);

    failLoads = false;
    await restoration.loadAIChatMessages("chat-1");
    expect(state.aiMessagesByChat["chat-1"]).toEqual([]);
    expect(persistence.loadFailed("aiMessages:chat-1")).toBe(false);
  });
});
