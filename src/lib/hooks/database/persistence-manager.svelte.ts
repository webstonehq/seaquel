import { withErrorHandling } from "$lib/errors";
import type {
  PersistedQueryTab,
  PersistedSchemaTab,
  PersistedExplainTab,
  PersistedErdTab,
  PersistedStatisticsTab,
  PersistedWorkflowTab,
  PersistedStarterTab,
  PersistedDashboardTab,
  PersistedSavedQuery,
  PersistedQueryHistoryItem,
  PersistedAIChat,
  PersistedAIMessage,
  PersistedQueryVersion,
  PersistedDashboardVersion,
  DatabaseConnection,
  PersistedProject,
  PersistedProjectState,
  PersistedSharedQueryRepo,
} from "$lib/types";
import { serializeRepo } from "$lib/types";
import type { SavedWorkflow } from "$lib/types/workflow";
import { toPersistedDashboard } from "./dashboard-serialize.js";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistedConnection } from "./types.js";
import type { ConnectionOverride } from "$lib/types";
import { getStorage, type PersistedDashboard } from "$lib/storage";
import { getKeyringService } from "$lib/services/keyring";
import { log } from "$lib/utils/logger";
import { skipUnloadedSave } from "$lib/storage/load-guard";

/**
 * A stored collection whose save replaces what's stored, keyed so a failed
 * load can be recorded (see `load-guard.ts`).
 */
export type LoadKey =
  | "projects"
  | "sharedRepos"
  | `projectState:${string}`
  | `savedQueries:${string}`
  | `history:${string}`
  | `aiMessages:${string}`;

/**
 * Manages persistence of projects, connections, and their state to SQLite.
 * Handles serialization, debounced saving, and state loading.
 *
 * Storage goes through `getStorage()`.
 */
export class PersistenceManager {
  // Keyed per project/connection: a single shared timer meant scheduling a save
  // for one project cancelled another project's pending write, losing it.
  private projectTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private connectionDataTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private sharedReposTimer: ReturnType<typeof setTimeout> | null = null;
  private aiChatTimers = new Map<string, ReturnType<typeof setTimeout>>();
  readonly PERSISTENCE_DEBOUNCE_MS = 500;
  readonly MAX_HISTORY_ITEMS = 500;

  /**
   * Collections whose last load failed or is still running. Their in-memory copy is empty or
   * default, not what's stored, so their replacing saves are refused until
   * a later load succeeds. A collection that was never loaded (a new
   * project, say) isn't in here and saves normally.
   */
  private failedLoads = new Set<LoadKey>();
  /** The subset of `failedLoads` whose read is still running. */
  private pendingLoads = new Set<LoadKey>();

  constructor(private state: DatabaseState) {}

  /** True if the last load of `key` failed, so saving it would overwrite it. */
  loadFailed(key: LoadKey): boolean {
    return this.failedLoads.has(key);
  }

  private recordLoad(key: LoadKey, ok: boolean): void {
    if (ok) this.failedLoads.delete(key);
    else this.failedLoads.add(key);
  }

  /** Refuses a save of `what` because one of `keys` isn't loaded. */
  private refuse(what: string, ...keys: LoadKey[]): void {
    const failed = keys.filter((k) => this.failedLoads.has(k));
    skipUnloadedSave(what, { pending: failed.every((k) => this.pendingLoads.has(k)) });
  }

  /**
   * Runs a load; records whether it worked, and returns `fallback` if not.
   * The key counts as unloaded while the read is pending too: a save then
   * would write the empty in-memory copy over what's being read.
   */
  private async load<T>(
    key: LoadKey,
    what: string,
    read: () => Promise<T>,
    fallback: T,
  ): Promise<T> {
    this.recordLoad(key, false);
    this.pendingLoads.add(key);
    try {
      const value = await read();
      this.recordLoad(key, true);
      return value;
    } catch (error) {
      this.recordLoad(key, false);
      void log.error(`Failed to load ${what}:`, error);
      return fallback;
    } finally {
      this.pendingLoads.delete(key);
    }
  }

