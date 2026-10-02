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
/** The messages each `putChatMessages` carried. */
const puts: { id: string }[][] = [];

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

const { WindowStateManager } = await import("./window-state.svelte.js");
const { AIChatManager } = await import("./ai-chat-manager.svelte.js");
const { SharedRepoManager } = await import("./shared-repo-manager.svelte.js");
const { setShared, NoShared } = await import("./shared/index");
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
          if (holdLoads) await holdLoads;
          if (failLoads) throw new Error("STORAGE_ERROR: upstream unavailable");
          if (method === "listChatMessages") {
            return { value: { messages: [], storedBytes: 0 }, seq };
          }
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
        if (method === "putChatMessages") {
          puts.push(args[1] as { id: string }[]);
          return { value: { messages: args[1], storedBytes: 0 }, seq: { ...seq, n: 3 } };
        }
        throw new Error(`unexpected library.${method}`);
      },
  }),
);
const { DatabaseState } = await import("./state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { ProjectManager } = await import("./project-manager.svelte.js");
const { resetLoadGuardToast } = await import("$lib/storage/load-guard");
import type { UiService } from "./library/types";

/**
 * The window's `ui` calls (phase 5d-2), recorded as `ui.method`: the reads
 * fail while `failLoads`, and wait for `holdLoads`.
 */
const ui: UiService = {
  async windowGet() {
    calls.push("ui.windowGet");
    if (failLoads) throw new Error("STORAGE_ERROR: upstream unavailable");
    return { value: { activeProjectId: null, from: null }, seq };
  },
  async windowActivate() {
    calls.push("ui.windowActivate");
    return { value: null, seq };
  },
  async windowStateLoad() {
    calls.push("ui.windowStateLoad");
    if (holdLoads) await holdLoads;
    if (failLoads) throw new Error("STORAGE_ERROR: upstream unavailable");
    return { value: { state: null, rev: 0, copiedFrom: "empty" }, seq };
  },
  async windowStateSave(_w, _p, rev) {
    calls.push("ui.windowStateSave");
    return { value: { stale: false, rev }, seq };
  },
  windowStateSaveKeepalive: () => false,
};

/** Writes that replace or delete stored rows (or would record a stand-in). */
const REPLACING = [
  "appState.set",
  "ui.windowStateSave",
  "ui.windowActivate",
  "queryHistory.replaceAll",
];
const replacingWrites = () => calls.filter((c) => REPLACING.includes(c));

function setup() {
  const state = new DatabaseState();
  const windowState = new WindowStateManager(state, { ui: () => ui, windowId: () => "win-1" });
  const restoration = new StateRestorationManager(state);
  const chats = new AIChatManager(state, (chatId) => restoration.loadAIChatMessages(chatId));
  return { state, windowState, chats, restoration };
}

beforeEach(() => {
  calls.length = 0;
  puts.length = 0;
  toasts.length = 0;
  failLoads = true;
  holdLoads = null;
  resetLoadGuardToast();
});

describe("a failed load blocks the save that would replace it", () => {
  it("view state: a tab change doesn't wipe the window's tabs", async () => {
    const { state, windowState } = setup();
    expect(await windowState.load("p1")).toBeNull();
    state.queryTabsByProject["p1"] = [];

    await windowState.saveNow("p1");

    expect(calls).not.toContain("ui.windowStateSave");
    expect(replacingWrites()).toEqual([]);
    expect(toasts).toHaveLength(1);
  });

  it("saved queries: a failed load leaves the page's copy, and saving one writes only it", async () => {
    const { state, windowState, restoration } = setup();
    await restoration.loadProjectData("p1"); // saved queries fail
    failLoads = false;
    await windowState.load("p1"); // the view state loads fine
    windowState.markLoaded("p1"); // and is restored
    state.activeProjectId = "p1";
    calls.length = 0;
    const saved = new SavedQueryManager(state, () => {});

    // The user saves a new query: one targeted create, nothing replaced.
    await saved.saveQuery("new", "select 1");
    await windowState.saveNow("p1");

    expect(calls.filter((c) => c.startsWith("library."))).toEqual(["library.createSavedQuery"]);
    expect(replacingWrites()).toEqual(["ui.windowStateSave"]);
  });

  it("shared repos: a failed list leaves the repos shown, and nothing replaces the list", async () => {
    // Phase 5e: the repo list is Core's (`shared.reposList`), written row by
    // row; there's no replace-all save left to guard.
    const { state } = setup();
    const shown = {
      id: "repo-1",
      name: "Team",
      path: "/r",
      remoteUrl: "",
      branch: "main",
      lastSyncAt: null,
      syncStatus: "synced" as const,
    };
    state.sharedRepos = [shown];
    setShared(new NoShared());
    try {
      await new SharedRepoManager(state).loadRepos();
    } finally {
      setShared(null);
    }

    expect(state.sharedRepos).toEqual([shown]);
    expect(replacingWrites()).toEqual([]);
  });

  it("AI chat messages: a new message is put on its own, not over the chat's history", async () => {
    // Decision 24: messages are upserted by id, so a failed load needs no
    // guard: the put carries only what the page has, and replaces nothing.
    const { state, chats, restoration } = setup();
    await restoration.loadAIChatMessages("chat-1");
    const message = {
      id: "m-new",
      chatId: "chat-1",
      role: "user" as const,
      content: "hello",
      timestamp: new Date("2026-01-01T00:00:00.000Z"),
    };
    state.aiMessagesByChat = { "chat-1": [message] };

    await chats.persistMessages("chat-1");

    expect(puts.map((p) => p.map((m) => m.id))).toEqual([["m-new"]]);
    expect(replacingWrites()).toEqual([]);
  });

  it("projects: no default project is stored and the active project isn't overwritten", async () => {
    const { state, windowState, restoration } = setup();
    const projects = new ProjectManager(state, windowState, restoration);

    await projects.initialize();

    // The app still gets a project to work in, in memory only: it isn't
    // recorded as the window's active project, and its tabs aren't saved.
    expect(state.projects).toHaveLength(1);
    await windowState.activate(state.projects[0].id);
    await windowState.saveNow(state.projects[0].id);
    expect(replacingWrites()).toEqual([]);
  });

  it("projects: an empty file gets the default project from Core", async () => {
    failLoads = false;
    const { state, windowState, restoration } = setup();
    const projects = new ProjectManager(state, windowState, restoration);

    await projects.initialize();

    expect(state.projects.map((p) => p.id)).toEqual(["default-seaquel"]);
    expect(calls).toContain("library.ensureDefaultProject");
  });
});

describe("a load that is still running blocks the save too", () => {
  it("refuses view-state saves until the window's load answers", async () => {
    failLoads = false;
    let release!: () => void;
    holdLoads = new Promise((r) => (release = r));
    const { windowState } = setup();

    const loading = windowState.load("p1");
    await windowState.saveNow("p1");
    expect(replacingWrites()).toEqual([]);
    expect(toasts).toEqual([]);

    release();
    holdLoads = null;
    await loading;
    // Answered, but not restored yet: still refused.
    await windowState.saveNow("p1");
    expect(replacingWrites()).toEqual([]);
    windowState.markLoaded("p1");
    await windowState.saveNow("p1");
    expect(replacingWrites()).toEqual(["ui.windowStateSave"]);
  });
});

describe("query history has no replacing save to guard", () => {
  it("flush writes no history", async () => {
    failLoads = false;
    const { state, windowState, restoration } = setup();
    state.queryHistoryByConnection = { c1: [] };
    await restoration.loadConnectionData("c1");
    calls.length = 0;

    await windowState.flush();

    expect(calls.filter((c) => c.startsWith("queryHistory."))).toEqual([]);
  });
});

describe("a chat whose messages failed to load", () => {
  it("stays unloaded, so switching to it loads it again", async () => {
    const { state, restoration } = setup();

    await restoration.loadAIChatMessages("chat-1");
    expect("chat-1" in state.aiMessagesByChat).toBe(false);

    failLoads = false;
    await restoration.loadAIChatMessages("chat-1");
    expect(state.aiMessagesByChat["chat-1"]).toEqual([]);
  });
});
