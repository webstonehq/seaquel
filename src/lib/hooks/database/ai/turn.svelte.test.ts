/**
 * The assistant's view model on Core's `ai` events (phase 6 Task 7): the
 * page sends a turn (`ai.chat`), shows what the events say and answers the
 * approval card and the dashboard tools through `respond`. It decides
 * nothing: not what the model is sent, which tools it gets, or when the
 * turn is stored (Core stores it; `done.messages` is applied by `seq`).
 * The turns are a fake `AiService` (`./testing`) whose "Core" stores rows
 * in a `RecordingLibrary`.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DashboardTabManager } from "../dashboard-tabs.svelte.js";
import type { DashboardManager } from "../dashboard-manager.svelte.js";
import type { ChatMessageDraft } from "../library/types";
import type { PersistedAIMessage } from "$lib/types/generated/PersistedAIMessage";

const env = vi.hoisted(() => ({ web: false }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => !env.web,
  isWeb: () => env.web,
  isDemo: () => false,
}));
vi.mock("$lib/stores/ai-settings.svelte", () => ({
  aiSettingsStore: { settings: { shareSchemaGlobally: true, shareDataGlobally: true } },
}));
const toasts = vi.hoisted(() => [] as string[]);
vi.mock("$lib/utils/toast", () => ({ errorToast: (m: string) => toasts.push(m) }));
vi.mock("svelte-sonner", () => ({
  toast: { success: vi.fn(), info: (m: string) => toasts.push(m), warning: vi.fn() },
}));
const callSecret = vi.hoisted(() => vi.fn());
vi.mock("$lib/storage/rust-client", async (importOriginal) => ({
  ...(await importOriginal<typeof import("$lib/storage/rust-client")>()),
  callSecret,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { DatabaseState } = await import("../state.svelte.js");
const { AIChatManager } = await import("../ai-chat-manager.svelte.js");
const { UIStateManager } = await import("../ui-state.svelte.js");
const { StateRestorationManager } = await import("../state-restoration.svelte.js");
const { RecordingLibrary } = await import("../library/recording-library");
const { setLibrary } = await import("../library/index");
const { setAi } = await import("./index");
const { FakeAi } = await import("./testing");
type Turn = import("./testing").FakeTurn;

let library: InstanceType<typeof RecordingLibrary>;
let ai: InstanceType<typeof FakeAi>;

const LOCAL = {
  id: "conn-1",
  name: "Local",
  type: "postgres",
  providerConnectionId: "pc-1",
  activeAIProviderId: "prov-1",
  activeAIModel: "model-1",
};
const OTHER = { ...LOCAL, id: "conn-2", name: "Other", providerConnectionId: "pc-2" };

function dashboardsStub() {
  const created: string[] = [];
  const added: unknown[] = [];
  const manager = {
    createDashboard: vi.fn(async (name: string) => {
      created.push(name);
      return { id: "dash-new", name };
    }),
    addWidget: vi.fn(async (dashboardId: string, widget: unknown) => {
      added.push({ dashboardId, widget });
    }),
    executeWidget: vi.fn(async (_d: string, _w: string, _signal?: AbortSignal) => {}),
    getDashboard: vi.fn(() => null),
    updateWidget: vi.fn(async () => {}),
    removeWidget: vi.fn(async () => {}),
  };
  return { manager, created, added };
}

function setup() {
  const state = new DatabaseState();
  state.activeProjectId = "p";
  state.connections = [LOCAL, OTHER] as never;
  state.activeConnectionIdByProject = { p: "conn-1" };
  const restoration = new StateRestorationManager(state);
  let ui!: InstanceType<typeof UIStateManager>;
  const chats = new AIChatManager(
    state,
    (chatId) => restoration.loadAIChatMessages(chatId),
    (chatId) => ui.abortStreamFor(chatId),
  );
  const dashboards = dashboardsStub();
  ui = new UIStateManager(
    state,
    () => {},
    chats,
    dashboards.manager as unknown as DashboardManager,
    { add: vi.fn() } as unknown as DashboardTabManager,
  );
  return { state, chats, ui, restoration, dashboards };
}

const settle = async () => {
  for (let i = 0; i < 6; i++) await new Promise((r) => setTimeout(r, 0));
};

/** Core's two writes for a turn (the user's row, then the reply), and its `done`. */
async function storeTurn(
  turn: Turn,
  reply: Partial<ChatMessageDraft> & { content: string },
): Promise<{ messages: PersistedAIMessage[]; seq: { epoch: string; n: number } }> {
  const { chatId, userMessage, assistantMessageId } = turn.request;
  const user: ChatMessageDraft = {
    id: userMessage.id,
    role: "user",
    content: userMessage.content,
    timestamp: "2030-01-01T00:00:01.000Z",
  };
  await library.putChatMessages(chatId, [user]);
  const row: ChatMessageDraft = {
    id: assistantMessageId,
    role: "assistant",
    timestamp: "2030-01-01T00:00:02.000Z",
    ...reply,
  };
  const { seq } = await library.putChatMessages(chatId, [row]);
  const messages = [user, row].map((m) => ({ ...m, chatId })) as PersistedAIMessage[];
  return { messages, seq };
}

