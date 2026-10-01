/**
 * AI chats through Core (phase 5d-2, Decision 24): a chat is created at
 * once with Core's id, a turn's messages are upserted by id and only the
 * ones that changed are sent, and the web's per-chat budget (Q17) fills a
 * chat: the refused turn stays on screen, sending stops there, and a chat
 * already full opens that way.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SendAIMessageParams } from "$lib/services/ai";
import type { DashboardManager } from "./dashboard-manager.svelte.js";
import type { DashboardTabManager } from "./dashboard-tabs.svelte.js";

const env = vi.hoisted(() => ({ web: false }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => !env.web,
  isWeb: () => env.web,
  isDemo: () => false,
}));
let behaviour: (p: SendAIMessageParams) => Promise<void> = async () => {};
const sent: SendAIMessageParams[] = [];
vi.mock("$lib/services/ai", () => ({
  sendAIMessage: vi.fn((p: SendAIMessageParams) => {
    sent.push(p);
    return behaviour(p);
  }),
}));
vi.mock("$lib/services/ai-mentions", () => ({ resolveMentions: (c: string) => c }));
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
import type { ChatMessageDraft } from "./library/types";

let library: InstanceType<typeof RecordingLibrary>;

function setup() {
  const state = new DatabaseState();
  state.activeProjectId = "p";
  state.connections = [
    {
      id: "conn-1",
      name: "Local",
      type: "postgres",
      activeAIProviderId: "prov",
      activeAIModel: "model",
    } as never,
  ];
  state.activeConnectionIdByProject = { p: "conn-1" };
  const restoration = new StateRestorationManager(state);
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
    {} as DashboardManager,
    {} as DashboardTabManager,
  );
  return { state, chats, ui: Object.assign(ui, { chats }), restoration };
}

const FULL_BYTES = () =>
  new LibraryCallError(
    "INVALID_ARGUMENT",
    "This chat holds more than allowed here (max_chat_bytes: 67108864 bytes).",
  );

/** A turn: two chunks, then the end. */
const say = (text: string) => async (p: SendAIMessageParams) => {
  p.onChunk(text.slice(0, 2));
  p.onChunk(text.slice(2));
  p.onDone();
};

const settle = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

/** The ids each put carried, in order. */
const puts = () =>
  library
    .callsOf("putChatMessages")
    .map(([, messages]) => (messages as ChatMessageDraft[]).map((m) => m.role));

beforeEach(() => {
  env.web = false;
  toasts.length = 0;
  sent.length = 0;
  library = new RecordingLibrary();
  library.seedConnection("conn-1");
  setLibrary(library);
});

