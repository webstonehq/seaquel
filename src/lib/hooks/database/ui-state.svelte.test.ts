/**
 * The AI chat's connection binding: a tool call runs on the connection the
 * chat belongs to, through `executeReadOnly`, whatever is active by then.
 * `sendAIMessage` is faked to capture its params; the tool calls go through
 * the real `handleToolCall`.
 */
import { describe, expect, it, vi } from "vitest";
import type { AIMessage } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { SendAIMessageParams } from "$lib/services/ai";
import type { DatabaseState } from "./state.svelte.js";
import type { AIChatManager } from "./ai-chat-manager.svelte.js";
import type { DashboardTabManager } from "./dashboard-tabs.svelte.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";

const sent: SendAIMessageParams[] = [];
vi.mock("$lib/services/ai", () => ({
  sendAIMessage: vi.fn(async (p: SendAIMessageParams) => {
    sent.push(p);
  }),
}));
vi.mock("$lib/services/ai-mentions", () => ({ resolveMentions: (c: string) => c }));
vi.mock("$lib/stores/ai-settings.svelte", () => ({
  aiSettingsStore: { settings: { shareSchemaGlobally: true, shareDataGlobally: true } },
}));
/** Runs while a dashboard is being saved: lets a test switch connection mid-await. */
let duringSave: () => void = () => {};
vi.mock("$lib/storage", () => ({
  getDatabase: vi.fn(async () => ({})),
  dashboardsRepo: {
    save: vi.fn(async () => duringSave()),
    remove: vi.fn(async () => {}),
  },
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));
vi.mock("$lib/engine", () => ({ getEngineClient: vi.fn(), usesRustEngine: () => false }));

const { handleToolCall } =
  await vi.importActual<typeof import("$lib/services/ai/index")>("$lib/services/ai/index");
const { UIStateManager } = await import("./ui-state.svelte.js");
const { DashboardManager } = await import("./dashboard-manager.svelte.js");
const { QueryCrudManager } = await import("./query-crud.svelte.js");

const ai = { activeAIProviderId: "prov", activeAIModel: "model" };
const local = {
  id: "conn-1",
  type: "mysql",
  name: "Local",
  providerConnectionId: "pc-1",
  ...ai,
};
const other = {
  id: "conn-2",
  type: "postgres",
  name: "Other",
  providerConnectionId: "pc-2",
  ...ai,
};

/**
 * @param hold The fake query never finishes on its own; only an abort ends it.
 * @param orphan The chat isn't listed under any connection.
 */
function setup({ hold = false, orphan = false } = {}) {
  duringSave = () => {};
  const state = {
    activeProjectId: "p",
    activeConnectionId: "conn-1",
    activeConnection: local,
    connections: [local, other],
    schemas: { "conn-1": [], "conn-2": [] },
    activeSchema: [],
    queriesByProject: {},
    dashboardsByProject: {
      p: [
        {
          id: "d-1",
          name: "D",
          projectId: "p",
          widgets: [],
          viewport: { x: 0, y: 0, zoom: 1 },
          dateFilter: null,
          createdAt: new Date(),
          updatedAt: new Date(),
          shared: false,
        },
      ],
    },
    aiChatsByConnection: {
      "conn-1": orphan
        ? []
        : [
            {
              id: "chat-1",
              connectionId: "conn-1",
              title: "",
              createdAt: new Date(),
              updatedAt: new Date(),
            },
          ],
    },
    aiMessagesByChat: { "chat-1": [] as AIMessage[] },
    activeAIChatId: "chat-1",
    isAIStreaming: false,
  };
  const s = state as unknown as DatabaseState;
  const switchTo = (c: typeof other) => {
    state.activeConnectionId = c.id;
    state.activeConnection = c as typeof local;
    state.activeAIChatId = null as unknown as string;
  };

  const provider = {
    select: vi.fn(async () => [{ written: true }]),
    // Honours the signal, as the real providers do: an abort rejects it.
    selectReadOnly: vi.fn(
      (_id: string, _sql: string, signal?: AbortSignal) =>
        new Promise<Record<string, unknown>[]>((resolve, reject) => {
          const abort = () => reject(new DOMException("Aborted", "AbortError"));
          if (signal?.aborted) return abort();
          signal?.addEventListener("abort", abort, { once: true });
          if (!hold) resolve([{ n: 1 }]);
        }),
    ),
  };
  const providers = { getForType: async () => provider } as unknown as ProviderRegistry;
  const crud = new QueryCrudManager(s, providers, {
    isEnabled: () => false,
  } as unknown as PendingChangesManager);
  const dashboards = new DashboardManager(
    s,
    (id, sql, signal) => crud.executeReadOnly(id, sql, signal),
    () => {},
  );
  const chats = {
    ensureActiveChat: () => "chat-1",
    updateChatTitle: vi.fn(),
    updateChatTimestamp: vi.fn(),
  } as unknown as AIChatManager;
  const ui = new UIStateManager(
    s,
    () => {},
    (id, sql, signal, name) => crud.executeReadOnly(id, sql, signal, name),
    chats,
    async () => {},
    dashboards,
    { add: vi.fn() } as unknown as DashboardTabManager,
  );

  sent.length = 0;
  ui.sendAIMessage("how many?");
  const params = sent[0];
  const assistant = () => state.aiMessagesByChat["chat-1"].at(-1)!;
  return { state, ui, params, provider, switchTo, assistant, dashboards };
}

describe("the chat's connection binding", () => {
  it("passes the chat's connection, not a type and an executor", () => {
    const { params } = setup();
    expect(params.connection).toEqual({ id: "conn-1", type: "mysql", name: "Local" });
    expect("executeQuery" in params).toBe(false);
    expect("databaseType" in params).toBe(false);
  });

  it("runs a tool call on the chat's connection after the active one changed", async () => {
    const { params, provider, switchTo } = setup();
    switchTo(other);
    // One SELECT on MySQL (`#` comment); two statements on Postgres.
    const sql = "SELECT 1 AS n # ; DELETE FROM t";
    const out = await handleToolCall(
      "run_query",
      { query: sql },
      { ...params, aiAllowAllQueries: true },
    );
    expect(out).toContain("| n |");
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-1", sql, params.signal);
    expect(provider.select).not.toHaveBeenCalled();
  });

  it("refuses when the chat's connection was removed, without calling the provider", async () => {
    const { params, provider, state } = setup();
    state.connections = [other];
    const out = await handleToolCall(
      "run_query",
      { query: "SELECT 1" },
      { ...params, aiAllowAllQueries: true },
    );
    expect(out).toBe('Query error: The connection "Local" was removed');
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("refuses when the chat's connection is disconnected, without calling the provider", async () => {
    const { params, provider, state } = setup();
    state.connections = [{ ...local, providerConnectionId: undefined as unknown as string }, other];
    const out = await handleToolCall(
      "run_query",
      { query: "SELECT 1" },
      { ...params, aiAllowAllQueries: true },
    );
    expect(out).toBe(
      'Query error: The connection "Local" is disconnected; reconnect it and try again',
    );
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("runs on the new provider id after a reconnect", async () => {
    const { params, provider, state } = setup();
    state.connections = [{ ...local, providerConnectionId: "pc-9" }, other];
    await handleToolCall(
      "run_query",
      { query: "SELECT 1" },
      { ...params, aiAllowAllQueries: true },
    );
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-9", "SELECT 1", params.signal);
  });

  it("runs an approval given after a switch on the chat's connection", async () => {
    const { params, provider, switchTo, assistant } = setup();
    const out = handleToolCall("run_query", { query: "SELECT 1 AS n" }, params);
    const pending = assistant().pendingApproval!;
    expect(pending.connectionName).toBe("Local");
    expect(pending.connectionType).toBe("mysql");
    switchTo(other);
    pending.approve();
    expect(await out).toContain("| n |");
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-1", "SELECT 1 AS n", params.signal);
  });

  it("Stop during an approval resolves the tool call and clears the card", async () => {
    const { ui, params, provider, assistant } = setup();
    const out = handleToolCall("run_query", { query: "SELECT 1" }, params);
    expect(assistant().pendingApproval).toBeTruthy();
    ui.cancelAIStream();
    expect(await out).toBe("Query cancelled");
    expect(assistant().pendingApproval).toBeNull();
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("Stop after a switch still clears the chat's approval card", async () => {
    const { ui, params, switchTo, assistant } = setup();
    const out = handleToolCall("run_query", { query: "SELECT 1" }, params);
    switchTo(other);
    ui.cancelAIStream();
    expect(await out).toBe("Query cancelled");
    expect(assistant().pendingApproval).toBeNull();
  });
});

describe("dashboard tools and the chat's connection", () => {
  const add = { dashboard_id: "d-1", widget_type: "kpi", title: "n", query: "SELECT 1 AS n" };

  it("adds a widget whose first run is read-only while the chat's connection is active", async () => {
    const { params, provider, dashboards } = setup();
    const out = JSON.parse(await handleToolCall("add_widget", add, params));
    expect(out.widget_id).toBeDefined();
    expect(provider.selectReadOnly).toHaveBeenCalledWith(
      "pc-1",
      "SELECT 1 AS n",
      expect.any(AbortSignal),
    );
    expect(provider.select).not.toHaveBeenCalled();
    expect(dashboards.getDashboard("d-1")!.widgets[0].result).toEqual([{ n: 1 }]);
  });

  it("refuses add_widget after a switch", async () => {
    const { params, provider, switchTo, dashboards } = setup();
    switchTo(other);
    const out = JSON.parse(await handleToolCall("add_widget", add, params));
    expect(out).toEqual({
      error: 'The active connection is "Other"; switch back to "Local" to change this dashboard',
    });
    expect(dashboards.getDashboard("d-1")!.widgets).toEqual([]);
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });
});

describe("approvals, Stop and cancellation", () => {
  it("clears each approval when it's settled, so a second one in the same reply works", async () => {
    const { params, provider, assistant } = setup();
    const first = handleToolCall("run_query", { query: "SELECT 1 AS n" }, params);
    const one = assistant().pendingApproval!;
    one.approve();
    expect(await first).toContain("| n |");
    expect(assistant().pendingApproval).toBeNull();

    const second = handleToolCall("run_query", { query: "SELECT 2 AS n" }, params);
    const two = assistant().pendingApproval!;
    expect(two.id).not.toBe(one.id);
    two.deny();
    expect(await second).toBe("User denied query execution");
    expect(assistant().pendingApproval).toBeNull();
    expect(provider.selectReadOnly).toHaveBeenCalledOnce();
  });

  it("Stop resolves every pending approval", async () => {
    const { ui, params, provider, assistant } = setup();
    const a = handleToolCall("run_query", { query: "SELECT 1" }, params);
    const b = handleToolCall("run_query", { query: "SELECT 2" }, params);
    ui.cancelAIStream();
    expect(await Promise.all([a, b])).toEqual(["Query cancelled", "Query cancelled"]);
    expect(assistant().pendingApproval).toBeNull();
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("Stop cancels a query that's running", async () => {
    const { ui, params, provider } = setup({ hold: true });
    const out = handleToolCall(
      "run_query",
      { query: "SELECT 1" },
      { ...params, aiAllowAllQueries: true },
    );
    await vi.waitFor(() => expect(provider.selectReadOnly).toHaveBeenCalledOnce());
    const signal = provider.selectReadOnly.mock.calls[0][2]!;
    expect(signal.aborted).toBe(false);
    ui.cancelAIStream();
    expect(await out).toBe("Query cancelled");
    expect(signal.aborted).toBe(true);
  });

  it("Stop cancels a widget's first run", async () => {
    const { ui, params, provider, dashboards } = setup({ hold: true });
    const out = handleToolCall(
      "add_widget",
      { dashboard_id: "d-1", widget_type: "kpi", title: "n", query: "SELECT 1 AS n" },
      params,
    );
    await vi.waitFor(() => expect(provider.selectReadOnly).toHaveBeenCalledOnce());
    ui.cancelAIStream();
    await out;
    expect(provider.selectReadOnly.mock.calls[0][2]!.aborted).toBe(true);
    const w = dashboards.getDashboard("d-1")!.widgets[0];
    expect(w.error).toBe("Query cancelled");
    expect(w.isLoading).toBe(false);
  });

  it("skips a new widget's first run when the connection switched while it was saved", async () => {
    const { params, provider, switchTo, dashboards } = setup();
    duringSave = () => switchTo(other);
    const out = JSON.parse(
      await handleToolCall(
        "add_widget",
        { dashboard_id: "d-1", widget_type: "kpi", title: "n", query: "SELECT 1 AS n" },
        params,
      ),
    );
    expect(out.widget_id).toBeDefined();
    expect(dashboards.getDashboard("d-1")!.widgets).toHaveLength(1);
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("fails closed for a chat that belongs to no connection", () => {
    const { state } = setup({ orphan: true });
    expect(sent).toHaveLength(0);
    expect(state.aiMessagesByChat["chat-1"].at(-1)!.content).toBe(
      "Error: This chat's connection was removed",
    );
  });
});
