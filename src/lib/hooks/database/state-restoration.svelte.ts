import type { PersistedQueryHistoryItem } from "$lib/types";
import { log } from "$lib/utils/logger";
import {
  getLibrary,
  rowKey,
  type ChangeSeq,
  type ChatMessages,
  type WireChat,
} from "./library/index.js";
import {
  chatFromWire,
  dashboardFromWire,
  dashboardVersionFromWire,
  messageFromWire,
  queryVersionFromWire,
  savedQueryFromWire,
} from "./library/convert.js";
import type { DatabaseState } from "./state.svelte.js";
import { withLiveView } from "./ai/events.js";
import { fromPersisted, trimHistory } from "./query-history.svelte.js";
import { getStorage } from "$lib/storage";

/**
 * Manages restoration of persisted connection data when loading the app.
 * Handles hydration of query history (per-connection), AI chats (per-connection),
 * and saved queries / dashboards (per-project).
 *
 * Note: Tab restoration is handled by ProjectManager since tabs are per-project.
 */
export class StateRestorationManager {
  constructor(private state: DatabaseState) {}

  /**
   * A connection's stored history. History is written by targeted calls in
   * `QueryHistoryManager` (append, set favourite), never by replacing the
   * list, so a failed load has no save to block: it reads as empty and
   * appends still reach the file.
   */
  private async loadHistory(connectionId: string): Promise<PersistedQueryHistoryItem[]> {
    try {
      return await getStorage().queryHistory.loadByConnection(connectionId);
    } catch (error) {
      void log.error(`Failed to load data for connection ${connectionId}:`, error);
      return [];
    }
  }

