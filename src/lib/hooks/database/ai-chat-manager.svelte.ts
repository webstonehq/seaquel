import type { AIChat } from "$lib/types";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import { toast } from "svelte-sonner";
import { log } from "$lib/utils/logger";
import type { DatabaseState } from "./state.svelte.js";
import { getLibrary, rowKey, NEW, type ChangeSeq } from "./library/index.js";
import { chatFromWire, messageFromWire } from "./library/convert.js";
import { libraryErrorMessage, limitMessage, limitOf } from "./library/messages.js";
import { withLiveView } from "./ai/events.js";
import type { PersistedAIMessage } from "$lib/types/generated/PersistedAIMessage";

/**
 * AI chats (phase 5d-2): a chat is created at once through
 * Core (`chatCreate`, Core's id) and its title is a `chatUpdate`. Its
 * messages are Core's to store since phase 6: a turn's `done` or `error`
 * carries the rows Core stored, applied here by their `seq`. A turn past
 * the web's per-chat budget (`CHAT_FULL`) marks the chat full: the
 * refused turn stays on screen, and sending there stops.
 */
export class AIChatManager {
  /** Chats whose `chatMessages` event came while they streamed: read again after the turn. */
  private refetchAfterTurn = new Set<string>();
  /**
   * Chats whose turn was stopped and whose reply Core stores after the
   * stream ended: their own-origin `chatMessages` event is heard (the
   * feed's `acceptOwn`) until one is applied.
   */
  private awaitingOwnStore = new Set<string>();
  /** A create on its way per connection: a second send waits for it. */
  private creating = new Map<string, Promise<string | null>>();

  constructor(
    private state: DatabaseState,
    private loadMessagesFromDb: (chatId: string) => Promise<void>,
    /** Stops a turn streaming on the chat, if one is (`UIStateManager.abortStreamFor`). */
    private abortStream: (chatId: string) => void = () => {},
  ) {}

  /**
   * Save a new chat on the active connection (Core's id) and make it the
   * active one. `null` when there's no connection or Core refused it (shown).
   */
  async createChat(title?: string): Promise<string | null> {
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) return null;

