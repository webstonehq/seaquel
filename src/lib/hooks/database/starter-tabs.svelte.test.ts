/**
 * Starter tabs for a project that isn't the active one (re-survey bug 21):
 * a project's load can finish after the user switched to another project,
 * and adding its defaults must not borrow the active project meanwhile.
 */
import { describe, it, expect, vi } from "vitest";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { DatabaseState } = await import("./state.svelte.js");
const { PaneManager } = await import("./pane-manager.svelte.js");
const { TabOrderingManager } = await import("./tab-ordering.svelte.js");
const { StarterTabManager } = await import("./starter-tabs.svelte.js");
const { hasSavedTabs } = await import("./project-manager.svelte.js");

function setup() {
  const state = new DatabaseState();
  const scheduled: (string | null)[] = [];
  const schedule = (id: string | null) => scheduled.push(id);
  const panes = new PaneManager(state, schedule);
  const tabs = new TabOrderingManager(state, schedule, panes);
  const starter = new StarterTabManager(state, tabs, schedule);
  return { state, starter, scheduled };
}

describe("starter tabs", () => {
  it("starter tabs for a project leave the active project alone", () => {
    const { state, starter, scheduled } = setup();
    state.activeProjectId = "active";
    state.activeView = "query";
    state.tabOrderByProject = { active: ["q1"] };
    state.paneLayoutByProject = {
      active: {
        panes: [{ id: "pane-a", tabIds: ["q1"], activeTabId: "q1" }],
        activePaneId: "pane-a",
      },
    };
    // Record every value the active project is set to while the defaults are added.
    const activeIds: (string | null)[] = [];
    let current = state.activeProjectId;
    Object.defineProperty(state, "activeProjectId", {
      configurable: true,
      get: () => current,
      set: (v) => {
        activeIds.push(v);
        current = v;
      },
    });

    starter.initializeDefaults("other");

    expect(activeIds).toEqual([]);
    expect(state.activeProjectId).toBe("active");
    expect(state.starterTabsByProject["other"]?.map((t) => t.id)).toEqual([
      "getting-started",
      "migration-tips",
    ]);
    expect(state.activeStarterTabIdByProject["other"]).toBe("getting-started");
    expect(state.tabOrderByProject["other"]).toEqual(["getting-started", "migration-tips"]);
    // The active project's order, layout and view are untouched.
    expect(state.tabOrderByProject["active"]).toEqual(["q1"]);
    expect(state.paneLayoutByProject["active"].panes[0].tabIds).toEqual(["q1"]);
    expect(state.activeView).toBe("query");
    expect(state.activeStarterTabIdByProject["active"]).toBeUndefined();
    expect(scheduled).toContain("other");
    expect(scheduled).not.toContain("active");
  });

  it("for the active project they are added as before", () => {
    const { state, starter } = setup();
    state.activeProjectId = "p";
    starter.initializeDefaults("p");
    expect(state.starterTabsByProject["p"]?.map((t) => t.id)).toEqual([
      "getting-started",
      "migration-tips",
    ]);
    expect(state.activeStarterTabIdByProject["p"]).toBe("getting-started");
    expect(state.tabOrderByProject["p"]).toEqual(["getting-started", "migration-tips"]);
  });
});

describe("pane focus", () => {
  it("focusing another pane schedules the project's save", () => {
    const { state, scheduled } = setup();
    state.activeProjectId = "p";
    state.paneLayoutByProject = {
      p: {
        panes: [
          { id: "left", tabIds: ["a"], activeTabId: "a" },
          { id: "right", tabIds: ["b"], activeTabId: "b" },
        ],
        activePaneId: "left",
      },
    };
    const panes = new PaneManager(state, (id) => scheduled.push(id));

    panes.setActivePane("right");

    expect(state.paneLayoutByProject.p.activePaneId).toBe("right");
    expect(scheduled).toContain("p");
  });
});

describe("which saved projects get starter tabs", () => {
  const none = {
    projectId: "p",
    queryTabs: [],
    schemaTabs: [],
    explainTabs: [],
    erdTabs: [],
    tabOrder: [],
    activeQueryTabId: null,
    activeSchemaTabId: null,
    activeExplainTabId: null,
    activeErdTabId: null,
    activeView: "query",
    activeConnectionId: null,
  } as never as Parameters<typeof hasSavedTabs>[0];

  it("a project with no saved tabs gets them", () => {
    expect(hasSavedTabs(none)).toBe(false);
  });

  it.each([
    ["workflowTabs", { id: "w", name: "W", connectionId: "c" }],
    ["canvasTabs", { id: "w", name: "W", connectionId: "c" }],
    ["statisticsTabs", { id: "s", name: "S", connectionId: "c" }],
    ["extensionsDuckdbTabs", { id: "e", name: "E", connectionId: "c" }],
    ["dashboardTabs", { id: "d", name: "D", dashboardId: "x" }],
    ["createTableTabs", { id: "t", name: "T", connectionId: "c", tableDefinition: "{}" }],
    ["dataTabs", { id: "d", connectionId: "c", tableName: "t", schemaName: "s" }],
    ["queryTabs", { id: "q", name: "Q", query: "" }],
  ])("a project whose only saved tab is in %s gets none", (key, tab) => {
    expect(hasSavedTabs({ ...none, [key]: [tab] })).toBe(true);
  });

  // Task 2 review follow-up: only tabs the restore keeps count.
  it.each([
    ["schemaTabs", { id: "s", tableName: "t", schemaName: "public" }],
    ["erdTabs", { id: "e", name: "E" }],
    ["statisticsTabs", { id: "s", name: "S", connectionId: "" }],
    ["workflowTabs", { id: "w", name: "W", connectionId: "" }],
    ["createTableTabs", { id: "t", name: "T", connectionId: "", tableDefinition: "{}" }],
    ["dataTabs", { id: "d", connectionId: "", tableName: "t", schemaName: "s" }],
    ["extensionsDuckdbTabs", { id: "e", name: "E", connectionId: "" }],
    ["dashboardTabs", { id: "d", name: "D", dashboardId: "" }],
  ])("a %s tab the restore drops (no connection or dashboard) doesn't count", (key, tab) => {
    expect(hasSavedTabs({ ...none, [key]: [tab] })).toBe(false);
  });

  it("a tab naming a connection that's gone doesn't count once the connections are known", () => {
    const saved = {
      ...none,
      dataTabs: [{ id: "d", connectionId: "gone", tableName: "t", schemaName: "s" }],
    };
    // Before the connections are read (startup), it counts: it may be kept.
    expect(hasSavedTabs(saved)).toBe(true);
    expect(hasSavedTabs(saved, new Set(["other"]))).toBe(false);
    expect(hasSavedTabs(saved, new Set(["gone"]))).toBe(true);
  });
});