/** A turn that streams `text` and ends `done`, stored by "Core". */
const say =
  (text: string, extra: Partial<ChatMessageDraft> = {}) =>
  async (turn: Turn) => {
    turn.emit({ type: "started", providerKind: "anthropic", model: "model-1" });
    turn.emit({ type: "text", delta: text.slice(0, 3) });
    turn.emit({ type: "text", delta: text.slice(3) });
    const { messages, seq } = await storeTurn(turn, { content: text, ...extra });
    turn.emit({ type: "done", messages, seq, stop: "end" });
  };

/** A turn asking for one query's approval; answers what the page decided. */
const asking =
  (sql: string, after: (d: unknown, turn: Turn) => Promise<void> | void) => async (turn: Turn) => {
    turn.emit({ type: "text", delta: "Checking. " });
    turn.emit({ type: "toolCall", callId: "call_1", name: "run_query", sql });
    if (turn.request.approval !== "allowAll") {
      turn.emit({ type: "approvalRequired", callId: "call_1", sql });
      const d = await Promise.race([turn.answer("call_1"), turn.stopped().then(() => null)]);
      if (d === null) return;
      await after(d, turn);
      return;
    }
    await after("allowAll-from-start", turn);
  };

const reply = (state: InstanceType<typeof DatabaseState>, chatId: string) =>
  state.aiMessagesByChat[chatId]?.at(-1);

beforeEach(() => {
  env.web = false;
  toasts.length = 0;
  callSecret.mockReset();
  library = new RecordingLibrary();
  library.seedConnection("conn-1");
  library.seedConnection("conn-2");
  setLibrary(library);
  ai = new FakeAi();
  setAi(ai);
});

