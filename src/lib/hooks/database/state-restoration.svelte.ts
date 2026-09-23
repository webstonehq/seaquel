import type {
  Query,
  QueryHistoryItem,
  Dashboard,
  AIChat,
  QueryVersion,
  PersistedSavedQuery,
  PersistedQueryHistoryItem,
  PersistedAIChat,
  PersistedAIMessage,
  PersistedQueryVersion,
  DashboardVersion,
  PersistedDashboardVersion,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { PersistedDashboard } from "$lib/storage/repository";

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
   * Restore saved queries from persisted data (per-project).
   */
  restoreSavedQueries(projectId: string, data: PersistedSavedQuery[]): void {
    const queries: Query[] = data.map((q) => ({
      id: q.id,
      name: q.name,
      query: q.query,
      projectId: q.projectId,
      createdAt: new Date(q.createdAt),
      updatedAt: new Date(q.updatedAt),
      parameters: q.parameters,
      starred: q.starred,
      shared: q.shared ?? false,
      description: q.description,
      databaseType: q.databaseType,
      tags: q.tags,
      folder: q.folder,
    }));
    this.state.queriesByProject = {
      ...this.state.queriesByProject,
      [projectId]: queries,
    };
  }

  /**
   * Restore query versions from persisted data (per-project).
   */
  restoreQueryVersions(projectId: string, data: PersistedQueryVersion[]): void {
    const versions: QueryVersion[] = data.map((v) => ({
      id: v.id,
      queryId: v.queryId,
      version: v.version,
      snapshot: v.snapshot,
      diff: v.diff,
      createdAt: new Date(v.createdAt),
    }));
    this.state.queryVersionsByProject = {
      ...this.state.queryVersionsByProject,
      [projectId]: versions,
    };
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
   * Restore query history from persisted data.
   */
  restoreQueryHistory(connectionId: string, data: PersistedQueryHistoryItem[]): void {
    const history: QueryHistoryItem[] = data.map((h) => ({
      id: h.id,
      query: h.query,
      timestamp: new Date(h.timestamp),
      executionTime: h.executionTime,
      rowCount: h.rowCount,
      connectionId: h.connectionId,
      favorite: h.favorite,
      connectionLabelsSnapshot: h.connectionLabelsSnapshot || [],
      connectionNameSnapshot: h.connectionNameSnapshot || "",
    }));
    this.state.queryHistoryByConnection = {
      ...this.state.queryHistoryByConnection,
      [connectionId]: history,
    };
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
    const savedQueries = await this.persistence.loadProjectSavedQueries(projectId);
    if (savedQueries.length > 0) {
      this.restoreSavedQueries(projectId, savedQueries);
    }

    const queryVersions = await this.persistence.loadProjectQueryVersions(projectId);
    if (queryVersions.length > 0) {
      this.restoreQueryVersions(projectId, queryVersions);
    }

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
   * Load messages for a specific AI chat (lazy loading when switching chats).
   */
  async loadAIChatMessages(chatId: string): Promise<void> {
    const messages = await this.persistence.loadAIChatMessages(chatId);
    this.restoreAIChatMessages(chatId, messages);
  }
}