  /**
   * Cancel any pending debounced persistence timer.
   */
  cancelPendingPersistence(): void {
    for (const timer of this.projectTimers.values()) clearTimeout(timer);
    this.projectTimers.clear();
    for (const timer of this.connectionDataTimers.values()) clearTimeout(timer);
    this.connectionDataTimers.clear();
  }

  /**
   * Cancel pending writes for one project (and optionally some of its
   * connections), leaving other projects' pending writes alone. Used when
   * deleting a project, where a queued write would recreate its rows.
   */
  cancelPendingPersistenceFor(projectId: string, connectionIds: string[] = []): void {
    const projectTimer = this.projectTimers.get(projectId);
    if (projectTimer) {
      clearTimeout(projectTimer);
      this.projectTimers.delete(projectId);
    }
    for (const connectionId of connectionIds) {
      const timer = this.connectionDataTimers.get(connectionId);
      if (timer) {
        clearTimeout(timer);
        this.connectionDataTimers.delete(connectionId);
      }
    }
  }

  /**
   * Schedule persistence with debouncing to avoid excessive I/O.
   */
  scheduleProject(projectId: string | null): void {
    if (!projectId) return;

    const existing = this.projectTimers.get(projectId);
    if (existing) clearTimeout(existing);
    this.projectTimers.set(
      projectId,
      setTimeout(() => {
        this.projectTimers.delete(projectId);
        void this.persistProjectState(projectId);
      }, this.PERSISTENCE_DEBOUNCE_MS),
    );
  }

  /**
   * Schedule connection data persistence (history, saved queries).
   */
  scheduleConnectionData(connectionId: string | null): void {
    if (!connectionId) return;

    const existing = this.connectionDataTimers.get(connectionId);
    if (existing) clearTimeout(existing);
    this.connectionDataTimers.set(
      connectionId,
      setTimeout(() => {
        this.connectionDataTimers.delete(connectionId);
        void this.persistConnectionData(connectionId);
      }, this.PERSISTENCE_DEBOUNCE_MS),
    );
  }

  /**
   * Schedule shared repos persistence.
   */
  scheduleSharedRepos(): void {
    if (this.sharedReposTimer) {
      clearTimeout(this.sharedReposTimer);
    }
    this.sharedReposTimer = setTimeout(() => {
      void this.persistSharedRepos();
      this.sharedReposTimer = null;
    }, this.PERSISTENCE_DEBOUNCE_MS);
  }

  /**
   * Immediately flush any pending persistence operations.
   */
  async flush(): Promise<void> {
    for (const timer of this.projectTimers.values()) clearTimeout(timer);
    this.projectTimers.clear();
    for (const timer of this.connectionDataTimers.values()) clearTimeout(timer);
    this.connectionDataTimers.clear();
    if (this.sharedReposTimer) {
      clearTimeout(this.sharedReposTimer);
      this.sharedReposTimer = null;
    }
    for (const timer of this.aiChatTimers.values()) {
      clearTimeout(timer);
    }
    this.aiChatTimers.clear();
    // Persist all projects that have data
    for (const projectId of Object.keys(this.state.queryTabsByProject)) {
      await this.persistProjectState(projectId);
    }
    // Persist all connection data
    for (const connectionId of Object.keys(this.state.queryHistoryByConnection)) {
      await this.persistConnectionData(connectionId);
    }
    // Persist shared repos
    if (this.state.sharedRepos.length > 0) {
      await this.persistSharedRepos();
    }
    // Persist AI chats
    for (const connectionId of Object.keys(this.state.aiChatsByConnection)) {
      await this.persistAIChats(connectionId);
    }
  }

  /**
   * Clean up resources. Should be called when component unmounts.
   */
  async cleanup(): Promise<void> {
    await this.flush();
  }

  // === SERIALIZATION METHODS ===