describe("a turn on Core's events", () => {
  it("sends the turn with the chat's open connection, ids and no key, and streams into the placeholder", async () => {
    const { ui, state } = setup();
    let release!: () => void;
    ai.scripts = [
      async (turn) => {
        turn.emit({ type: "text", delta: "Hel" });
        await new Promise<void>((r) => (release = r));
        await say("Hello there.")(turn);
      },
    ];
    expect(await ui.sendAIMessage("Hi @public.users")).toBe(true);
    await settle();
    const [req] = ai.chats;
    const chatId = state.activeAIChatId!;
    expect(req).toMatchObject({
      chatId,
      connectionId: "pc-1",
      providerId: "prov-1",
      approval: "ask",
      clientTools: true,
      // The page sends what was typed: Core resolves mentions (Decision 13).
      userMessage: { content: "Hi @public.users" },
    });
    expect(req).not.toHaveProperty("apiKey");
    const shown = state.aiMessagesByChat[chatId];
    expect(shown.map((m) => [m.id, m.role, m.content])).toEqual([
      [req.userMessage.id, "user", "Hi @public.users"],
      [req.assistantMessageId, "assistant", "Hel"],
    ]);
    expect(state.isAIStreaming).toBe(true);

    release();
    await settle();
    expect(state.isAIStreaming).toBe(false);
    expect(state.aiMessagesByChat[chatId].map((m) => [m.id, m.content])).toEqual([
      [req.userMessage.id, "Hi @public.users"],
      [req.assistantMessageId, "Hello there."],
    ]);
    // Stored rows replaced the optimistic ones: Core's timestamps.
    expect(reply(state, chatId)!.timestamp.toISOString()).toBe("2030-01-01T00:00:02.000Z");
    // The page stores nothing of a turn itself.
    expect(library.callsOf("putChatMessages")).toHaveLength(2);
    // The desktop page never reads a key.
    expect(callSecret).not.toHaveBeenCalled();
  });

  it("the first message titles the chat (chatUpdate {title, touched}); later ones don't", async () => {
    const { ui, state } = setup();
    ai.scripts = [say("ok")];
    await ui.sendAIMessage("How many orders are there?");
    await settle();
    await ui.sendAIMessage("And yesterday?");
    await settle();
    expect(library.callsOf("updateChat")).toEqual([
      [state.activeAIChatId, { title: "How many orders are there?", touched: true }],
    ]);
  });

  it("a list read older than the turn's done doesn't overwrite the stored reply", async () => {
    const { ui, state, restoration } = setup();
    let staleSeq!: { epoch: string; n: number };
    ai.scripts = [
      async (turn) => {
        staleSeq = (await library.listChatMessages(turn.request.chatId)).seq;
        await say("Fresh reply.")(turn);
      },
    ];
    await ui.sendAIMessage("Q");
    await settle();
    const chatId = state.activeAIChatId!;
    // A list read that started before the reply was stored lands now.
    restoration.restoreAIChatMessages(
      chatId,
      { messages: [], storedBytes: 0, full: false },
      staleSeq,
    );
    expect(reply(state, chatId)?.content).toBe("Fresh reply.");
  });

  it("another window's chatMessages event during a turn waits for the turn", async () => {
    const { ui, state, chats } = setup();
    let finish!: () => void;
    ai.scripts = [
      async (turn) => {
        await new Promise<void>((r) => (finish = r));
        await say("Done.")(turn);
      },
    ];
    await ui.sendAIMessage("hello");
    await settle();
    const chatId = state.activeAIChatId!;
    await library.putChatMessages(chatId, [
      { id: "elsewhere", role: "user", content: "hi", timestamp: "2020-01-01T00:00:00.000Z" },
    ]);
    const reads = () => library.callsOf("listChatMessages").length;
    const before = reads();
    await chats.refreshMessages(chatId);
    expect(reads()).toBe(before);
    expect(state.aiMessagesByChat[chatId].map((m) => m.id)).not.toContain("elsewhere");

    finish();
    await settle();
    expect(reads()).toBe(before + 1);
    expect(state.aiMessagesByChat[chatId].map((m) => m.content)).toEqual(["hi", "hello", "Done."]);
  });

  it("shows Q7's tool lines from the events, and from the stored parts after done", async () => {
    const { ui, state } = setup();
    ai.scripts = [
      async (turn) => {
        turn.emit({ type: "text", delta: "Checking. " });
        turn.emit({ type: "toolCall", callId: "call_1", name: "run_query", sql: "SELECT 1" });
        turn.emit({ type: "toolDone", callId: "call_1", ok: true, rows: 1, truncated: false });
        turn.emit({ type: "text", delta: "One." });
        const { messages, seq } = await storeTurn(turn, {
          content: "Checking. One.",
          parts: [
            { round: 0, type: "text", text: "Checking. " },
            {
              round: 0,
              type: "tool",
              callId: "call_1",
              name: "run_query",
              input: { sql: "SELECT 1" },
              ok: true,
              result: '{"columns":["n"],"rowCount":1,"rows":[[1]],"truncated":false}',
            },
            { round: 1, type: "text", text: "One." },
          ],
        } as never);
        turn.emit({ type: "done", messages, seq, stop: "end" });
      },
    ];
    await ui.sendAIMessage("Count");
    await settle();
    const r = reply(state, state.activeAIChatId!)!;
    expect(r.segments).toEqual([
      { type: "text", text: "Checking. " },
      {
        type: "tool",
        callId: "call_1",
        name: "run_query",
        sql: "SELECT 1",
        state: "ok",
        rows: 1,
        truncated: false,
      },
      { type: "text", text: "One." },
    ]);
  });

  it("a reply cut by max_tokens says so", async () => {
    const { ui, state } = setup();
    ai.scripts = [
      async (turn) => {
        turn.emit({ type: "text", delta: "Half" });
        const { messages, seq } = await storeTurn(turn, { content: "Half" });
        turn.emit({ type: "done", messages, seq, stop: "maxTokens" });
      },
    ];
    await ui.sendAIMessage("Long");
    await settle();
    expect(reply(state, state.activeAIChatId!)).toMatchObject({ content: "Half", truncated: true });
  });

  it("a reply Core cut for its size shows as cut, without Core's note (probe F2)", async () => {
    const { REPLY_CUT_NOTE } = await import("./reply");
    const { ui, state } = setup();
    ai.scripts = [
      async (turn) => {
        turn.emit({ type: "text", delta: "Half" });
        const { messages, seq } = await storeTurn(turn, { content: `Half${REPLY_CUT_NOTE}` });
        turn.emit({ type: "done", messages, seq, stop: "tooLong" });
      },
    ];
    await ui.sendAIMessage("Long");
    await settle();
    const shown = reply(state, state.activeAIChatId!);
    expect(shown).toMatchObject({ content: "Half", cut: true });
    expect(shown?.truncated).toBeUndefined();
    expect(shown?.error).toBeUndefined();
  });
});

