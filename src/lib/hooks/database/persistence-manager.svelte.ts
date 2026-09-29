import { extractErrorMessage } from "$lib/errors";
import { errorToast } from "$lib/utils/toast";
import { m } from "$lib/paraglide/messages.js";
import type {
  PersistedQueryTab,
  PersistedSchemaTab,
  PersistedExplainTab,
  PersistedErdTab,
  PersistedStatisticsTab,
  PersistedWorkflowTab,
  PersistedStarterTab,
  PersistedDashboardTab,
  PersistedQueryHistoryItem,
  PersistedAIChat,
  PersistedAIMessage,
  PersistedDashboardVersion,
  PersistedProjectState,
  PersistedSharedQueryRepo,
} from "$lib/types";
import { serializeRepo } from "$lib/types";
import type { SavedWorkflow } from "$lib/types/workflow";
import { toPersistedDashboard } from "./dashboard-serialize.js";
import type { DatabaseState } from "./state.svelte.js";
import type { ConnectionOverride } from "$lib/types";
import { getStorage, type PersistedDashboard } from "$lib/storage";
import { log } from "$lib/utils/logger";
import { skipUnloadedSave } from "$lib/storage/load-guard";

/** Versions kept per saved query or dashboard when the setting is unset. */
const DEFAULT_VERSION_LIMIT = 100;

/**
 * A stored collection whose save replaces what's stored, keyed so a failed
 * load can be recorded (see `load-guard.ts`).
 */
export type LoadKey = "sharedRepos" | `projectState:${string}` | `aiMessages:${string}`;

/**
 * Manages persistence of the projects' state (tabs, layout, saved
 * workflows), dashboards, AI chats, shared repos and connection overrides.
 * Handles serialization, debounced saving, and state loading.
 *
 * Storage goes through `getStorage()`. The library (connections, projects,
 * custom labels, saved queries and their versions) isn't here: it is
 * written through `LibraryService` with one targeted call per change
 * (phase 5d-1), so nothing of it is saved whole or on a timer.
 */
export class PersistenceManager {
  // Keyed per project/connection: a single shared timer meant scheduling a save
  // for one project cancelled another project's pending write, losing it.
  private projectTimers = new Map<string, ReturnType<typeof setTimeout>>();
  private sharedReposTimer: ReturnType<typeof setTimeout> | null = null;
  private aiChatTimers = new Map<string, ReturnType<typeof setTimeout>>();
  readonly PERSISTENCE_DEBOUNCE_MS = 500;

  /**
   * Collections whose last load failed or is still running. Their in-memory copy is empty or
   * default, not what's stored, so their replacing saves are refused until
   * a later load succeeds. A collection that was never loaded (a new
   * project, say) isn't in here and saves normally.
   */
  private failedLoads = new Set<LoadKey>();
  /** The subset of `failedLoads` whose read is still running. */
  private pendingLoads = new Set<LoadKey>();

  /**
   * Whether the projects were read at startup. Without them the active
   * project is an in-memory stand-in, whose id must not replace the stored
   * choice and whose state can't be stored.
   */
  private projectsLoaded = true;

  constructor(private state: DatabaseState) {}

  /** Set by `ProjectManager.initialize`. */
  setProjectsLoaded(loaded: boolean): void {
    this.projectsLoaded = loaded;
  }

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
  }

  /**
   * Cancel pending writes for one project, leaving other projects' pending
   * writes alone. Used when deleting a project, where a queued write would
   * recreate its rows.
   */
  cancelPendingPersistenceFor(projectId: string): void {
    const projectTimer = this.projectTimers.get(projectId);
    if (projectTimer) {
      clearTimeout(projectTimer);
      this.projectTimers.delete(projectId);
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

  // === APP STATE PERSISTENCE ===

  async persistAppState(): Promise<void> {
    // With no stored projects loaded, the active project is an in-memory
    // stand-in whose id must not replace the stored choice.
    if (!this.projectsLoaded) {
      skipUnloadedSave("the active project", { pending: false });
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
    if (!this.projectsLoaded) {
      skipUnloadedSave(`the state of project ${projectId}`, { pending: false });
      return;
    }
    if (this.loadFailed(`projectState:${projectId}`)) {
      this.refuse(`the state of project ${projectId}`, `projectState:${projectId}`);
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

  // === CONNECTION DATA (history) ===
  // History is written by targeted calls in `QueryHistoryManager` (append,
  // set favourite), never by replacing the list, so a failed load has no
  // save to block: the cache stays empty and appends still reach the file.

  async loadConnectionData(connectionId: string): Promise<{
    queryHistory: PersistedQueryHistoryItem[];
  }> {
    try {
      return { queryHistory: await getStorage().queryHistory.loadByConnection(connectionId) };
    } catch (error) {
      void log.error(`Failed to load data for connection ${connectionId}:`, error);
      return { queryHistory: [] };
    }
  }

  /**
   * A version limit setting (`query_version_limit`, `dashboard_version_limit`),
   * or 100 when it's unset, unreadable, negative or not a number.
   */
  async versionLimit(key: "query_version_limit" | "dashboard_version_limit"): Promise<number> {
    try {
      const value = await getStorage().appState.get(key);
      const limit = value ? parseInt(value, 10) : NaN;
      return Number.isNaN(limit) || limit < 0 ? DEFAULT_VERSION_LIMIT : limit;
    } catch (error) {
      void log.error(`Failed to read ${key}:`, error);
      return DEFAULT_VERSION_LIMIT;
    }
  }

  /** Inserts a dashboard version. Throws on failure, so the caller doesn't prune after it. */
  async persistDashboardVersion(version: PersistedDashboardVersion): Promise<void> {
    await getStorage().dashboardVersions.insert(version);
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
      errorToast(m.connection_override_save_failed({ message: extractErrorMessage(error) }));
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