    const seqs = this.state.librarySeqs;
    let chat: AIChat;
    try {
      const { value, seq } = await seqs.write([rowKey("chat", NEW)], () =>
        getLibrary().createChat({ connectionId, title: title || "New Chat" }),
      );
      seqs.note(rowKey("chat", value.id), seq);
      chat = chatFromWire(value);
    } catch (error) {
      void log.error("Failed to create a chat:", error);
      errorToast(m.ai_chat_save_failed({ message: libraryErrorMessage(error) }));
      return null;
    }

    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: [chat, ...(this.state.aiChatsByConnection[connectionId] ?? [])],
    };
    this.state.aiMessagesByChat = {
      ...this.state.aiMessagesByChat,
      [chat.id]: [],
    };
    this.state.aiMessagesStored.set(chat.id, new Set());
    this.state.activeAIChatIdByConnection = {
      ...this.state.activeAIChatIdByConnection,
      [connectionId]: chat.id,
    };
    return chat.id;
  }

  async switchChat(chatId: string): Promise<void> {
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) return;

    this.state.activeAIChatIdByConnection = {
      ...this.state.activeAIChatIdByConnection,
      [connectionId]: chatId,
    };

    if (!(chatId in this.state.aiMessagesByChat)) {
      await this.loadMessagesFromDb(chatId);
    }
  }

  /**
   * Delete a chat (`chatRemove`), stopping its turn first: a pending
   * approval settles, and the end of the turn can't save messages for the
   * chat after it's removed.
   */
  async deleteChat(chatId: string): Promise<void> {
    const connectionId = this.chatConnectionId(chatId) ?? this.state.activeConnectionId;
    if (!connectionId) return;
    this.abortStream(chatId);
    try {
      const { seq } = await this.state.librarySeqs.write([rowKey("chat", chatId)], () =>
        getLibrary().removeChat(chatId),
      );
      this.state.librarySeqs.note(rowKey("chat", chatId), seq);
    } catch (error) {
      void log.error(`Failed to remove AI chat ${chatId}:`, error);
      errorToast(m.ai_chat_save_failed({ message: libraryErrorMessage(error) }));
      return;
    }
    await this.forget(connectionId, chatId);
  }

  /**
   * A chat gone from the page (deleted here or in another window): out of
   * the lists, and when it was the connection's active one, the next chat
   * becomes active and its messages load.
   */
  private async forget(connectionId: string, chatId: string): Promise<void> {
    const chats = this.state.aiChatsByConnection[connectionId] ?? [];
    const remaining = chats.filter((c) => c.id !== chatId);

    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: remaining,
    };

    const { [chatId]: _, ...restMessages } = this.state.aiMessagesByChat;
    this.state.aiMessagesByChat = restMessages;
    this.state.aiMessagesStored.delete(chatId);
    this.refetchAfterTurn.delete(chatId);
    this.awaitingOwnStore.delete(chatId);
    if (this.state.aiChatFull[chatId]) {
      const { [chatId]: _full, ...rest } = this.state.aiChatFull;
      this.state.aiChatFull = rest;
    }

    // Switch to next chat or clear
    if (this.state.activeAIChatIdByConnection[connectionId] === chatId) {
      this.state.activeAIChatIdByConnection = {
        ...this.state.activeAIChatIdByConnection,
        [connectionId]: remaining[0]?.id ?? null,
      };
      // Load messages for the new active chat if needed
      if (remaining[0] && !(remaining[0].id in this.state.aiMessagesByChat)) {
        await this.loadMessagesFromDb(remaining[0].id);
      }
    }
  }

  /** The connection a chat belongs to, whatever is active now. */
  private chatConnectionId(chatId: string): string | undefined {
    return Object.values(this.state.aiChatsByConnection)
      .flat()
      .find((c) => c.id === chatId)?.connectionId;
  }

  /**
   * The active chat, created when there's none (once: a second call while
   * the create is on its way gets the same chat). `null` without a
   * connection, or when Core refused the create (shown).
   */
  async ensureActiveChat(): Promise<string | null> {
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) return null;

    const activeChatId = this.state.activeAIChatIdByConnection[connectionId] ?? null;
    if (activeChatId) return activeChatId;

    let pending = this.creating.get(connectionId);
    if (!pending) {
      pending = this.createChat().finally(() => this.creating.delete(connectionId));
      this.creating.set(connectionId, pending);
    }
    return pending;
  }

  updateChatTitle(chatId: string, firstMessage: string): void {
    const connectionId = this.chatConnectionId(chatId);
    if (!connectionId) return;

    const title = firstMessage.slice(0, 50) + (firstMessage.length > 50 ? "..." : "");
    this.showChat(connectionId, chatId, (c) => ({ ...c, title, updatedAt: new Date() }));
    void this.saveChat(chatId, { title, touched: true });
  }

  /**
   * A reply was stored (which may be after the active connection changed):
   * Core touched the chat with it, and its `chat` event carries this page's
   * origin, so the list's time is updated here.
   */
  updateChatTimestamp(chatId: string): void {
    const connectionId = this.chatConnectionId(chatId);
    if (!connectionId) return;
    this.showChat(connectionId, chatId, (c) => ({ ...c, updatedAt: new Date() }));
  }

  private showChat(connectionId: string, chatId: string, update: (c: AIChat) => AIChat): void {
    const chats = this.state.aiChatsByConnection[connectionId] ?? [];
    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: chats.map((c) => (c.id === chatId ? update(c) : c)),
    };
  }

  private async saveChat(chatId: string, patch: { title?: string; touched?: boolean }) {
    try {
      const { seq } = await this.state.librarySeqs.write([rowKey("chat", chatId)], () =>
        getLibrary().updateChat(chatId, patch),
      );
      this.state.librarySeqs.note(rowKey("chat", chatId), seq);
    } catch (error) {
      void log.error(`Failed to save AI chat ${chatId}:`, error);
      const limit = limitOf(error);
      const message = (limit && limitMessage(limit)) || libraryErrorMessage(error);
      errorToast(m.ai_chat_save_failed({ message }));
    }
  }

  /**
   * The rows Core stored for a turn (`done.messages`, or an `error`'s), at
   * their `seq` (phase 6): each replaces the message the page
   * showed under its id, keeping what the page saw live (tool rows, the
   * worded error), unless a newer version of it was applied already.
   */
  applyTurnRows(chatId: string, rows: PersistedAIMessage[], seq: ChangeSeq): void {
    const shown = this.state.aiMessagesByChat[chatId];
    if (!shown) return; // The chat was deleted.
    const seqs = this.state.librarySeqs;
    let next = shown;
    let stored = this.state.aiMessagesStored.get(chatId);
    if (!stored) this.state.aiMessagesStored.set(chatId, (stored = new Set()));
    for (const row of rows) {
      if (row.chatId !== chatId) continue;
      if (!seqs.take(rowKey("aiMessage", row.id), seq)) continue;
      stored.add(row.id);
      const local = next.find((m) => m.id === row.id);
      const message = withLiveView(messageFromWire(row), local);
      next = local ? next.map((m) => (m.id === row.id ? message : m)) : [...next, message];
    }
    if (next !== shown) {
      this.state.aiMessagesByChat = { ...this.state.aiMessagesByChat, [chatId]: next };
    }
  }

  /** The chat takes no more messages here; `tell`: say so (once). */
  markFull(chatId: string, tell: boolean): void {
    if (this.state.aiChatFull[chatId]) return;
    this.state.aiChatFull = { ...this.state.aiChatFull, [chatId]: true };
    if (tell) errorToast(m.ai_chat_full());
  }

  /** Read `chatId` again once its turn ended (a turn whose ending the transport lost). */
  refetchAfterTurnFor(chatId: string): void {
    this.refetchAfterTurn.add(chatId);
  }

  /**
   * Stop: Core stores the stopped reply after the stream ended, and its
   * `chatMessages` event carries this page's origin, so the feed would
   * skip it. Hear that chat's own events until one is applied.
   */
  awaitOwnStore(chatId: string): void {
    this.awaitingOwnStore.add(chatId);
  }

  /** Whether `chatId` waits for Core to store a stopped turn (the feed's `acceptOwn`). */
  awaitsOwnStore(chatId: string): boolean {
    return this.awaitingOwnStore.has(chatId);
  }

  /** A chat another window changed while it streamed is read again once its turn ended. */
  async afterTurn(chatId: string): Promise<void> {
    if (!this.refetchAfterTurn.has(chatId) || this.state.aiStreamingChatId === chatId) return;
    this.refetchAfterTurn.delete(chatId);
    await this.loadMessagesFromDb(chatId);
  }

  // === OTHER WINDOWS ===

  /**
   * Another window changed a connection's chats (a `chat` event): read them
   * again. A chat deleted there that this page has open stops its turn
   * first (without saving it), then another chat becomes active.
   */
  async refreshChats(
    connectionId: string,
    reload: (connectionId: string) => Promise<void>,
  ): Promise<void> {
    if (!(connectionId in this.state.aiChatsByConnection)) return;
    const before = this.state.aiChatsByConnection[connectionId] ?? [];
    const active = this.state.activeAIChatIdByConnection[connectionId];
    // Applies the list: a deleted chat leaves it, with its messages.
    await reload(connectionId);
    const remaining = this.state.aiChatsByConnection[connectionId] ?? [];
    const after = new Set(remaining.map((c) => c.id));
    for (const chat of before) {
      if (after.has(chat.id)) continue;
      // Its turn stops without saving (the chat is gone).
      this.abortStream(chat.id);
      this.state.aiMessagesStored.delete(chat.id);
      this.refetchAfterTurn.delete(chat.id);
      this.awaitingOwnStore.delete(chat.id);
      if (this.state.aiChatFull[chat.id]) {
        const { [chat.id]: _full, ...rest } = this.state.aiChatFull;
        this.state.aiChatFull = rest;
      }
      if (chat.id !== active) continue;
      toast.info(m.ai_chat_removed_elsewhere({ name: chat.title }));
      this.state.activeAIChatIdByConnection = {
        ...this.state.activeAIChatIdByConnection,
        [connectionId]: remaining[0]?.id ?? null,
      };
      if (remaining[0] && !(remaining[0].id in this.state.aiMessagesByChat)) {
        await this.loadMessagesFromDb(remaining[0].id);
      }
    }
  }

  /**
   * Another window stored messages of `chatId` (a `chatMessages` event). A
   * chat streaming here ignores it until its turn is stored, then reads
   * again; one the page holds reads again now.
   */
  async refreshMessages(chatId: string): Promise<void> {
    if (!(chatId in this.state.aiMessagesByChat)) return;
    if (this.state.aiStreamingChatId === chatId) {
      this.refetchAfterTurn.add(chatId);
      return;
    }
    await this.loadMessagesFromDb(chatId);
    this.awaitingOwnStore.delete(chatId);
  }
}