describe("the approval card answers through respond", () => {
  it("allow and deny each answer their call", async () => {
    const { ui, state } = setup();
    ai.scripts = [
      asking("SELECT 1", async (_d, turn) => {
        const { messages, seq } = await storeTurn(turn, { content: "Checking. Done." });
        turn.emit({ type: "done", messages, seq, stop: "end" });
      }),
    ];
    await ui.sendAIMessage("one");
    await settle();
    const chatId = state.activeAIChatId!;
    const card = reply(state, chatId)!.pendingApproval!;
    expect(card).toMatchObject({ query: "SELECT 1", connectionName: "Local" });
    expect(reply(state, chatId)!.segments?.at(-1)).toMatchObject({ state: "waiting" });
    card.approve();
    // Answered after a microtask, never from inside the event handler.
    expect(ai.responses).toEqual([]);
    await settle();
    expect(ai.responses).toEqual([
      { streamId: ai.chats[0].streamId, callId: "call_1", decision: "allow" },
    ]);
    expect(reply(state, chatId)!.pendingApproval).toBeFalsy();

    await ui.sendAIMessage("two");
    await settle();
    reply(state, chatId)!.pendingApproval!.deny();
    await settle();
    expect(ai.responses.at(-1)).toMatchObject({ decision: "deny" });
  });

  it("Allow all sticks to the connection for the session, not to every connection", async () => {
    const { ui, state } = setup();
    const end = async (_d: unknown, turn: Turn) => {
      const { messages, seq } = await storeTurn(turn, { content: "ok" });
      turn.emit({ type: "done", messages, seq, stop: "end" });
    };
    ai.scripts = [asking("SELECT 1", end)];
    await ui.sendAIMessage("first");
    await settle();
    reply(state, state.activeAIChatId!)!.pendingApproval!.allowAll();
    await settle();
    expect(ai.responses.at(-1)).toMatchObject({ decision: "allowAll" });
    expect(ui.isAllowAll("conn-1")).toBe(true);

    // The next turn on this connection asks no more.
    await ui.sendAIMessage("second");
    await settle();
    expect(ai.chats[1].approval).toBe("allowAll");

    // A chat on another connection still asks.
    state.activeConnectionIdByProject = { p: "conn-2" };
    await ui.sendAIMessage("third");
    await settle();
    expect(ai.chats[2]).toMatchObject({ connectionId: "pc-2", approval: "ask" });
    expect(ui.isAllowAll("conn-2")).toBe(false);
    expect(ui.isAllowAll("conn-1")).toBe(true);
  });

  it("the Allow all tick is the card's own: a ticked box on conn-1's card isn't ticked on conn-2's (review I3)", async () => {
    const { ui, state } = setup();
    const end = async (_d: unknown, turn: Turn) => {
      const { messages, seq } = await storeTurn(turn, { content: "ok" });
      turn.emit({ type: "done", messages, seq, stop: "end" });
    };
    ai.scripts = [asking("SELECT 1", end)];
    await ui.sendAIMessage("first");
    await settle();
    const first = reply(state, state.activeAIChatId!)!.pendingApproval!;
    expect(first.allowAllTicked).toBe(false);
    first.setAllowAllTicked(true);
    expect(reply(state, state.activeAIChatId!)!.pendingApproval!.allowAllTicked).toBe(true);

    // Before answering, the user moves to a chat on conn-2, whose turn asks too.
    state.activeConnectionIdByProject = { p: "conn-2" };
    await ui.sendAIMessage("second");
    await settle();
    const second = reply(state, state.activeAIChatId!)!.pendingApproval!;
    expect(second.id).not.toBe(first.id);
    expect(second.allowAllTicked).toBe(false);
    second.approve();
    await settle();
    expect(ai.responses.at(-1)).toMatchObject({ decision: "allow" });
    expect(ui.isAllowAll("conn-2")).toBe(false);
  });

  it("approving a ticked card is Allow all for its connection", async () => {
    const { ui, state } = setup();
    ai.scripts = [asking("SELECT 1", () => {})];
    await ui.sendAIMessage("one");
    await settle();
    const card = reply(state, state.activeAIChatId!)!.pendingApproval!;
    card.setAllowAllTicked(true);
    reply(state, state.activeAIChatId!)!.pendingApproval!.approve();
    await settle();
    expect(ai.responses.at(-1)).toMatchObject({ decision: "allowAll" });
    expect(ui.isAllowAll("conn-1")).toBe(true);
  });

  it("Allow all is forgotten when its connection is removed or points elsewhere (review M4)", async () => {
    const { ui, state } = setup();
    const grant = async () => {
      ai.scripts = [asking("SELECT 1", () => {})];
      await ui.sendAIMessage("q");
      await settle();
      reply(state, state.activeAIChatId!)!.pendingApproval!.allowAll();
      await settle();
      ui.cancelAIStream();
      expect(ui.isAllowAll("conn-1")).toBe(true);
    };
    await grant();
    // A rename keeps it.
    state.connections = state.connections.map((c) =>
      c.id === "conn-1" ? { ...c, name: "Renamed" } : c,
    );
    expect(ui.isAllowAll("conn-1")).toBe(true);
    for (const change of [
      { host: "other" },
      { port: 6543 },
      { databaseName: "x" },
      { type: "mysql" },
    ]) {
      state.connections = [{ ...LOCAL, ...change }, OTHER] as never;
      expect(ui.isAllowAll("conn-1"), JSON.stringify(change)).toBe(false);
      // What the connection manager calls once connections change.
      ui.forgetStaleAllowAll();
      state.connections = [LOCAL, OTHER] as never;
      expect(ui.isAllowAll("conn-1")).toBe(false);
      await grant();
    }
    // Reading never writes (review): only a connection change clears it.
    const snapshot = { ...ui.aiAllowAllByConnection };
    state.connections = [OTHER] as never;
    expect(ui.isAllowAll("conn-1")).toBe(false);
    expect(ui.aiAllowAllByConnection).toEqual(snapshot);
    ui.forgetStaleAllowAll();
    expect(ui.aiAllowAllByConnection["conn-1"]).toBeUndefined();
    // Coming back doesn't bring it back.
    state.connections = [LOCAL, OTHER] as never;
    expect(ui.isAllowAll("conn-1")).toBe(false);
  });

  it("Stop during an approval answers nothing, cancels the turn and clears the card", async () => {
    const { ui, state } = setup();
    ai.scripts = [asking("SELECT 1", () => {})];
    await ui.sendAIMessage("one");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(reply(state, chatId)!.pendingApproval).toBeTruthy();
    ui.cancelAIStream();
    await settle();
    expect(ai.turns[0].cancelled).toBe(true);
    expect(ai.responses).toEqual([]);
    expect(reply(state, chatId)).toMatchObject({ content: "Checking. ", pendingApproval: null });
    expect(state.isAIStreaming).toBe(false);
    // Core stores the reply on Stop; the page wrote nothing.
    expect(library.callsOf("putChatMessages")).toEqual([]);
    expect(toasts).toEqual([]);
  });

  it("Stop reads the chat again once the turn ends, since Core stored what streamed (review M5)", async () => {
    const { ui, state } = setup();
    ai.scripts = [asking("SELECT 1", () => {})];
    await ui.sendAIMessage("one");
    await settle();
    const reads = () => library.callsOf("listChatMessages").length;
    const before = reads();
    ui.cancelAIStream();
    await settle();
    expect(reads()).toBe(before + 1);
    expect(state.isAIStreaming).toBe(false);
  });
});