  serializeQueryTabs(projectId: string): PersistedQueryTab[] {
    const tabs = this.state.queryTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      query: tab.query,
      queryId: tab.queryId,
    }));
  }

  serializeSchemaTabs(projectId: string): PersistedSchemaTab[] {
    const tabs = this.state.schemaTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      tableName: tab.table.name,
      schemaName: tab.table.schema,
      connectionId: tab.connectionId,
    }));
  }

  serializeExplainTabs(projectId: string): PersistedExplainTab[] {
    const tabs = this.state.explainTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      sourceQuery: tab.sourceQuery,
    }));
  }

  serializeErdTabs(projectId: string): PersistedErdTab[] {
    const tabs = this.state.erdTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializeStatisticsTabs(projectId: string): PersistedStatisticsTab[] {
    const tabs = this.state.statisticsTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializeWorkflowTabs(projectId: string): PersistedWorkflowTab[] {
    const tabs = this.state.workflowTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializeStarterTabs(projectId: string): PersistedStarterTab[] {
    const tabs = this.state.starterTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      type: tab.type,
      name: tab.name,
      closable: tab.closable,
    }));
  }

  serializeDashboardTabs(projectId: string): PersistedDashboardTab[] {
    const tabs = this.state.dashboardTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      dashboardId: tab.dashboardId,
    }));
  }

  serializeCreateTableTabs(projectId: string): import("$lib/types").PersistedCreateTableTab[] {
    const tabs = this.state.createTableTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      connectionId: tab.connectionId,
      name: tab.name,
      tableDefinition: JSON.stringify(tab.tableDefinition),
    }));
  }

  serializeDataTabs(projectId: string): import("$lib/types").PersistedDataTab[] {
    const tabs = this.state.dataTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      connectionId: tab.connectionId,
      tableName: tab.tableName,
      schemaName: tab.schemaName,
    }));
  }

  serializeExtensionsDuckdbTabs(
    projectId: string,
  ): { id: string; name: string; connectionId: string }[] {
    const tabs = this.state.extensionsDuckdbTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializePaneLayout(projectId: string): PersistedProjectState["paneLayout"] | undefined {
    const layout = this.state.paneLayoutByProject[projectId];
    if (!layout || layout.panes.length <= 1) return undefined;
    return {
      panes: layout.panes.map((p) => ({
        id: p.id,
        tabIds: p.tabIds,
        activeTabId: p.activeTabId,
      })),
      activePaneId: layout.activePaneId,
    };
  }

  serializeSavedWorkflows(projectId: string): SavedWorkflow[] {
    return this.state.savedWorkflowsByProject[projectId] ?? [];
  }

  serializeSavedQueries(projectId: string): PersistedSavedQuery[] {
    const queries = this.state.queriesByProject[projectId] ?? [];
    return queries.map((q) => ({
      id: q.id,
      name: q.name,
      query: q.query,
      projectId: q.projectId,
      createdAt: q.createdAt.toISOString(),
      updatedAt: q.updatedAt.toISOString(),
      parameters: q.parameters,
      starred: q.starred,
      shared: q.shared,
      description: q.description,
      databaseType: q.databaseType,
      tags: q.tags,
      folder: q.folder,
    }));
  }

  serializeQueryHistory(connectionId: string): PersistedQueryHistoryItem[] {
    const history = this.state.queryHistoryByConnection[connectionId] ?? [];
    // Favorites are user-curated, so they survive the cap; only unfavorited
    // entries past MAX_HISTORY_ITEMS are dropped.
    const kept = history.slice(0, this.MAX_HISTORY_ITEMS);
    const keptIds = new Set(kept.map((h) => h.id));
    const favoritesBeyondCap = history
      .slice(this.MAX_HISTORY_ITEMS)
      .filter((h) => h.favorite && !keptIds.has(h.id));

    return [...kept, ...favoritesBeyondCap].map((h) => ({
      id: h.id,
      query: h.query,
      timestamp: h.timestamp.toISOString(),
      executionTime: h.executionTime,
      rowCount: h.rowCount,
      connectionId: h.connectionId,
      favorite: h.favorite,
      connectionLabelsSnapshot: h.connectionLabelsSnapshot,
      connectionNameSnapshot: h.connectionNameSnapshot,
    }));
  }

  // === PROJECT PERSISTENCE ===

  async persistProjects(): Promise<void> {
    if (this.loadFailed("projects")) {
      this.refuse("projects", "projects");
      return;
    }
    try {
      const projects: PersistedProject[] = this.state.projects.map((p) => ({
        id: p.id,
        name: p.name,
        description: p.description,
        createdAt: p.createdAt.toISOString(),
        updatedAt: p.updatedAt.toISOString(),
        customLabels: p.customLabels,
        gitRepoPath: p.gitRepoPath,
      }));

      await getStorage().projects.saveAll(projects);
    } catch (error) {
      void log.error("Failed to persist projects:", error);
    }
  }

  /** The stored projects, or `[]` with `loadFailed("projects")` set if the read failed. */
  async loadProjects(): Promise<PersistedProject[]> {
    return this.load("projects", "projects", () => getStorage().projects.loadAll(), []);
  }

  // === APP STATE PERSISTENCE ===

  async persistAppState(): Promise<void> {
    // With no stored projects loaded, the active project is an in-memory
    // stand-in whose id must not replace the stored choice.
    if (this.loadFailed("projects")) {
      this.refuse("the active project", "projects");
      return;
    }
    try {
      await getStorage().appState.set("lastActiveProjectId", this.state.activeProjectId);
    } catch (error) {
      void log.error("Failed to persist app state:", error);
    }
  }

  async getLastActiveProjectId(): Promise<string | null> {
    try {
      return await getStorage().appState.get("lastActiveProjectId");
    } catch (error) {
      void log.error("Failed to load last active project:", error);
      return null;
    }
  }

  // === PROJECT STATE PERSISTENCE (tabs, active IDs) ===

  async persistProjectState(projectId: string): Promise<void> {
    void log.debug(`Persisting project state: ${projectId}`);
    // Saving replaces the project's tabs and saved canvases.
    if (this.loadFailed("projects") || this.loadFailed(`projectState:${projectId}`)) {
      this.refuse(`the state of project ${projectId}`, "projects", `projectState:${projectId}`);
      return;
    }
    try {
      const state: PersistedProjectState = {
        projectId,
        queryTabs: this.serializeQueryTabs(projectId),
        schemaTabs: this.serializeSchemaTabs(projectId),
        explainTabs: this.serializeExplainTabs(projectId),
        erdTabs: this.serializeErdTabs(projectId),
        statisticsTabs: this.serializeStatisticsTabs(projectId),
        workflowTabs: this.serializeWorkflowTabs(projectId),
        tabOrder: this.state.tabOrderByProject[projectId] ?? [],
        connectionOrder: this.state.connectionOrderByProject[projectId] ?? [],
        activeQueryTabId: this.state.activeQueryTabIdByProject[projectId] ?? null,
        activeSchemaTabId: this.state.activeSchemaTabIdByProject[projectId] ?? null,
        activeExplainTabId: this.state.activeExplainTabIdByProject[projectId] ?? null,
        activeErdTabId: this.state.activeErdTabIdByProject[projectId] ?? null,
        activeStatisticsTabId: this.state.activeStatisticsTabIdByProject[projectId] ?? null,
        activeWorkflowTabId: this.state.activeWorkflowTabIdByProject[projectId] ?? null,
        activeView: this.state.activeView,
        activeConnectionId: this.state.activeConnectionIdByProject[projectId] ?? null,
        starterTabs: this.serializeStarterTabs(projectId),
        activeStarterTabId: this.state.activeStarterTabIdByProject[projectId] ?? null,
        savedWorkflows: this.serializeSavedWorkflows(projectId),
        connectionTabs: [],
        activeConnectionTabId: null,
        dashboardTabs: this.serializeDashboardTabs(projectId),
        activeDashboardTabId: this.state.activeDashboardTabIdByProject[projectId] ?? null,
        createTableTabs: this.serializeCreateTableTabs(projectId),
        activeCreateTableTabId: this.state.activeCreateTableTabIdByProject[projectId] ?? null,
        dataTabs: this.serializeDataTabs(projectId),
        activeDataTabId: this.state.activeDataTabIdByProject[projectId] ?? null,
        extensionsDuckdbTabs: this.serializeExtensionsDuckdbTabs(projectId),
        activeExtensionsDuckdbTabId:
          this.state.activeExtensionsDuckdbTabIdByProject[projectId] ?? null,
        starredSharedQueryIds: [], // Legacy: starring is now on the Query object
        starredSharedDashboardIds: [], // Legacy: starring is now on the Dashboard object
        paneLayout: this.serializePaneLayout(projectId),
      };

      await getStorage().projectState.save(state);

      // Only persist saved queries if they've been loaded into memory for this project.
      // Otherwise saveAll would delete all queries (since the in-memory list is empty).
      // The same goes for a project whose saved queries failed to load: its
      // list holds only what was added since, and saveAll would delete the rest.
      if (this.loadFailed(`savedQueries:${projectId}`)) {
        this.refuse(`the saved queries of project ${projectId}`, `savedQueries:${projectId}`);
      } else if (projectId in this.state.queriesByProject) {
        await getStorage().savedQueries.saveAll(projectId, this.serializeSavedQueries(projectId));
      }
    } catch (error) {
      void log.error(`Persistence failed: ${projectId}`);
      void log.error(`Failed to persist state for project ${projectId}:`, error);
    }
  }

  async persistProjectDashboards(projectId: string): Promise<void> {
    try {
      const dashboards = this.state.dashboardsByProject[projectId] ?? [];
      for (const d of dashboards) {
        // Through the shared helper so widget results (runtime rows, possibly
        // bigint) are stripped exactly as on every other save path.
        await getStorage().dashboards.save(toPersistedDashboard(d));
      }
    } catch (error) {
      void log.error(`Failed to persist dashboards for project ${projectId}:`, error);
    }
  }

  async loadProjectState(projectId: string): Promise<PersistedProjectState | null> {
    return this.load(
      `projectState:${projectId}`,
      `persisted state for project ${projectId}`,
      () => getStorage().projectState.load(projectId),
      null,
    );
  }

  async removeProjectState(projectId: string): Promise<void> {
    try {
      await getStorage().projectState.remove(projectId);
    } catch (error) {
      void log.error(`Failed to remove persisted state for project ${projectId}:`, error);
    }
  }

  async removeProject(projectId: string): Promise<void> {
    try {
      await getStorage().projects.remove(projectId);
    } catch (error) {
      void log.error(`Failed to remove project ${projectId}:`, error);
    }
  }

  // === CONNECTION DATA PERSISTENCE (history, saved queries) ===

  async persistConnectionData(connectionId: string): Promise<void> {
    void log.debug(`Persisting connection data: ${connectionId}`);
    // Saving replaces the connection's whole history.
    if (this.loadFailed(`history:${connectionId}`)) {
      this.refuse(`the query history of connection ${connectionId}`, `history:${connectionId}`);
      return;
    }
    try {
      await getStorage().queryHistory.replaceAll(
        connectionId,
        this.serializeQueryHistory(connectionId),
      );
    } catch (error) {
      void log.error(`Persistence failed: ${connectionId}`);
      void log.error(`Failed to persist data for connection ${connectionId}:`, error);
    }
  }

  async loadConnectionData(connectionId: string): Promise<{
    queryHistory: PersistedQueryHistoryItem[];
  }> {
    return {
      queryHistory: await this.load(
        `history:${connectionId}`,
        `data for connection ${connectionId}`,
        () => getStorage().queryHistory.loadByConnection(connectionId),
        [],
      ),
    };
  }

  async loadProjectSavedQueries(projectId: string): Promise<PersistedSavedQuery[]> {
    return this.load(
      `savedQueries:${projectId}`,
      `saved queries for project ${projectId}`,
      () => getStorage().savedQueries.loadByProject(projectId),
      [],
    );
  }

  async loadProjectQueryVersions(projectId: string): Promise<PersistedQueryVersion[]> {
    try {
      return await getStorage().queryVersions.loadByProject(projectId);
    } catch (error) {
      void log.error(`Failed to load query versions for project ${projectId}:`, error);
      return [];
    }
  }

  async persistQueryVersion(version: PersistedQueryVersion): Promise<void> {
    try {
      await getStorage().queryVersions.insert(version);
    } catch (error) {
      void log.error(`Failed to persist query version:`, error);
    }
  }

  async pruneQueryVersions(queryId: string, keepCount: number): Promise<void> {
    try {
      await getStorage().queryVersions.pruneOldVersions(queryId, keepCount);
    } catch (error) {
      void log.error(`Failed to prune query versions:`, error);
    }
  }

  async persistDashboardVersion(version: PersistedDashboardVersion): Promise<void> {
    try {
      await getStorage().dashboardVersions.insert(version);
    } catch (error) {
      void log.error(`Failed to persist dashboard version:`, error);
    }
  }

  async pruneDashboardVersions(dashboardId: string, keepCount: number): Promise<void> {
    try {
      await getStorage().dashboardVersions.pruneOldVersions(dashboardId, keepCount);
    } catch (error) {
      void log.error(`Failed to prune dashboard versions:`, error);
    }
  }

  async loadProjectDashboardVersions(projectId: string): Promise<PersistedDashboardVersion[]> {
    try {
      return await getStorage().dashboardVersions.loadByProject(projectId);
    } catch (error) {
      void log.error(`Failed to load dashboard versions for project ${projectId}:`, error);
      return [];
    }
  }

  async loadProjectDashboards(projectId: string): Promise<PersistedDashboard[]> {
    try {
      return await getStorage().dashboards.loadByProject(projectId);
    } catch (error) {
      void log.error(`Failed to load dashboards for project ${projectId}:`, error);
      return [];
    }
  }

  async removeConnectionData(connectionId: string): Promise<void> {
    // Cancel any pending AI chat persistence timer for this connection
    const aiTimer = this.aiChatTimers.get(connectionId);
    if (aiTimer) {
      clearTimeout(aiTimer);
      this.aiChatTimers.delete(connectionId);
    }

    try {
      await getStorage().queryHistory.removeByConnection(connectionId);
      await getStorage().aiChats.removeByConnection(connectionId);
    } catch (error) {
      void log.error(`Failed to remove data for connection ${connectionId}:`, error);
    }
  }

  // === AI CHAT PERSISTENCE ===

  scheduleAIChats(connectionId: string | null): void {
    if (!connectionId) return;
    const existing = this.aiChatTimers.get(connectionId);
    if (existing) clearTimeout(existing);
    this.aiChatTimers.set(
      connectionId,
      setTimeout(() => {
        void this.persistAIChats(connectionId);
        this.aiChatTimers.delete(connectionId);
      }, this.PERSISTENCE_DEBOUNCE_MS),
    );
  }

  async persistAIChats(connectionId: string): Promise<void> {
    try {
      const chats = this.state.aiChatsByConnection[connectionId] ?? [];
      for (const chat of chats) {
        await getStorage().aiChats.saveChat({
          id: chat.id,
          connectionId: chat.connectionId,
          title: chat.title,
          createdAt: chat.createdAt.toISOString(),
          updatedAt: chat.updatedAt.toISOString(),
        });
      }
    } catch (error) {
      void log.error(`Failed to persist AI chats for connection ${connectionId}:`, error);
    }
  }

  async persistAIChatMessages(chatId: string): Promise<void> {
    try {
      // Ensure the parent chat record exists before inserting messages
      // (chat persistence is debounced so it may not have run yet)
      const chat = Object.values(this.state.aiChatsByConnection)
        .flat()
        .find((c) => c.id === chatId);
      if (chat) {
        await getStorage().aiChats.saveChat({
          id: chat.id,
          connectionId: chat.connectionId,
          title: chat.title,
          createdAt: chat.createdAt.toISOString(),
          updatedAt: chat.updatedAt.toISOString(),
        });
      }

      // Saving replaces every message of the chat.
      if (this.loadFailed(`aiMessages:${chatId}`)) {
        this.refuse(`the messages of AI chat ${chatId}`, `aiMessages:${chatId}`);
        return;
      }
      const messages = (this.state.aiMessagesByChat[chatId] ?? []).filter(
        (m) => !m.pendingModelSelection,
      );
      await getStorage().aiChats.replaceAllMessages(
        chatId,
        messages.map((m) => ({
          id: m.id,
          chatId,
          role: m.role,
          content: m.content,
          timestamp: m.timestamp.toISOString(),
          query: m.query,
          dashboardId: m.dashboardId,
        })),
      );
    } catch (error) {
      void log.error(`Failed to persist AI chat messages for chat ${chatId}:`, error);
    }
  }

  async loadAIChats(connectionId: string): Promise<PersistedAIChat[]> {
    try {
      return await getStorage().aiChats.loadByConnection(connectionId);
    } catch (error) {
      void log.error(`Failed to load AI chats for connection ${connectionId}:`, error);
      return [];
    }
  }

  async loadAIChatMessages(chatId: string): Promise<PersistedAIMessage[]> {
    return this.load(
      `aiMessages:${chatId}`,
      `AI chat messages for chat ${chatId}`,
      () => getStorage().aiChats.loadMessages(chatId),
      [],
    );
  }

  async removeAIChat(chatId: string): Promise<void> {
    try {
      await getStorage().aiChats.removeChat(chatId);
    } catch (error) {
      void log.error(`Failed to remove AI chat ${chatId}:`, error);
    }
  }

  // === CONNECTION PERSISTENCE ===

  stripPasswordFromConnectionString(connectionString?: string): string | undefined {
    if (!connectionString) return undefined;

    try {
      // Handle SQLite
      if (connectionString.startsWith("sqlite://") || connectionString.startsWith("sqlite:")) {
        return connectionString;
      }

      // Parse URL-based connection strings
      let normalized = connectionString.replace("postgresql://", "postgres://");
      const url = new URL(normalized);

      // Remove password from URL
      if (url.password) {
        url.password = "";
      }

      return url.toString().replace("postgres://", "postgresql://");
    } catch {
      // If parsing fails, return original string (it might not be a URL)
      return connectionString;
    }
  }

  /**
   * Saves a connection row, and the secrets the caller asked to change.
   *
   * `options` describes an intent, not the full state: a flag left out means
   * "leave that secret alone", so metadata-only callers (label changes, AI
   * model selection) can omit `options` entirely without wiping the user's
   * keychain entries. Only an explicit `false` deletes a stored secret.
   */
  async persistConnection(
    connection: DatabaseConnection,
    options?: {
      savePassword?: boolean;
      saveSshPassword?: boolean;
      saveSshKeyPassphrase?: boolean;
      sshPassword?: string;
      sshKeyPassphrase?: string;
    },
  ): Promise<void> {
    await withErrorHandling(
      async () => {
        // Fall back to what the connection already carries so an omitted flag
        // doesn't clear the stored one (auto-reconnect reads these at launch).
        const savePassword = options?.savePassword ?? connection.savePassword;
        const saveSshPassword = options?.saveSshPassword ?? connection.saveSshPassword;
        const saveSshKeyPassphrase =
          options?.saveSshKeyPassphrase ?? connection.saveSshKeyPassphrase;

        const persistedConnection: PersistedConnection = {
          id: connection.id,
          name: connection.name,
          type: connection.type,
          host: connection.host,
          port: connection.port,
          databaseName: connection.databaseName,
          username: connection.username,
          sslMode: connection.sslMode,
          connectionString: this.stripPasswordFromConnectionString(connection.connectionString),
          lastConnected: connection.lastConnected,
          sshTunnel: connection.sshTunnel,
          savePassword,
          saveSshPassword,
          saveSshKeyPassphrase,
          projectId: connection.projectId,
          labelIds: connection.labelIds,
          isLocalOnly: connection.isLocalOnly,
          sharedConnectionId: connection.sharedConnectionId,
          aiShareSchema: connection.aiShareSchema,
          aiShareData: connection.aiShareData,
          activeAIProviderId: connection.activeAIProviderId,
          activeAIModel: connection.activeAIModel,
        };

        await getStorage().connections.save(persistedConnection);

        // Save passwords to keyring if enabled
        const keyring = getKeyringService();
        if (keyring.isAvailable()) {
          await withErrorHandling(
            async () => {
              if (options?.savePassword && connection.password) {
                await keyring.setDbPassword(connection.id, connection.password);
              } else if (options?.savePassword === false) {
                await keyring.deleteDbPassword(connection.id);
              }

              if (options?.saveSshPassword && options.sshPassword) {
                await keyring.setSshPassword(connection.id, options.sshPassword);
              } else if (options?.saveSshPassword === false) {
                await keyring.deleteSshPassword(connection.id);
              }

              if (options?.saveSshKeyPassphrase && options.sshKeyPassphrase) {
                await keyring.setSshKeyPassphrase(connection.id, options.sshKeyPassphrase);
              } else if (options?.saveSshKeyPassphrase === false) {
                await keyring.deleteSshKeyPassphrase(connection.id);
              }
            },
            "PERSISTENCE_FAILED",
            "Could not save password to system keychain",
          );
        }
      },
      "PERSISTENCE_FAILED",
      "Failed to save connection to storage",
    );
  }

  async removePersistedConnection(connectionId: string): Promise<void> {
    await withErrorHandling(
      async () => {
        await getStorage().connections.remove(connectionId);

        // Delete passwords from keyring
        const keyring = getKeyringService();
        if (keyring.isAvailable()) {
          try {
            await keyring.deleteAllForConnection(connectionId);
          } catch (error) {
            void log.warn("Failed to delete credentials from keyring:", error);
          }
        }

        // Remove connection data
        await this.removeConnectionData(connectionId);
      },
      "PERSISTENCE_FAILED",
      "Failed to delete connection from storage",
    );
  }

  async loadPersistedConnections(): Promise<PersistedConnection[]> {
    try {
      return await getStorage().connections.loadAll();
    } catch (error) {
      void log.error("Failed to load persisted connections:", error);
      return [];
    }
  }

  // === SHARED QUERY REPOS PERSISTENCE ===

  async persistSharedRepos(): Promise<void> {
    // Saving replaces every stored repo.
    if (this.loadFailed("sharedRepos")) {
      this.refuse("shared query repositories", "sharedRepos");
      return;
    }
    try {
      const repos: PersistedSharedQueryRepo[] = this.state.sharedRepos.map(serializeRepo);
      await getStorage().sharedRepos.saveAll(repos, this.state.activeRepoId);
    } catch (error) {
      void log.error("Failed to persist shared repos:", error);
    }
  }

  async loadSharedRepos(): Promise<{
    repos: PersistedSharedQueryRepo[];
    activeRepoId: string | null;
  }> {
    return this.load("sharedRepos", "shared repos", () => getStorage().sharedRepos.loadAll(), {
      repos: [],
      activeRepoId: null,
    });
  }

  // === CONNECTION OVERRIDES PERSISTENCE ===

  async persistConnectionOverride(override: ConnectionOverride): Promise<void> {
    try {
      await getStorage().connectionOverrides.save({
        sharedConnectionId: override.sharedConnectionId,
        username: override.username,
        hostOverride: override.hostOverride,
        portOverride: override.portOverride,
        savePassword: override.savePassword,
        saveSshPassword: override.saveSshPassword,
        saveSshKeyPassphrase: override.saveSshKeyPassphrase,
      });
    } catch (error) {
      void log.error("Failed to persist connection override:", error);
    }
  }

  async loadConnectionOverrides(): Promise<Record<string, ConnectionOverride>> {
    try {
      const overrides = await getStorage().connectionOverrides.loadAll();
      const result: Record<string, ConnectionOverride> = {};
      for (const o of overrides) {
        result[o.sharedConnectionId] = {
          sharedConnectionId: o.sharedConnectionId,
          username: o.username,
          hostOverride: o.hostOverride,
          portOverride: o.portOverride,
          savePassword: o.savePassword,
          saveSshPassword: o.saveSshPassword,
          saveSshKeyPassphrase: o.saveSshKeyPassphrase,
        };
      }
      return result;
    } catch (error) {
      void log.error("Failed to load connection overrides:", error);
      return {};
    }
  }

  async removeConnectionOverride(sharedConnectionId: string): Promise<void> {
    try {
      await getStorage().connectionOverrides.remove(sharedConnectionId);
    } catch (error) {
      void log.error("Failed to remove connection override:", error);
    }
  }
}
