/**
 * Other windows' 5d-2 changes reaching this page (Decisions 16 and 18):
 * dashboards, saved workflows, AI chats and their messages, and the
 * settings kinds, through `LibrarySync` + `ChangeFeed` and the view models,
 * over one recording library that two pages share and a Core client whose
 * event channel each test drives.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { CoreClient, ResubscribedInfo, WorkspaceEvent } from "$lib/core/client";
import type { SendAIMessageParams } from "$lib/services/ai";
import type { DashboardTabManager } from "../dashboard-tabs.svelte.js";

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => true,
  isWeb: () => false,
  isDemo: () => false,
}));
let behaviour: (p: SendAIMessageParams) => Promise<void> = async () => {};
vi.mock("$lib/services/ai", () => ({
  sendAIMessage: vi.fn((p: SendAIMessageParams) => behaviour(p)),
}));
vi.mock("$lib/services/ai-mentions", () => ({ resolveMentions: (c: string) => c }));
vi.mock("$lib/stores/ai-settings.svelte", () => ({
  aiSettingsStore: { settings: { shareSchemaGlobally: true, shareDataGlobally: true } },
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
const { StateRestorationManager } = await import("../state-restoration.svelte.js");
const { DashboardManager } = await import("../dashboard-manager.svelte.js");
const { WorkflowManager } = await import("../workflow-manager.svelte.js");
const { WorkflowState } = await import("../workflow-state.svelte.js");
const { AIChatManager } = await import("../ai-chat-manager.svelte.js");
const { UIStateManager } = await import("../ui-state.svelte.js");
const { ChangeFeed } = await import("./change-feed");
const { LibrarySync } = await import("./sync");
const { RecordingLibrary } = await import("./recording-library");
const { setLibrary } = await import("./index");
import type { StoredKind } from "./types";

/** A Core client whose event channel the test drives. */
function fakeClient() {
  const events = new Set<(e: WorkspaceEvent) => void>();
  const resubscribed = new Set<(i: ResubscribedInfo) => void>();
  const client = {
    call: vi.fn(),
    stream: vi.fn(),
    events: (h: (e: WorkspaceEvent) => void) => (events.add(h), () => events.delete(h)),
    onResubscribed: (h: (i: ResubscribedInfo) => void) => (
      resubscribed.add(h),
      () => resubscribed.delete(h)
    ),
    onEventsUnavailable: () => () => {},
  } as unknown as CoreClient;
  return {
    client,
    emit: (e: WorkspaceEvent) => events.forEach((h) => h(e)),
    resubscribe: (initial: boolean) => resubscribed.forEach((h) => h({ initial })),
  };
}

let library: InstanceType<typeof RecordingLibrary>;

/** One page: its view models, feed and sync over the shared library. */
async function openPage(origin: string) {
  const channel = fakeClient();
  const state = new DatabaseState();
  state.projects = [
    { id: "p1", name: "p1", createdAt: new Date(), updatedAt: new Date(), customLabels: [] },
  ];
  state.activeProjectId = "p1";
  state.connections = [
    {
      id: "conn-1",
      projectId: "p1",
      name: "Local",
      type: "postgres",
      activeAIProviderId: "prov",
      activeAIModel: "model",
    } as never,
  ];
  state.activeConnectionIdByProject = { p1: "conn-1" };
  state.queriesByProject = { p1: [] };
  state.savedWorkflowsByProject = { p1: [] };
  const restoration = new StateRestorationManager(state);
  const dashboards = new DashboardManager(
    state,
    async () => [],
    () => {},
  );
  const workflowState = new WorkflowState();
  const workflow = new WorkflowManager(state, workflowState, async () => ({
    rows: [],
    truncated: false,
  }));
  let ui: InstanceType<typeof UIStateManager>;
  const chats = new AIChatManager(
    state,
    (chatId) => restoration.loadAIChatMessages(chatId),
    (chatId) => ui.abortStreamFor(chatId),
  );
  ui = new UIStateManager(
    state,
    () => {},
    async () => ({ rows: [], truncated: false }),
    chats,
    (chatId) => chats.persistMessages(chatId),
    dashboards,
    {} as DashboardTabManager,
  );
  const settings = vi.fn(async (_kind: string, _ids: readonly string[] | null) => {});
  const views = {
    connections: { refreshFromLibrary: vi.fn(async () => {}) },
    projects: { refreshFromLibrary: vi.fn(async () => {}) },
    savedQueries: { refreshFromLibrary: vi.fn(async () => {}) },
    history: { reloadHistory: vi.fn(async () => {}) },
    dashboards,
    workflows: workflow,
    chats: {
      refreshChats: (connectionId: string) =>
        chats.refreshChats(connectionId, (id) => restoration.loadAIChats(id)),
      refreshMessages: (chatId: string) => chats.refreshMessages(chatId),
    },
    settings,
  };
  const feed = new ChangeFeed({
    client: () => channel.client,
    origin: () => origin,
    seqs: state.librarySeqs,
  });
  const sync = new LibrarySync(state, feed, views);
  sync.start();
  await restoration.loadProjectData("p1");
  await restoration.loadAIChats("conn-1");
  sync.markLoaded();
  return { ...channel, state, dashboards, workflow, workflowState, chats, ui, views, sync };
}