describe("dashboard client tools", () => {
  it("runs handleDashboardToolCall and answers with its result", async () => {
    const { ui, state, dashboards } = setup();
    ai.scripts = [
      async (turn) => {
        turn.emit({
          type: "clientTool",
          callId: "call_1",
          name: "create_dashboard",
          input: { name: "Signups" },
        });
        const d = await turn.answer("call_1");
        const { messages, seq } = await storeTurn(turn, {
          content: "Created.",
          dashboardId: "dash-new",
        });
        turn.emit({ type: "done", messages, seq, stop: "end" });
        void d;
      },
    ];
    await ui.sendAIMessage("dash");
    await settle();
    expect(dashboards.created).toEqual(["Signups"]);
    expect(ai.responses).toEqual([
      {
        streamId: ai.chats[0].streamId,
        callId: "call_1",
        decision: { result: '{"dashboard_id":"dash-new"}' },
      },
    ]);
    expect(reply(state, state.activeAIChatId!)).toMatchObject({ dashboardId: "dash-new" });
  });

  it("refuses a widget change while another connection is active, adding nothing", async () => {
    const { ui, state, dashboards } = setup();
    ai.scripts = [
      async (turn) => {
        state.activeConnectionIdByProject = { p: "conn-2" };
        turn.emit({
          type: "clientTool",
          callId: "call_1",
          name: "add_widget",
          input: { dashboard_id: "d-1", widget_type: "text", text_config: { content: "x" } },
        });
        await turn.answer("call_1");
      },
    ];
    await ui.sendAIMessage("widget");
    await settle();
    expect(dashboards.added).toEqual([]);
    const answer = ai.responses[0].decision as { result: string };
    expect(JSON.parse(answer.result)).toEqual({
      error: 'The active connection is "Other"; switch back to "Local" to change this dashboard',
    });
  });

  it("a widget's first run is on the chat's connection, and Stop cancels it", async () => {
    const { ui, dashboards } = setup();
    let runSignal: AbortSignal | undefined;
    dashboards.manager.executeWidget.mockImplementation(
      async (_d: string, _w: string, signal?: AbortSignal) => {
        runSignal = signal;
        await new Promise<void>((r) => signal?.addEventListener("abort", () => r()));
      },
    );
    ai.scripts = [
      async (turn) => {
        turn.emit({
          type: "clientTool",
          callId: "call_1",
          name: "add_widget",
          input: { dashboard_id: "d-1", widget_type: "kpi", query: "SELECT 1 AS n" },
        });
        await turn.stopped();
      },
    ];
    await ui.sendAIMessage("widget");
    await settle();
    expect(dashboards.added).toHaveLength(1);
    expect(runSignal?.aborted).toBe(false);
    ui.cancelAIStream();
    await settle();
    expect(runSignal?.aborted).toBe(true);
    expect(ai.responses).toEqual([]);
  });

  it("skips a new widget's first run when the connection switched while it was saved", async () => {
    const { ui, state, dashboards } = setup();
    dashboards.manager.addWidget.mockImplementation(
      async (dashboardId: string, widget: unknown) => {
        dashboards.added.push({ dashboardId, widget });
        state.activeConnectionIdByProject = { p: "conn-2" };
      },
    );
    ai.scripts = [
      async (turn) => {
        turn.emit({
          type: "clientTool",
          callId: "call_1",
          name: "add_widget",
          input: { dashboard_id: "d-1", widget_type: "kpi", query: "SELECT 1 AS n" },
        });
        await turn.answer("call_1");
      },
    ];
    await ui.sendAIMessage("widget");
    await settle();
    expect(dashboards.added).toHaveLength(1);
    expect(dashboards.manager.executeWidget).not.toHaveBeenCalled();
    expect(ai.responses).toHaveLength(1);
  });

  it("Stop during a client tool answers nothing", async () => {
    const { ui } = setup();
    let unblock!: () => void;
    ai.scripts = [
      async (turn) => {
        turn.emit({
          type: "clientTool",
          callId: "call_1",
          name: "get_dashboard",
          input: { dashboard_id: "d-1" },
        });
        await new Promise<void>((r) => (unblock = r));
      },
    ];
    await ui.sendAIMessage("get");
    ui.cancelAIStream();
    await settle();
    unblock();
    await settle();
    expect(ai.responses).toEqual([]);
    expect(ai.turns[0].cancelled).toBe(true);
  });
});

