import type { AIChat } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

export class AIChatManager {
  constructor(
    private state: DatabaseState,
    private scheduleAIChatPersistence: (connectionId: string) => void,
    private loadMessagesFromDb: (chatId: string) => Promise<void>,
    private persistMessages: (chatId: string) => Promise<void>,
    private removeChatFromDb: (chatId: string) => Promise<void>,
  ) {}

  createChat(title?: string): string | null {
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) return null;

    const chat: AIChat = {
      id: crypto.randomUUID(),
      connectionId,
      title: title || "New Chat",
      createdAt: new Date(),
      updatedAt: new Date(),
    };

    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: [chat, ...(this.state.aiChatsByConnection[connectionId] ?? [])],
    };
    this.state.aiMessagesByChat = {
      ...this.state.aiMessagesByChat,
      [chat.id]: [],
    };
    this.state.activeAIChatIdByConnection = {
      ...this.state.activeAIChatIdByConnection,
      [connectionId]: chat.id,
    };

    this.scheduleAIChatPersistence(connectionId);
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

  async deleteChat(chatId: string): Promise<void> {
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) return;

    const chats = this.state.aiChatsByConnection[connectionId] ?? [];
    const remaining = chats.filter((c) => c.id !== chatId);

    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: remaining,
    };

    const { [chatId]: _, ...restMessages } = this.state.aiMessagesByChat;
    this.state.aiMessagesByChat = restMessages;

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

    await this.removeChatFromDb(chatId);
  }

  /** The connection a chat belongs to, whatever is active now. */
  private chatConnectionId(chatId: string): string | undefined {
    return Object.values(this.state.aiChatsByConnection)
      .flat()
      .find((c) => c.id === chatId)?.connectionId;
  }

  ensureActiveChat(): string | null {
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) return null;

    const activeChatId = this.state.activeAIChatIdByConnection[connectionId] ?? null;
    if (activeChatId) return activeChatId;

    return this.createChat();
  }

  updateChatTitle(chatId: string, firstMessage: string): void {
    const connectionId = this.chatConnectionId(chatId);
    if (!connectionId) return;

    const title = firstMessage.slice(0, 50) + (firstMessage.length > 50 ? "..." : "");
    const chats = this.state.aiChatsByConnection[connectionId] ?? [];
    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: chats.map((c) =>
        c.id === chatId ? { ...c, title, updatedAt: new Date() } : c,
      ),
    };

    this.scheduleAIChatPersistence(connectionId);
  }

  /** Called when a reply ends, which may be after the active connection changed. */
  updateChatTimestamp(chatId: string): void {
    const connectionId = this.chatConnectionId(chatId);
    if (!connectionId) return;

    const chats = this.state.aiChatsByConnection[connectionId] ?? [];
    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: chats.map((c) => (c.id === chatId ? { ...c, updatedAt: new Date() } : c)),
    };

    this.scheduleAIChatPersistence(connectionId);
  }
}