type Page = Awaited<ReturnType<typeof openPage>>;

/** Another window's write reaching `page`: Core's event for it. */
function changedElsewhere(
  page: Page,
  kind: StoredKind,
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
  behaviour = async () => {};
  library = new RecordingLibrary();
  library.seedProject("p1");
  library.seedConnection("conn-1", { projectId: "p1" });
  setLibrary(library);
});

afterEach(() => {
  setLibrary(null);
  vi.useRealTimers();
});

describe("other windows' 5d-2 changes", () => {
  it("a workflow saved in one tab appears in the other", async () => {
    const one = await openPage("tab-1");
    const two = await openPage("tab-2");
    one.workflow.addQueryNode("SELECT 1");
    const saved = (await one.workflow.saveWorkflow("Flow"))!;

    changedElsewhere(two, "workflow", "p1", [saved.id]);
    await settle();
    expect(two.state.savedWorkflowsByProject.p1.map((w) => w.name)).toEqual(["Flow"]);

    // Deleted there: gone here, and a canvas showing it is unlinked, not cleared.
    expect(await two.workflow.loadWorkflow(saved.id)).toBe(true);
    await one.workflow.deleteWorkflow(saved.id);
    changedElsewhere(two, "workflow", "p1", [saved.id]);
    await settle();
    expect(two.state.savedWorkflowsByProject.p1).toEqual([]);
    expect(two.workflowState.activeWorkflowId).toBeNull();
    expect(two.workflowState.nodes).toHaveLength(1);
  });

  it("a dashboard renamed in one tab is renamed in the other", async () => {
    const one = await openPage("tab-1");
    const dashboard = (await one.dashboards.createDashboard("Sales"))!;
    const two = await openPage("tab-2");
    expect(two.state.dashboardsByProject.p1.map((d) => d.name)).toEqual(["Sales"]);

    await one.dashboards.renameDashboard(dashboard.id, "Revenue");
    changedElsewhere(two, "dashboard", "p1", [dashboard.id]);
    await settle();

    expect(two.state.dashboardsByProject.p1.map((d) => d.name)).toEqual(["Revenue"]);
    // The rename's version arrives too.
    expect(two.state.dashboardVersionsByProject.p1).toHaveLength(1);
  });

  it("another tab's rename renames this tab's dashboard tabs", async () => {
    const one = await openPage("tab-1");
    const dashboard = (await one.dashboards.createDashboard("Sales"))!;
    const two = await openPage("tab-2");
    two.state.dashboardTabsByProject = {
      p1: [{ id: "t", name: "Sales", dashboardId: dashboard.id }],
    };

    await one.dashboards.renameDashboard(dashboard.id, "Revenue");
    changedElsewhere(two, "dashboard", "p1", [dashboard.id]);
    await settle();

    expect(two.state.dashboardTabsByProject.p1[0].name).toBe("Revenue");
  });

  it("a refused edit is taken back, and another tab's change then applies", async () => {
    const one = await openPage("tab-1");
    const dashboard = (await one.dashboards.createDashboard("Sales"))!;
    const two = await openPage("tab-2");
    library.failures.set("updateDashboard", { error: new Error("STORAGE_ERROR: busy") });
    await two.dashboards.renameDashboard(dashboard.id, "Mine");
    expect(two.dashboards.getDashboard(dashboard.id)?.name).toBe("Sales");

    await one.dashboards.renameDashboard(dashboard.id, "Theirs");
    changedElsewhere(two, "dashboard", "p1", [dashboard.id]);
    await settle();

    expect(two.dashboards.getDashboard(dashboard.id)?.name).toBe("Theirs");
  });

  it("a dashboard change skipped while this tab's write was on its way is read after it", async () => {
    const one = await openPage("tab-1");
    const dashboard = (await one.dashboards.createDashboard("Sales"))!;
    const two = await openPage("tab-2");
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const update = library.updateDashboard.bind(library);
    const spy = vi.spyOn(library, "updateDashboard").mockImplementationOnce(async (id, patch) => {
      await held;
      return update(id, patch);
    });

    const moving = two.dashboards.updateViewport(dashboard.id, { x: 9, y: 9, zoom: 1 });
    await one.dashboards.toggleDashboardStarred(dashboard.id);
    changedElsewhere(two, "dashboard", "p1", [dashboard.id]);
    await vi.advanceTimersByTimeAsync(150);
    release();
    await moving;
    await settle();
    spy.mockRestore();

    expect(two.dashboards.getDashboard(dashboard.id)?.starred).toBe(true);
    expect(two.dashboards.getDashboard(dashboard.id)?.viewport).toEqual({ x: 9, y: 9, zoom: 1 });
  });

  it("a workflow change skipped while this tab's write was on its way is read after it", async () => {
    const one = await openPage("tab-1");
    one.workflow.addQueryNode("SELECT 1");
    const saved = (await one.workflow.saveWorkflow("Flow"))!;
    const two = await openPage("tab-2");
    await two.workflow.refreshFromLibrary("p1", null);
    let release!: () => void;
    const held = new Promise<void>((r) => (release = r));
    const update = library.updateWorkflow.bind(library);
    // This tab's rename is stored first; its answer is held.
    vi.spyOn(library, "updateWorkflow").mockImplementationOnce(async (id, body) => {
      const answer = await update(id, body);
      await held;
      return answer;
    });

    const renaming = two.workflow.renameWorkflow(saved.id, "Mine");
    await vi.advanceTimersByTimeAsync(0);
    // The other tab's rename lands after it: the stored name is "Theirs".
    await one.workflow.renameWorkflow(saved.id, "Theirs");
    changedElsewhere(two, "workflow", "p1", [saved.id]);
    await vi.advanceTimersByTimeAsync(150);
    release();
    await renaming;
    await settle();

    expect(two.state.savedWorkflowsByProject.p1.map((w) => w.name)).toEqual(["Theirs"]);
  });

  it("a dashboard deleted elsewhere closes its tabs", async () => {
    const one = await openPage("tab-1");
    const dashboard = (await one.dashboards.createDashboard("Sales"))!;
    const two = await openPage("tab-2");
    two.state.dashboardTabsByProject = {
      p1: [{ id: "dash-tab", name: "Sales", dashboardId: dashboard.id }],
      p2: [{ id: "dash-tab-2", name: "Sales", dashboardId: dashboard.id }],
    };
    two.state.activeDashboardTabIdByProject = { p1: "dash-tab" };
    two.state.tabOrderByProject = { p1: ["dash-tab"], p2: ["dash-tab-2"] };

    const stops = vi.spyOn(two.dashboards, "closeDashboard");
    await one.dashboards.deleteDashboard(dashboard.id);
    changedElsewhere(two, "dashboard", "p1", [dashboard.id]);
    await settle();

    // Its auto-refresh timers and runs stop.
    expect(stops).toHaveBeenCalledWith(dashboard.id);
    expect(two.state.dashboardsByProject.p1).toEqual([]);
    expect(two.state.dashboardTabsByProject).toEqual({ p1: [], p2: [] });
    expect(two.state.activeDashboardTabIdByProject.p1).toBeNull();
    expect(two.state.tabOrderByProject).toEqual({ p1: [], p2: [] });
    expect(toasts).toEqual([expect.stringContaining("Sales")]);
  });

  it("a chat deleted elsewhere while streaming stops and switches", async () => {
    const one = await openPage("tab-1");
    const older = (await one.chats.createChat("Older"))!;
    await vi.advanceTimersByTimeAsync(10);
    const streaming = (await one.chats.createChat("Streaming"))!;
    const two = await openPage("tab-2");
    let signal: AbortSignal | undefined;
    behaviour = (p) => {
      signal = p.signal;
      return new Promise(() => {});
    };
    await one.ui.sendAIMessage("hello");
    expect(one.state.aiStreamingChatId).toBe(streaming);

    await two.chats.deleteChat(streaming);
    changedElsewhere(one, "chat", "conn-1", [streaming]);
    await settle();

    expect(signal?.aborted).toBe(true);
    expect(one.state.isAIStreaming).toBe(false);
    expect(one.state.aiChatsByConnection["conn-1"].map((c) => c.id)).toEqual([older]);
    expect(one.state.activeAIChatIdByConnection["conn-1"]).toBe(older);
    // Nothing of the deleted chat was stored again.
    expect(library.messages.get(streaming)).toBeUndefined();
  });

  it("a streaming chat holds another window's message events until its turn ends", async () => {
    const one = await openPage("tab-1");
    const chatId = (await one.chats.createChat("Chat"))!;
    let finish!: () => void;
    behaviour = (p) =>
      new Promise<void>((resolve) => {
        finish = () => {
          p.onChunk("done");
          p.onDone();
          resolve();
        };
      });
    await one.ui.sendAIMessage("hello");

    // Another window stored a message in this chat meanwhile.
    await library.putChatMessages(chatId, [
      { id: "elsewhere", role: "user", content: "hi", timestamp: "2020-01-01T00:00:00.000Z" },
    ]);
    const reads = () => library.callsOf("listChatMessages").length;
    const before = reads();
    changedElsewhere(one, "chatMessages", chatId, ["elsewhere"]);
    await settle();
    expect(reads()).toBe(before);
    expect(one.state.aiMessagesByChat[chatId].map((m) => m.id)).not.toContain("elsewhere");

    finish();
    await settle();
    expect(reads()).toBe(before + 1);
    expect(one.state.aiMessagesByChat[chatId].map((m) => m.content)).toEqual([
      "hi",
      "hello",
      "done",
    ]);
  });

  it("every new kind reloads on resubscribe and a new epoch", async () => {
    const page = await openPage("tab-1");
    const chatId = (await page.chats.createChat("Chat"))!;
    const refreshDashboards = vi.spyOn(page.dashboards, "refreshFromLibrary");
    const refreshWorkflows = vi.spyOn(page.workflow, "refreshFromLibrary");
    const refreshChats = vi.spyOn(page.views.chats, "refreshChats");
    const refreshMessages = vi.spyOn(page.views.chats, "refreshMessages");
    const settingsKinds = () =>
      page.views.settings.mock.calls.map(([kind, ids]) => `${String(kind)}:${String(ids)}`).sort();
    const expected = [
      "aiSettings:null",
      "importState:null",
      "onboarding:null",
      "setting:null",
      "theme:null",
      "tutorial:null",
    ];

    page.resubscribe(false);
    await settle();
    expect(refreshDashboards).toHaveBeenCalledWith("p1", null);
    expect(refreshWorkflows).toHaveBeenCalledWith("p1", null);
    expect(refreshChats).toHaveBeenCalledWith("conn-1");
    expect(refreshMessages).toHaveBeenCalledWith(chatId);
    expect(settingsKinds()).toEqual(expected);

    for (const spy of [refreshDashboards, refreshWorkflows, refreshChats, refreshMessages]) {
      spy.mockClear();
    }
    page.views.settings.mockClear();
    library.epoch = "epoch-2";
    changedElsewhere(page, "dashboard", "p1", null);
    await settle();
    expect(refreshDashboards).toHaveBeenCalledWith("p1", null);
    expect(refreshWorkflows).toHaveBeenCalledWith("p1", null);
    expect(refreshChats).toHaveBeenCalledWith("conn-1");
    expect(refreshMessages).toHaveBeenCalledWith(chatId);
    expect(settingsKinds()).toEqual(expected);
  });
});
