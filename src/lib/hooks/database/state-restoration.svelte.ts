import type {
  Dashboard,
  AIChat,
  PersistedQueryHistoryItem,
  PersistedAIChat,
  PersistedAIMessage,
  DashboardVersion,
  PersistedDashboardVersion,
} from "$lib/types";
import { log } from "$lib/utils/logger";
import { getLibrary, rowKey } from "./library/index.js";
import { queryVersionFromWire, savedQueryFromWire } from "./library/convert.js";
import type { DatabaseState } from "./state.svelte.js";
import { fromPersisted, trimHistory } from "./query-history.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { PersistedDashboard } from "$lib/storage";

/**
 * Manages restoration of persisted connection data when loading the app.
 * Handles hydration of query history (per-connection), AI chats (per-connection),
 * and saved queries / dashboards (per-project).
 *
 * Note: Tab restoration is handled by ProjectManager since tabs are per-project.
 */
export class StateRestorationManager {
  constructor(
    private state: DatabaseState,
    private persistence: PersistenceManager,
  ) {}

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
   * Restore dashboard versions from persisted data (per-project).
   */
  restoreDashboardVersions(projectId: string, data: PersistedDashboardVersion[]): void {
    const versions: DashboardVersion[] = data.map((v) => ({
      id: v.id,
      dashboardId: v.dashboardId,
      version: v.version,
      snapshot: v.snapshot,
      createdAt: new Date(v.createdAt),
    }));
    this.state.dashboardVersionsByProject = {
      ...this.state.dashboardVersionsByProject,
      [projectId]: versions,
    };
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
    const { queryHistory } = await this.persistence.loadConnectionData(connectionId);
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
   * Restore AI chats from persisted data.
   */
  restoreAIChats(connectionId: string, data: PersistedAIChat[]): void {
    const chats: AIChat[] = data.map((c) => ({
      id: c.id,
      connectionId: c.connectionId,
      title: c.title,
      createdAt: new Date(c.createdAt),
      updatedAt: new Date(c.updatedAt),
    }));
    this.state.aiChatsByConnection = {
      ...this.state.aiChatsByConnection,
      [connectionId]: chats,
    };
    // Set most recent chat as active
    if (chats.length > 0) {
      this.state.activeAIChatIdByConnection = {
        ...this.state.activeAIChatIdByConnection,
        [connectionId]: chats[0].id,
      };
    }
  }

  /**
   * Restore AI chat messages from persisted data.
   */
  restoreAIChatMessages(chatId: string, data: PersistedAIMessage[]): void {
    this.state.aiMessagesByChat = {
      ...this.state.aiMessagesByChat,
      [chatId]: data.map((m) => ({
        id: m.id,
        chatId: m.chatId,
        role: m.role,
        content: m.content,
        timestamp: new Date(m.timestamp),
        query: m.query,
        dashboardId: m.dashboardId,
      })),
    };
  }

  /**
   * Restore dashboards from persisted data (per-project).
   */
  restoreDashboards(projectId: string, data: PersistedDashboard[]): void {
    const dashboards: Dashboard[] = data.map((r) => ({
      id: r.id,
      name: r.name,
      projectId: r.projectId,
      widgets: JSON.parse(r.widgets),
      viewport: JSON.parse(r.viewport),
      dateFilter: r.dateFilter ? JSON.parse(r.dateFilter) : null,
      shared: r.shared ?? false,
      starred: r.starred ?? false,
      description: r.description,
      createdAt: new Date(r.createdAt),
      updatedAt: new Date(r.updatedAt),
    }));
    this.state.dashboardsByProject = {
      ...this.state.dashboardsByProject,
      [projectId]: dashboards,
    };
  }

  /**
   * Load connection data (query history and AI chats) from persistence.
   */
  async loadConnectionData(connectionId: string): Promise<void> {
    const data = await this.persistence.loadConnectionData(connectionId);

    if (data.queryHistory.length > 0) {
      this.restoreQueryHistory(connectionId, data.queryHistory);
    }

    // Load AI chats
    const aiChats = await this.persistence.loadAIChats(connectionId);
    if (aiChats.length > 0) {
      this.restoreAIChats(connectionId, aiChats);
      // Load messages for the most recent (active) chat only
      const activeChatId = aiChats[0].id;
      const messages = await this.persistence.loadAIChatMessages(activeChatId);
      if (messages.length > 0) {
        this.restoreAIChatMessages(activeChatId, messages);
      }
    }
  }

  /**
   * Load project-specific data (saved queries and dashboards) from persistence.
   */
  async loadProjectData(projectId: string): Promise<void> {
    await this.loadSavedQueries(projectId);

    const dashboards = await this.persistence.loadProjectDashboards(projectId);
    if (dashboards.length > 0) {
      this.restoreDashboards(projectId, dashboards);
    }

    const dashboardVersions = await this.persistence.loadProjectDashboardVersions(projectId);
    if (dashboardVersions.length > 0) {
      this.restoreDashboardVersions(projectId, dashboardVersions);
    }
  }

  /**
   * Load a project's saved queries and their versions from the library, and
   * apply them by the `seq` rule (Decision 17). A failed read leaves the
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
   * Load messages for a specific AI chat (lazy loading when switching chats).
   */
  async loadAIChatMessages(chatId: string): Promise<void> {
    const messages = await this.persistence.loadAIChatMessages(chatId);
    // After a failed load, leave the chat out of `aiMessagesByChat` so the
    // next `switchChat` loads it again instead of showing it empty.
    if (this.persistence.loadFailed(`aiMessages:${chatId}`)) return;
    this.restoreAIChatMessages(chatId, messages);
  }
}
