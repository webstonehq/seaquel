/**
 * A chat turn that fails outside the provider's own handling, and a chat
 * deleted while it streams (re-survey bugs 10 and 11).
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SendAIMessageParams } from "$lib/services/ai";
import type { DashboardManager } from "./dashboard-manager.svelte.js";
import type { DashboardTabManager } from "./dashboard-tabs.svelte.js";

/** What the fake service does with each turn. */
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
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));

const { DatabaseState } = await import("./state.svelte.js");
const { AIChatManager } = await import("./ai-chat-manager.svelte.js");
const { UIStateManager } = await import("./ui-state.svelte.js");
const { RecordingLibrary } = await import("./library/recording-library");
const { setLibrary } = await import("./library/index");

/** Every persistence call, in order: message saves and chat removals. */
const log: string[] = [];

/** Chats through Core (5d-2): a removal is the library's `removeChat`. */
class ChatLibrary extends RecordingLibrary {
  override async removeChat(id: string) {
    log.push(`remove:${id}`);
    return super.removeChat(id);
  }
}

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
  let ui: InstanceType<typeof UIStateManager>;
  const chats = new AIChatManager(
    state,
    async () => {},
    (chatId) => ui.abortStreamFor(chatId),
  );
  ui = new UIStateManager(
    state,
    () => {},
    async () => ({ rows: [], truncated: false }),
    chats,
    async (chatId) => {
      log.push(`persist:${chatId}`);
    },
    {} as DashboardManager,
    {} as DashboardTabManager,
  );
  return { state, chats, ui };
}

const settle = () => new Promise((r) => setTimeout(r, 0));

beforeEach(() => {
  const library = new ChatLibrary();
  library.seedConnection("conn-1");
  setLibrary(library);
  log.length = 0;
  sent.length = 0;
  behaviour = async () => {};
});

describe("a chat turn", () => {
  it("a stream that throws ends the turn and saves it", async () => {
    behaviour = async () => {
      throw new TypeError("Failed to fetch");
    };
    const { state, ui } = setup();
    await ui.sendAIMessage("hello");
    const chatId = state.activeAIChatId!;
    expect(sent).toHaveLength(1);

    await settle();

    expect(state.isAIStreaming).toBe(false);
    expect(state.aiStreamingChatId).toBeNull();
    const assistant = state.aiMessagesByChat[chatId].at(-1)!;
    expect(assistant.role).toBe("assistant");
    expect(assistant.content).toBe("Error: Failed to fetch");
    expect(log).toEqual([`persist:${chatId}`]);
  });

  it("a stream aborted by Stop doesn't turn into an error", async () => {
    behaviour = (p) =>
      new Promise((_resolve, reject) => {
        p.signal!.addEventListener("abort", () =>
          reject(new DOMException("Aborted", "AbortError")),
        );
      });
    const { state, ui } = setup();
    await ui.sendAIMessage("hello");
    const chatId = state.activeAIChatId!;
    ui.cancelAIStream();
    await settle();
    expect(state.aiMessagesByChat[chatId].at(-1)!.content).toBe("");
  });

  it("deleting a streaming chat aborts its turn and saves nothing of it", async () => {
    // The model asks to run a query; the approval waits on the user. How an
    // abort settles it is `handleToolCall`'s (the ui-state suite's "Stop
    // resolves every pending approval"); here only the abort and the saves.
    behaviour = (p) => {
      p.onApprovalRequired?.(
        "SELECT 1",
        { id: "conn-1", name: "Local", type: "postgres" } as never,
        () => {},
        () => {},
      );
      return new Promise(() => {});
    };
    const { state, chats, ui } = setup();
    await ui.sendAIMessage("hello");
    const chatId = state.activeAIChatId!;
    const signal = sent[0].signal!;
    expect(state.aiStreamingChatId).toBe(chatId);
    expect(state.aiMessagesByChat[chatId].at(-1)?.pendingApproval).toBeTruthy();

    await chats.deleteChat(chatId);

    expect(signal.aborted).toBe(true);
    expect(state.isAIStreaming).toBe(false);
    expect(state.aiStreamingChatId).toBeNull();
    // Nothing of the deleted chat is saved again, before or after its removal.
    expect(log).toEqual([`remove:${chatId}`]);
  });

  it("deleting another chat leaves the stream running", async () => {
    behaviour = () => new Promise(() => {});
    const { state, chats, ui } = setup();
    const other = (await chats.createChat("other"))!;
    await chats.createChat("streaming");
    await ui.sendAIMessage("hello");
    const signal = sent[0].signal!;

    await chats.deleteChat(other);

    expect(signal.aborted).toBe(false);
    expect(state.isAIStreaming).toBe(true);
  });
});
