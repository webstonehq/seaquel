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
const { setLibrary } = await import("./library/index");
const { SavedQueryManager } = await import("./saved-queries.svelte.js");

/**
 * The library (phase 5d-1): every call is recorded as `library.method`;
 * lists fail while `failLoads`, and writes answer a row like Core's.
 */
const seq = { epoch: "e1", n: 1 };
setLibrary(
  new Proxy({} as never, {
    get:
      (_t, method: string) =>
      async (...args: unknown[]) => {
        calls.push(`library.${method}`);
        if (/^list|^ensure/.test(method)) {
          if (failLoads) throw new Error("STORAGE_ERROR: upstream unavailable");
          return {
            value:
              method === "ensureDefaultProject"
                ? [
                    {
                      id: "default-seaquel",
                      name: "Seaquel",
                      createdAt: "2026-01-01T00:00:00.000Z",
                      updatedAt: "2026-01-01T00:00:00.000Z",
                      customLabels: [],
                    },
                  ]
                : [],
            seq,
          };
        }
        if (method === "createSavedQuery") {
          const draft = args[0] as { projectId: string; name: string; query: string };
          return {
            value: {
              id: "saved-new",
              ...draft,
              createdAt: "2026-01-01T00:00:00.000Z",
              updatedAt: "2026-01-01T00:00:00.000Z",
              starred: false,
              shared: false,
            },
            seq: { ...seq, n: 2 },
          };
        }
        throw new Error(`unexpected library.${method}`);
      },
  }),
);
const { DatabaseState } = await import("./state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { resetLoadGuardToast } = await import("$lib/storage/load-guard");

/** Writes that replace or delete stored rows. */
const REPLACING = [
  "appState.set",
  "projectState.save",
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

  it("saved queries: a failed load leaves the page's copy, and saving one writes only it", async () => {
    const { state, persistence, restoration } = setup();
    await restoration.loadProjectData("p1"); // saved queries fail
    failLoads = false;
    await persistence.loadProjectState("p1"); // project state loads fine
    state.activeProjectId = "p1";
    calls.length = 0;
    const saved = new SavedQueryManager(state, () => {}, persistence);

    // The user saves a new query: one targeted create, nothing replaced.
    await saved.saveQuery("new", "select 1");
    await persistence.persistProjectState("p1");

    expect(calls.filter((c) => c.startsWith("library."))).toEqual(["library.createSavedQuery"]);
    expect(calls).toContain("projectState.save");
    expect(replacingWrites()).toEqual(["projectState.save"]);
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
    await persistence.persistAppState();
    await persistence.persistProjectState(state.projects[0].id);
    expect(replacingWrites()).toEqual([]);
  });

  it("projects: an empty file gets the default project from Core", async () => {
    failLoads = false;
    const { state, persistence, restoration } = setup();
    const projects = new ProjectManager(state, persistence, restoration);

    await projects.initialize();

    expect(state.projects.map((p) => p.id)).toEqual(["default-seaquel"]);
    expect(calls).toContain("library.ensureDefaultProject");
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
