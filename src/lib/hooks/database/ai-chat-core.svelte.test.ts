/**
 * AI chats through Core (phase 5d-2, Decision 24; phase 6): a chat is
 * created at once with Core's id, and a turn's messages are Core's to
 * store (the page sends none). The web's per-chat budget (Q17) fills a
 * chat: Core refuses the turn with `CHAT_FULL`, the refused turn stays on
 * screen, sending stops there, and a chat already full opens that way.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { DashboardManager } from "./dashboard-manager.svelte.js";
import type { DashboardTabManager } from "./dashboard-tabs.svelte.js";

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
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { DatabaseState } = await import("./state.svelte.js");
const { AIChatManager } = await import("./ai-chat-manager.svelte.js");
const { UIStateManager } = await import("./ui-state.svelte.js");
const { StateRestorationManager } = await import("./state-restoration.svelte.js");
const { RecordingLibrary } = await import("./library/recording-library");
const { setLibrary, LibraryCallError } = await import("./library/index");
const { setAi } = await import("./ai/index");
const { FakeAi } = await import("./ai/testing");
type Turn = import("./ai/testing").FakeTurn;

let library: InstanceType<typeof RecordingLibrary>;
let ai: InstanceType<typeof FakeAi>;

function setup() {
  const state = new DatabaseState();
  state.activeProjectId = "p";
  state.connections = [
    {
      id: "conn-1",
      name: "Local",
      type: "postgres",
      providerConnectionId: "pc-1",
      activeAIProviderId: "prov",
      activeAIModel: "model",
    } as never,
  ];
  state.activeConnectionIdByProject = { p: "conn-1" };
  const restoration = new StateRestorationManager(state);
  let ui!: InstanceType<typeof UIStateManager>;
  const chats = new AIChatManager(
    state,
    (chatId) => restoration.loadAIChatMessages(chatId),
    (chatId) => ui.abortStreamFor(chatId),
  );
  ui = new UIStateManager(
    state,
    () => {},
    chats,
    {} as DashboardManager,
    {} as DashboardTabManager,
  );
  return { state, chats, ui, restoration };
}

/** A turn Core stores: the user's row, then the reply, then `done`. */
const say = (text: string) => async (turn: Turn) => {
  turn.emit({ type: "text", delta: text });
  const { chatId, userMessage, assistantMessageId } = turn.request;
  const user = {
    id: userMessage.id,
    role: "user" as const,
    content: userMessage.content,
    timestamp: "2030-01-01T00:00:00.000Z",
  };
  await library.putChatMessages(chatId, [user]);
  const reply = {
    id: assistantMessageId,
    role: "assistant" as const,
    content: text,
    timestamp: "2030-01-01T00:00:01.000Z",
  };
  const { seq } = await library.putChatMessages(chatId, [reply]);
  turn.emit({
    type: "done",
    messages: [user, reply].map((m) => ({ ...m, chatId })),
    seq,
    stop: "end",
  });
};
/** Core refuses the turn: the chat is full. */
const full = (turn: Turn) =>
  turn.emit({ type: "error", code: "CHAT_FULL", message: "This chat is full." });

const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

beforeEach(() => {
  env.web = false;
  toasts.length = 0;
  library = new RecordingLibrary();
  library.seedConnection("conn-1");
  setLibrary(library);
  ai = new FakeAi();
  setAi(ai);
});

describe("AI chats through Core", () => {
  it("Core makes the chat at once and stores each turn; the page puts nothing", async () => {
    const { state, ui } = setup();
    ai.scripts = [say("hello there"), say("again")];
    await ui.sendAIMessage("first");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(library.callsOf("createChat")).toHaveLength(1);
    expect(library.chats.has(chatId)).toBe(true);

    await ui.sendAIMessage("second");
    await settle();
    // Only "Core" (the fake turn) wrote messages: two writes per turn.
    expect(library.callsOf("putChatMessages")).toHaveLength(4);
    expect(library.messages.get(chatId)?.map((m) => m.content)).toEqual([
      "first",
      "hello there",
      "second",
      "again",
    ]);
    expect(state.aiMessagesByChat[chatId].map((m) => m.content)).toEqual([
      "first",
      "hello there",
      "second",
      "again",
    ]);
  });

  it("a full chat says so, keeps the turn on screen and disables sending", async () => {
    const { state, ui } = setup();
    ai.scripts = [full];
    await ui.sendAIMessage("question");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(toasts).toEqual(["This chat is full. Start a new chat to continue."]);
    expect(state.aiMessagesByChat[chatId].map((m) => m.role)).toEqual(["user", "assistant"]);
    expect(state.aiChatFull[chatId]).toBe(true);
    expect(await ui.sendAIMessage("more")).toBe(false);
    expect(ai.chats).toHaveLength(1);
  });

  it("a full chat opens disabled", async () => {
    // Core says so (`ChatMessages.full`, from its own budget).
    const { state, restoration } = setup();
    const { value: chat } = await library.createChat({ connectionId: "conn-1", title: "Big" });
    await library.putChatMessages(chat.id, [
      { id: "m1", role: "user", content: "hi", timestamp: "2030-01-01T00:00:00.000Z" },
    ]);
    library.full.add(chat.id);

    await restoration.loadAIChats("conn-1");

    expect(state.activeAIChatIdByConnection["conn-1"]).toBe(chat.id);
    expect(state.aiMessagesByChat[chat.id].map((m) => m.id)).toEqual(["m1"]);
    expect(state.aiChatFull[chat.id]).toBe(true);
  });

  it("a reload doesn't clear a full chat a refusal marked, and keeps the refused turn", async () => {
    const { state, ui, restoration } = setup();
    ai.scripts = [full];
    await ui.sendAIMessage("question");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(state.aiChatFull[chatId]).toBe(true);

    await restoration.loadAIChatMessages(chatId);
    expect(state.aiChatFull[chatId]).toBe(true);
    // Not stored, so still shown after the read.
    expect(state.aiMessagesByChat[chatId].map((m) => m.content)).toEqual(["question", ""]);
  });

  it("a stored message deleted elsewhere leaves on the next read", async () => {
    const { state, ui, restoration } = setup();
    ai.scripts = [say("answer")];
    await ui.sendAIMessage("question");
    await settle();
    const chatId = state.activeAIChatId!;
    library.messages.set(chatId, []);
    await restoration.loadAIChatMessages(chatId);
    expect(state.aiMessagesByChat[chatId]).toEqual([]);
  });

  it("two quick sends before the chat exists make one chat", async () => {
    const { ui } = setup();
    ai.scripts = [say("answer")];
    await Promise.all([ui.sendAIMessage("one"), ui.sendAIMessage("two")]);
    await settle();
    expect(library.callsOf("createChat")).toHaveLength(1);
  });

  it("a refused chat create sends nothing and says so", async () => {
    const { ui } = setup();
    library.failures.set("createChat", { error: new LibraryCallError("STORAGE_FULL", "full") });
    expect(await ui.sendAIMessage("question")).toBe(false);
    expect(ai.chats).toHaveLength(0);
    expect(toasts).toHaveLength(1);
  });

  it("a refused chat title is said", async () => {
    const { ui } = setup();
    library.failures.set("updateChat", { error: new LibraryCallError("STORAGE_FULL", "full") });
    ai.scripts = [say("answer")];
    await ui.sendAIMessage("question");
    await settle();
    expect(toasts).toEqual([expect.stringMatching(/size limit/)]);
  });
});