describe("AI chats through Core", () => {
  it("a message put carries only the changed messages", async () => {
    const { state, ui } = setup();
    behaviour = say("hello there");
    await ui.sendAIMessage("first");
    await settle();
    const chatId = state.activeAIChatId!;
    // Core made the chat, at once.
    expect(library.callsOf("createChat")).toHaveLength(1);
    expect(library.chats.has(chatId)).toBe(true);

    behaviour = say("again");
    await ui.sendAIMessage("second");
    await settle();

    // Each turn put its own two messages, not the chat's whole list.
    expect(puts()).toEqual([
      ["user", "assistant"],
      ["user", "assistant"],
    ]);
    const second = library.callsOf("putChatMessages")[1][1] as ChatMessageDraft[];
    expect(second.map((m) => m.content)).toEqual(["second", "again"]);
    expect(library.messages.get(chatId)?.map((m) => m.content)).toEqual([
      "first",
      "hello there",
      "second",
      "again",
    ]);
  });

  it("a full chat says so, keeps the turn on screen and disables sending", async () => {
    const { state, ui } = setup();
    library.failures.set("putChatMessages", {
      error: new LibraryCallError(
        "INVALID_ARGUMENT",
        "This chat holds more than allowed here (max_chat_bytes: 67108864 bytes).",
      ),
      sticky: true,
    });
    behaviour = say("a long answer");
    await ui.sendAIMessage("question");
    await settle();
    const chatId = state.activeAIChatId!;

    expect(toasts).toEqual(["This chat is full. Start a new chat to continue."]);
    // The refused turn stays on screen, not stored.
    expect(state.aiMessagesByChat[chatId].map((m) => m.content)).toEqual([
      "question",
      "a long answer",
    ]);
    expect(state.aiChatFull[chatId]).toBe(true);

    // Sending there does nothing.
    const turns = sent.length;
    await ui.sendAIMessage("more");
    expect(sent.length).toBe(turns);
    expect(state.aiMessagesByChat[chatId]).toHaveLength(2);
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

  it("a reload doesn't clear a full chat a refusal marked", async () => {
    const { state, ui, restoration } = setup();
    library.failures.set("putChatMessages", { error: FULL_BYTES(), sticky: true });
    behaviour = say("answer");
    await ui.sendAIMessage("question");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(state.aiChatFull[chatId]).toBe(true);

    await restoration.loadAIChatMessages(chatId);
    expect(state.aiChatFull[chatId]).toBe(true);
  });

  it("a turn a failed put didn't store survives a refetch, and is sent again", async () => {
    const { state, ui, chats, restoration } = setup();
    library.failures.set("putChatMessages", {
      error: new LibraryCallError("STORAGE_ERROR", "busy"),
    });
    behaviour = say("first answer");
    await ui.sendAIMessage("first");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(library.messages.get(chatId) ?? []).toEqual([]);

    // Another window's change makes the page read the chat again.
    await restoration.loadAIChatMessages(chatId);
    expect(state.aiMessagesByChat[chatId].map((m) => m.content)).toEqual(["first", "first answer"]);

    await chats.persistMessages(chatId);
    expect(library.messages.get(chatId)?.map((m) => m.content)).toEqual(["first", "first answer"]);
  });

  it("a chat at its message count is full", async () => {
    const { state, ui } = setup();
    library.failures.set("putChatMessages", {
      error: new LibraryCallError(
        "INVALID_ARGUMENT",
        "This chat has more messages than allowed here (max_messages_per_chat: 5000).",
      ),
      sticky: true,
    });
    behaviour = say("answer");
    await ui.sendAIMessage("question");
    await settle();
    expect(state.aiChatFull[state.activeAIChatId!]).toBe(true);
    expect(toasts).toEqual(["This chat is full. Start a new chat to continue."]);
  });

  it("a message over the size limit is said once and left out of later puts", async () => {
    const { state, ui } = setup();
    library.failures.set("putChatMessages", {
      error: new LibraryCallError(
        "INVALID_ARGUMENT",
        "The message is longer than allowed here (max_message_bytes: 10 bytes).",
      ),
    });
    behaviour = say("ok");
    await ui.sendAIMessage("a question far longer than ten bytes");
    await settle();
    const chatId = state.activeAIChatId!;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toContain("max_message_bytes");
    expect(state.aiChatFull[chatId]).toBeUndefined();

    behaviour = say("fine");
    await ui.sendAIMessage("short");
    await settle();
    expect(toasts).toHaveLength(1);
    // The long message stays on screen but is never sent again.
    expect(library.messages.get(chatId)?.map((m) => m.content)).toEqual(["ok", "short", "fine"]);
  });

  it("each oversize message is said, and one with an oversize query is left out too", async () => {
    const { state, ui } = setup();
    const refuse = (name: string) =>
      new LibraryCallError(
        "INVALID_ARGUMENT",
        `The message is larger than allowed here (${name}: 10 bytes).`,
      );
    library.failures.set("putChatMessages", { error: refuse("max_message_bytes") });
    behaviour = say("ok");
    await ui.sendAIMessage("the first question far longer than ten bytes");
    await settle();
    library.failures.set("putChatMessages", { error: refuse("max_message_bytes") });
    behaviour = say("ok2");
    await ui.sendAIMessage("the second question far longer than ten bytes");
    await settle();
    expect(toasts).toHaveLength(2);

    // An answer whose query is too large is left out the same way.
    const chatId = state.activeAIChatId!;
    behaviour = (p) => {
      p.onChunk("with query");
      return new Promise(() => {});
    };
    await ui.sendAIMessage("q");
    const last = state.aiMessagesByChat[chatId].at(-1)!;
    state.aiMessagesByChat = {
      ...state.aiMessagesByChat,
      [chatId]: state.aiMessagesByChat[chatId].map((m) =>
        m.id === last.id ? { ...m, query: "SELECT * FROM a_long_table_name" } : m,
      ),
    };
    library.failures.set("putChatMessages", { error: refuse("max_query_bytes") });
    await ui.chats.persistMessages(chatId);
    await settle();
    expect(toasts).toHaveLength(3);
    expect(library.messages.get(chatId)?.map((m) => m.content)).not.toContain("with query");
  });

  it("a deleted chat forgets which of its messages were too large", async () => {
    const { state, ui, chats } = setup();
    library.failures.set("putChatMessages", {
      error: new LibraryCallError(
        "INVALID_ARGUMENT",
        "The message is longer than allowed here (max_message_bytes: 10 bytes).",
      ),
    });
    behaviour = say("ok");
    await ui.sendAIMessage("a question far longer than ten bytes");
    await settle();
    const chatId = state.activeAIChatId!;
    const unsendable = (chats as unknown as { unsendable: Map<string, Set<string>> }).unsendable;
    expect(unsendable.has(chatId)).toBe(true);
    await chats.deleteChat(chatId);
    expect(unsendable.has(chatId)).toBe(false);
  });

  it("an oversize refusal it can't read is said as an error", async () => {
    const { ui } = setup();
    library.failures.set("putChatMessages", {
      error: new LibraryCallError("INVALID_ARGUMENT", "Too large (max_message_bytes)."),
    });
    behaviour = say("ok");
    await ui.sendAIMessage("question");
    await settle();
    expect(toasts).toEqual([expect.stringContaining("Couldn't save the chat")]);
  });

  it("Stop saves the chat that streams, not the active one", async () => {
    const { state, ui, chats } = setup();
    behaviour = (p) => {
      p.onChunk("partial");
      return new Promise(() => {});
    };
    await ui.sendAIMessage("question");
    const streaming = state.activeAIChatId!;
    const other = (await chats.createChat("Other"))!;
    expect(state.activeAIChatId).toBe(other);

    ui.cancelAIStream();
    await settle();
    expect(library.messages.get(streaming)?.map((m) => m.content)).toEqual(["question", "partial"]);
  });

  it("sending in another chat saves the streaming chat's partial turn first", async () => {
    const { state, ui, chats } = setup();
    behaviour = (p) => {
      p.onChunk("partial");
      return new Promise(() => {});
    };
    await ui.sendAIMessage("first");
    const streaming = state.activeAIChatId!;
    await chats.createChat("Other");
    behaviour = say("done");
    await ui.sendAIMessage("second");
    await settle();
    expect(library.messages.get(streaming)?.map((m) => m.content)).toEqual(["first", "partial"]);
  });

  it("two quick sends before the chat exists make one chat", async () => {
    const { ui } = setup();
    behaviour = say("answer");
    await Promise.all([ui.sendAIMessage("one"), ui.sendAIMessage("two")]);
    await settle();
    expect(library.callsOf("createChat")).toHaveLength(1);
  });

  it("a refused chat create sends nothing and says so", async () => {
    const { ui } = setup();
    library.failures.set("createChat", { error: new LibraryCallError("STORAGE_FULL", "full") });
    expect(await ui.sendAIMessage("question")).toBe(false);
    expect(sent).toHaveLength(0);
    expect(toasts).toHaveLength(1);
  });

  it("a refused chat title is said", async () => {
    const { ui } = setup();
    library.failures.set("updateChat", { error: new LibraryCallError("STORAGE_FULL", "full") });
    behaviour = say("answer");
    await ui.sendAIMessage("question");
    await settle();
    expect(toasts).toEqual([expect.stringMatching(/size limit/)]);
  });
});