describe("how a turn ends", () => {
  it("an error after the user's row applies the stored rows and words the error", async () => {
    const { ui, state } = setup();
    ai.scripts = [
      async (turn) => {
        turn.emit({ type: "text", delta: "Half an ans" });
        const { messages, seq } = await storeTurn(turn, { content: "Half an ans" });
        turn.emit({ type: "error", code: "PROVIDER_ERROR", message: "Overloaded", messages, seq });
      },
    ];
    await ui.sendAIMessage("Explain");
    await settle();
    expect(reply(state, state.activeAIChatId!)).toMatchObject({
      content: "Half an ans",
      error: "AI provider error: Overloaded",
    });
    expect(state.isAIStreaming).toBe(false);
  });

  it("Core's own refusal stores nothing and says why on the reply", async () => {
    const { ui, state } = setup();
    ai.scripts = [
      (turn) =>
        turn.emit({
          type: "error",
          code: "NO_API_KEY",
          message: "No API key is set for this provider.",
        }),
    ];
    await ui.sendAIMessage("Which tables are the largest?");
    await settle();
    expect(reply(state, state.activeAIChatId!)).toMatchObject({
      content: "",
      error: "No API key is set for this provider. Add your key in Settings → AI.",
    });
    expect(library.callsOf("putChatMessages")).toEqual([]);
  });

  it("CHAT_FULL marks the chat full and keeps the turn on screen", async () => {
    const { ui, state } = setup();
    ai.scripts = [
      (turn) => turn.emit({ type: "error", code: "CHAT_FULL", message: "This chat is full." }),
    ];
    await ui.sendAIMessage("More");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(state.aiChatFull[chatId]).toBe(true);
    expect(state.aiMessagesByChat[chatId].map((m) => m.role)).toEqual(["user", "assistant"]);
    expect(toasts).toEqual(["This chat is full. Start a new chat to continue."]);
    // A full chat takes no more sends.
    expect(await ui.sendAIMessage("again")).toBe(false);
    expect(ai.chats).toHaveLength(1);
  });

  it("a chat deleted during a turn cancels it", async () => {
    const { ui, state, chats } = setup();
    ai.scripts = [
      async (turn) => {
        await turn.stopped();
      },
    ];
    await ui.sendAIMessage("hello");
    await settle();
    const chatId = state.activeAIChatId!;
    await chats.deleteChat(chatId);
    await settle();
    expect(ai.turns[0].cancelled).toBe(true);
    expect(state.isAIStreaming).toBe(false);
    expect(state.aiMessagesByChat[chatId]).toBeUndefined();
  });

  it("deleting another chat leaves the turn running", async () => {
    const { ui, state, chats } = setup();
    ai.scripts = [
      async (turn) => {
        await turn.stopped();
      },
    ];
    const other = (await chats.createChat("Other"))!;
    await chats.createChat("Streaming");
    await ui.sendAIMessage("hello");
    await settle();
    await chats.deleteChat(other);
    await settle();
    expect(ai.turns[0].cancelled).toBe(false);
    expect(state.isAIStreaming).toBe(true);
  });

  it("a stream that throws ends the turn with its error", async () => {
    const { ui, state } = setup();
    ai.chat = () => ({
      [Symbol.asyncIterator]: () => ({
        next: () => Promise.reject(new TypeError("Failed to fetch")),
      }),
    });
    await ui.sendAIMessage("hello");
    await settle();
    expect(state.isAIStreaming).toBe(false);
    expect(reply(state, state.activeAIChatId!)?.error).toBe(
      "Something went wrong (UNKNOWN). Try again.",
    );
  });

  it("sending in another chat stops the streaming one", async () => {
    const { ui, state, chats } = setup();
    ai.scripts = [
      async (turn) => {
        await turn.stopped();
      },
      say("ok"),
    ];
    await ui.sendAIMessage("first");
    await settle();
    await chats.createChat("Second");
    await ui.sendAIMessage("second");
    await settle();
    expect(ai.turns[0].cancelled).toBe(true);
    expect(state.isAIStreaming).toBe(false);
  });
});