  /**
   * Initialize connection data maps for a new connection.
   * Sets up query history, schema storage, and AI chat storage.
   */
  initializeConnectionMaps(connectionId: string): void {
    // Query history remains per-connection
    this.state.queryHistoryByConnection = {
      ...this.state.queryHistoryByConnection,
      [connectionId]: [],
    };
    // Schema storage is per-connection
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: [],
    };
    // AI chats per-connection
    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: this.state.aiChatsByConnection[connectionId] ?? [],
    };
  }

  /**
   * Clean up connection data when removing a connection.
   */
  cleanupConnectionMaps(connectionId: string): void {
    const { [connectionId]: _1, ...restQueryHistory } = this.state.queryHistoryByConnection;
    this.state.queryHistoryByConnection = restQueryHistory;

    const { [connectionId]: _3, ...restSchemas } = this.state.schemas;
    this.state.schemas = restSchemas;

    // Clean up AI chat state
    const chats = this.state.aiChatsByConnection[connectionId] ?? [];
    const { [connectionId]: _4, ...restAIChats } = this.state.aiChatsByConnection;
    this.state.aiChatsByConnection = restAIChats;

    const { [connectionId]: _5, ...restActiveChat } = this.state.activeAIChatIdByConnection;
    this.state.activeAIChatIdByConnection = restActiveChat;

    const newMessages = { ...this.state.aiMessagesByChat };
    for (const chat of chats) {
      delete newMessages[chat.id];
    }
    this.state.aiMessagesByChat = newMessages;
  }

  /**
   * Ensure connection data maps exist (used during reconnect).
   */
  ensureConnectionMapsExist(connectionId: string): void {
    if (!(connectionId in this.state.queryHistoryByConnection)) {
      this.state.queryHistoryByConnection = {
        ...this.state.queryHistoryByConnection,
        [connectionId]: [],
      };
    }
  }

  /**
   * Restore query history from persisted data. Rows already cached that the
   * load didn't return (a query run while it was reading) stay on top, and
   * the list is trimmed like the file.
   */
  restoreQueryHistory(connectionId: string, data: PersistedQueryHistoryItem[]): void {
    const loaded = data.map(fromPersisted);
    const ids = new Set(loaded.map((h) => h.id));
    const newer = (this.state.queryHistoryByConnection[connectionId] ?? []).filter(
      (h) => !ids.has(h.id),
    );
    this.state.queryHistoryByConnection = {
      ...this.state.queryHistoryByConnection,
      [connectionId]: trimHistory([...newer, ...loaded]),
    };
  }

  /**
   * Read a connection's history again after another window changed it
   * (phase 5d-1). `replace`: the stored list replaces the page's (its
   * history was cleared elsewhere); otherwise it's merged as a load is.
   * Rows the page doesn't hold for a connection it hasn't loaded are left
   * alone.
   */
  async reloadHistory(connectionId: string, replace: boolean): Promise<void> {
    if (!(connectionId in this.state.queryHistoryByConnection)) return;
    const queryHistory = await this.loadHistory(connectionId);
    if (replace) {
      this.state.queryHistoryByConnection = {
        ...this.state.queryHistoryByConnection,
        [connectionId]: trimHistory(queryHistory.map(fromPersisted)),
      };
    } else {
      this.restoreQueryHistory(connectionId, queryHistory);
    }
  }

  /**
   * Show a connection's stored chats (newest first) by the `seq` rule per
   * chat. The page's own chats the list doesn't hold yet (one being
   * created) stay. The first time, the most recent becomes active.
   */
  restoreAIChats(connectionId: string, data: WireChat[], seq: ChangeSeq): void {
    const seqs = this.state.librarySeqs;
    const shown = this.state.aiChatsByConnection[connectionId] ?? [];
    const byId = new Map(data.map((c) => [c.id, c]));
    let next = [...shown];
    const removed: string[] = [];
    for (const id of new Set([...shown.map((c) => c.id), ...byId.keys()])) {
      if (seqs.busy(rowKey("chat", id))) continue;
      if (!seqs.take(rowKey("chat", id), seq)) continue;
      const row = byId.get(id);
      if (row) {
        const chat = chatFromWire(row);
        next = next.some((c) => c.id === id)
          ? next.map((c) => (c.id === id ? chat : c))
          : [...next, chat];
      } else {
        next = next.filter((c) => c.id !== id);
        removed.push(id);
      }
    }
    next.sort((a, b) => b.updatedAt.getTime() - a.updatedAt.getTime());
    this.state.aiChatsByConnection = { ...this.state.aiChatsByConnection, [connectionId]: next };
    if (removed.length > 0) {
      const messages = { ...this.state.aiMessagesByChat };
      for (const id of removed) delete messages[id];
      this.state.aiMessagesByChat = messages;
    }
    if (!(connectionId in this.state.activeAIChatIdByConnection) && next.length > 0) {
      this.state.activeAIChatIdByConnection = {
        ...this.state.activeAIChatIdByConnection,
        [connectionId]: next[0].id,
      };
    }
  }

  /**
   * Show a chat's stored messages, and whether Core says the
   * chat is full (it opens with sending off; a flag a refusal set is never
   * cleared here). Messages this page shows that Core hasn't stored (a
   * turn in flight, a message waiting for a model, a turn Core refused)
   * stay, merged by time. A row a turn's `done` applied at a newer `seq`
   * than this list keeps that version (phase 6: Core stores the turn).
   */
  restoreAIChatMessages(chatId: string, data: ChatMessages, seq: ChangeSeq): void {
    const seqs = this.state.librarySeqs;
    if (!seqs.take(rowKey("chatMessages", chatId), seq)) return;
    const stored = this.state.aiMessagesStored.get(chatId);
    const shown = this.state.aiMessagesByChat[chatId] ?? [];
    const local = new Map(shown.map((msg) => [msg.id, msg]));
    const listed = new Set(data.messages.map((msg) => msg.id));
    /** Applied from a turn's ending at a `seq` this list predates. */
    const newer = (id: string) => (seqs.last(rowKey("aiMessage", id)) ?? -1) > seq.n;
    // The stored rows in their stored order (time, then insertion).
    const merged = data.messages.map((wire) => {
      const shownNow = local.get(wire.id);
      if (shownNow && newer(wire.id)) return shownNow;
      return withLiveView(messageFromWire(wire), shownNow);
    });
    const order = new Map(merged.map((msg, i) => [msg.id, i]));
    merged.sort(
      (a, b) =>
        a.timestamp.getTime() - b.timestamp.getTime() || order.get(a.id)! - order.get(b.id)!,
    );
    // Rows only this page shows keep their place: right after the row
    // shown before them (Core's clock and the page's needn't agree, so a
    // turn half stored doesn't put its reply above its question).
    const nowStored = new Set(listed);
    let after: string | null = null;
    for (const msg of shown) {
      if (listed.has(msg.id)) {
        after = msg.id;
        continue;
      }
      if (stored?.has(msg.id) && !newer(msg.id)) continue; // Deleted since.
      const at: number = after === null ? 0 : merged.findIndex((m) => m.id === after) + 1;
      merged.splice(at === 0 && after !== null ? merged.length : at, 0, msg);
      after = msg.id;
      if (stored?.has(msg.id)) nowStored.add(msg.id);
    }
    this.state.aiMessagesByChat = { ...this.state.aiMessagesByChat, [chatId]: merged };
    this.state.aiMessagesStored.set(chatId, nowStored);
    if (data.full && !this.state.aiChatFull[chatId]) {
      this.state.aiChatFull = { ...this.state.aiChatFull, [chatId]: true };
    }
  }

  /**
   * Load connection data (query history and AI chats) from persistence.
   */
  async loadConnectionData(connectionId: string): Promise<void> {
    const queryHistory = await this.loadHistory(connectionId);
    if (queryHistory.length > 0) {
      this.restoreQueryHistory(connectionId, queryHistory);
    }
    await this.loadAIChats(connectionId);
  }

  /**
   * A connection's chats, and the messages of its active one. A failed
   * read leaves what the page shows (nothing is replaced whole any more,
   * so there is no save to block).
   */
  async loadAIChats(connectionId: string): Promise<void> {
    try {
      const { value, seq } = await getLibrary().listChats(connectionId);
      this.restoreAIChats(connectionId, value, seq);
    } catch (error) {
      void log.error(`Failed to load AI chats for connection ${connectionId}:`, error);
      return;
    }
    const active = this.state.activeAIChatIdByConnection[connectionId];
    if (active && !(active in this.state.aiMessagesByChat)) {
      await this.loadAIChatMessages(active);
    }
  }

  /**
   * Load a project's saved queries and dashboards (with their versions)
   * from the library, by the `seq` rule. An empty list applies too (bug
   * 18: a project whose dashboards were all deleted elsewhere shows none).
   */
  async loadProjectData(projectId: string): Promise<void> {
    await Promise.all([this.loadSavedQueries(projectId), this.loadDashboards(projectId)]);
  }

  /** A project's dashboards and their versions; a failed read leaves what's shown. */
  async loadDashboards(projectId: string): Promise<void> {
    const library = getLibrary();
    try {
      const [dashboards, versions] = await Promise.all([
        library.listDashboards(projectId),
        library.listDashboardVersions(projectId),
      ]);
      const seqs = this.state.librarySeqs;
      const shown = this.state.dashboardsByProject[projectId] ?? [];
      const byId = new Map(dashboards.value.map((d) => [d.id, d]));
      let next = [...shown];
      for (const id of new Set([...shown.map((d) => d.id), ...byId.keys()])) {
        if (!seqs.take(rowKey("dashboard", id), dashboards.seq)) continue;
        const row = byId.get(id);
        const current = next.find((d) => d.id === id);
        next = row
          ? current
            ? next.map((d) => (d.id === id ? dashboardFromWire(row, d) : d))
            : [...next, dashboardFromWire(row)]
          : next.filter((d) => d.id !== id);
      }
      this.state.dashboardsByProject = { ...this.state.dashboardsByProject, [projectId]: next };
      if (seqs.take(rowKey("dashboardVersion", projectId), versions.seq)) {
        this.state.dashboardVersionsByProject = {
          ...this.state.dashboardVersionsByProject,
          [projectId]: versions.value.map(dashboardVersionFromWire),
        };
      }
    } catch (error) {
      void log.error(`Failed to load dashboards for project ${projectId}:`, error);
      this.state.dashboardsByProject[projectId] ??= [];
    }
  }

  /**
   * Load a project's saved queries and their versions from the library, and
   * apply them by the `seq` rule. A failed read leaves the
   * page's copy as it is: nothing replaces the stored list any more, so a
   * failed load can't lose queries (the next activation reads them again).
   */
  async loadSavedQueries(projectId: string): Promise<void> {
    const library = getLibrary();
    try {
      const [queries, versions] = await Promise.all([
        library.listSavedQueries(projectId),
        library.listQueryVersions(projectId),
      ]);
      const seqs = this.state.librarySeqs;
      const shown = this.state.queriesByProject[projectId] ?? [];
      const byId = new Map(queries.value.map((q) => [q.id, q]));
      // Every row the list or the page has, each only if the list is newer.
      let next = [...shown];
      for (const id of new Set([...shown.map((q) => q.id), ...byId.keys()])) {
        if (!seqs.take(rowKey("savedQuery", id), queries.seq)) continue;
        const row = byId.get(id);
        next = row
          ? next.some((q) => q.id === id)
            ? next.map((q) => (q.id === id ? savedQueryFromWire(row) : q))
            : [...next, savedQueryFromWire(row)]
          : next.filter((q) => q.id !== id);
      }
      this.state.queriesByProject = { ...this.state.queriesByProject, [projectId]: next };
      if (seqs.take(rowKey("queryVersion", projectId), versions.seq)) {
        this.state.queryVersionsByProject = {
          ...this.state.queryVersionsByProject,
          [projectId]: versions.value.map(queryVersionFromWire),
        };
      }
    } catch (error) {
      void log.error(`Failed to load saved queries for project ${projectId}:`, error);
    }
  }

  /**
   * Load messages for a specific AI chat (lazy loading when switching
   * chats, and another window's change). A failed load leaves the chat out
   * of `aiMessagesByChat`, so the next switch loads it again instead of
   * showing it empty.
   */
  async loadAIChatMessages(chatId: string): Promise<void> {
    try {
      // After this page's own puts of the chat: the read then holds them.
      await this.state.librarySeqs.settled(rowKey("chatMessages", chatId));
      const { value, seq } = await getLibrary().listChatMessages(chatId);
      this.restoreAIChatMessages(chatId, value, seq);
    } catch (error) {
      void log.error(`Failed to load AI chat messages for chat ${chatId}:`, error);
    }
  }
}
