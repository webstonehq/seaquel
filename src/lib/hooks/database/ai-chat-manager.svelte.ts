import type { AIChat } from "$lib/types";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import { toast } from "svelte-sonner";
import { log } from "$lib/utils/logger";
import type { DatabaseState } from "./state.svelte.js";
import { getLibrary, rowKey, NEW, type ChatMessageDraft } from "./library/index.js";
import { chatFromWire, messageDraft } from "./library/convert.js";
import { isChatFull, libraryErrorMessage, limitMessage, limitOf } from "./library/messages.js";

/**
 * AI chats (phase 5d-2, Decision 24): a chat is created at once through
 * Core (`chatCreate`, Core's id), its title and time are `chatUpdate`s,
 * and its messages are upserted by their (GUI-made) ids, sending only the
 * ones new or changed since the page last loaded or sent them
 * (`chatMessagesPut`). A put past the web's per-chat budget (Q17) marks
 * the chat full: the refused turn stays on screen, and sending there stops.
 */
export class AIChatManager {
  /** Chats whose `chatMessages` event came while they streamed: read again after the turn. */
  private refetchAfterTurn = new Set<string>();
  /** A create on its way per connection: a second send waits for it. */
  private creating = new Map<string, Promise<string | null>>();
  /**
   * Messages Core refused as too large (`max_message_bytes`), per chat:
   * they stay on screen and are left out of later puts.
   */
  private unsendable = new Map<string, Set<string>>();

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
    this.state.aiMessagesSent.set(chat.id, new Map());
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
    this.state.aiMessagesSent.delete(chatId);
    this.unsendable.delete(chatId);
    this.refetchAfterTurn.delete(chatId);
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

  /** Called when a reply ends, which may be after the active connection changed. */
  updateChatTimestamp(chatId: string): void {
    const connectionId = this.chatConnectionId(chatId);
    if (!connectionId) return;
    this.showChat(connectionId, chatId, (c) => ({ ...c, updatedAt: new Date() }));
    void this.saveChat(chatId, { touched: true });
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
   * Store the chat's messages that are new or changed since the page last
   * loaded or sent them (Decision 24); a put that failed leaves them unsent,
   * so the next one carries them again. A message still waiting for a model
   * isn't stored. Past the web's budget (`max_chat_bytes`) or message count
   * (`max_messages_per_chat`) the chat is marked full (said once); a
   * message over `max_message_bytes` is said once, stays on screen and is
   * left out of this and later puts.
   */
  async persistMessages(chatId: string): Promise<void> {
    const dropped = this.unsendable.get(chatId);
    const messages = (this.state.aiMessagesByChat[chatId] ?? []).filter(
      (msg) => !msg.pendingModelSelection && !dropped?.has(msg.id),
    );
    const sent = this.state.aiMessagesSent.get(chatId) ?? new Map<string, string>();
    const changed = messages
      .map((msg) => messageDraft(msg))
      .filter((d) => sent.get(d.id) !== JSON.stringify(d));
    if (changed.length === 0) return this.afterTurn(chatId);
    const seqs = this.state.librarySeqs;
    try {
      // Its answer holds only these messages, not the chat's list, so its
      // `seq` isn't recorded for the list: a read after it still applies.
      const { value } = await seqs.write([rowKey("chatMessages", chatId)], () =>
        getLibrary().putChatMessages(chatId, changed),
      );
      const now = this.state.aiMessagesSent.get(chatId) ?? new Map<string, string>();
      for (const d of changed) now.set(d.id, JSON.stringify(d));
      this.state.aiMessagesSent.set(chatId, now);
      if (value.full) this.markFull(chatId, false);
    } catch (error) {
      void log.error(`Failed to save the messages of AI chat ${chatId}:`, error);
      const limit = limitOf(error);
      if (isChatFull(error) || limit === "max_messages_per_chat") {
        this.markFull(chatId, true);
      } else if (
        (limit === "max_message_bytes" || limit === "max_query_bytes") &&
        this.dropTooLarge(chatId, changed, limit, error)
      ) {
        await this.persistMessages(chatId);
      } else if (chatId in this.state.aiMessagesByChat) {
        errorToast(m.ai_chat_save_failed({ message: libraryErrorMessage(error) }));
      }
      return;
    }
    await this.afterTurn(chatId);
  }

  /** The chat takes no more messages here; `tell`: say so (once). */
  private markFull(chatId: string, tell: boolean): void {
    if (this.state.aiChatFull[chatId]) return;
    this.state.aiChatFull = { ...this.state.aiChatFull, [chatId]: true };
    if (tell) errorToast(m.ai_chat_full());
  }

  /**
   * Leaves out of later puts the messages of `changed` whose content
   * (`max_message_bytes`) or query (`max_query_bytes`) is longer than the
   * limit Core named (its message says the bytes), each said once. True
   * when any was left out (so the rest can go now); false when the refusal
   * can't be read or names none of them (the caller says it as an error).
   */
  private dropTooLarge(
    chatId: string,
    changed: ChatMessageDraft[],
    limit: "max_message_bytes" | "max_query_bytes",
    error: unknown,
  ): boolean {
    const message = error instanceof Error ? error.message : String(error);
    const max = Number(new RegExp(`${limit}: (\\d+)`).exec(message)?.[1]);
    if (!Number.isFinite(max)) return false;
    const encoder = new TextEncoder();
    const field = (d: ChatMessageDraft) =>
      limit === "max_query_bytes" ? (d.query ?? "") : d.content;
    const over = changed.filter((d) => encoder.encode(field(d)).length > max);
    if (over.length === 0) return false;
    let set = this.unsendable.get(chatId);
    if (!set) this.unsendable.set(chatId, (set = new Set()));
    for (const d of over) {
      set.add(d.id);
      errorToast(m.ai_message_too_large({ limit }));
    }
    return true;
  }

  /** A chat another window changed while it streamed is read again once its turn is stored. */
  private async afterTurn(chatId: string): Promise<void> {
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
      this.state.aiMessagesSent.delete(chat.id);
      this.unsendable.delete(chat.id);
      this.refetchAfterTurn.delete(chat.id);
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
  }
}