describe("what the page stops itself", () => {
  it("no model: a pending message, no turn; once a model is chosen it's sent with its own id", async () => {
    const { ui, state } = setup();
    state.connections = [{ ...LOCAL, activeAIModel: undefined }, OTHER] as never;
    ai.scripts = [say("Hi.")];
    await ui.sendAIMessage("Hello there, assistant");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(ai.chats).toEqual([]);
    const [user, pending] = state.aiMessagesByChat[chatId];
    expect(pending.pendingModelSelection).toBe("Hello there, assistant");

    state.connections = [LOCAL, OTHER] as never;
    ui.retryPendingMessage(pending.id);
    await settle();
    expect(ai.chats).toHaveLength(1);
    expect(ai.chats[0].userMessage).toEqual({ id: user.id, content: "Hello there, assistant" });
    expect(state.aiMessagesByChat[chatId].map((m) => m.content)).toEqual([
      "Hello there, assistant",
      "Hi.",
    ]);
  });

  it("a connection that isn't open: no turn, and the reply says to connect", async () => {
    const { ui, state } = setup();
    state.connections = [{ ...LOCAL, providerConnectionId: undefined }, OTHER] as never;
    await ui.sendAIMessage("Hello");
    await settle();
    expect(ai.chats).toEqual([]);
    expect(reply(state, state.activeAIChatId!)).toMatchObject({
      error: "Connect Local to ask about it.",
    });
  });
});

describe("keys on web", () => {
  it("the page's turn carries the provider for the vault's key; the service adds it", async () => {
    env.web = true;
    const { ui } = setup();
    ai.scripts = [say("ok")];
    await ui.sendAIMessage("hi");
    await settle();
    expect(ai.chats[0].providerId).toBe("prov-1");
  });
});
