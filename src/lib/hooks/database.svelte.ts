import { setContext, getContext } from "svelte";
import type { SchemaTable, ActiveViewType } from "$lib/types";
import type { EngineClient } from "$lib/engine";
import { log } from "$lib/utils/logger";
import { DatabaseState } from "./database/state.svelte.js";
import { PersistenceManager } from "./database/persistence-manager.svelte.js";
import { StateRestorationManager } from "./database/state-restoration.svelte.js";
import { TabOrderingManager } from "./database/tab-ordering.svelte.js";
import { ConnectionManager } from "./database/connection-manager.svelte.js";
import { QueryExecutionManager } from "./database/query-execution.svelte.js";
import { UIStateManager } from "./database/ui-state.svelte.js";
import { QueryTabManager } from "./database/query-tabs.svelte.js";
import { QueryHistoryManager } from "./database/query-history.svelte.js";
import { SavedQueryManager } from "./database/saved-queries.svelte.js";
import { SchemaTabManager } from "./database/schema-tabs.svelte.js";
import { ExplainTabManager } from "./database/explain-tabs.svelte.js";
import { ErdTabManager } from "./database/erd-tabs.svelte.js";
import { StatisticsTabManager } from "./database/statistics-tabs.svelte.js";
import { ExtensionsDuckdbTabManager } from "./database/extensions-duckdb-tabs.svelte.js";
import { WorkflowTabManager } from "./database/workflow-tabs.svelte.js";
import { VisualizeTabManager } from "./database/visualize-tabs.svelte.js";
import { ConnectionTabManager } from "./database/connection-tabs.svelte.js";
import { ProjectManager } from "./database/project-manager.svelte.js";
import { LabelManager } from "./database/label-manager.svelte.js";
import { StarterTabManager } from "./database/starter-tabs.svelte.js";
import { SettingsTabManager } from "./database/settings-tabs.svelte.js";
import { CreateTableTabManager } from "./database/create-table-tabs.svelte.js";
import { DataTabManager } from "./database/data-tabs.svelte.js";
import { DashboardTabManager } from "./database/dashboard-tabs.svelte.js";
import { DashboardManager } from "./database/dashboard-manager.svelte.js";
import { WorkflowState } from "./database/workflow-state.svelte.js";
import { WorkflowManager } from "./database/workflow-manager.svelte.js";
import { SharedRepoManager } from "./database/shared-repo-manager.svelte.js";
import { SharedQueryManager } from "./database/shared-query-manager.svelte.js";
import { SharedDashboardManager } from "./database/shared-dashboard-manager.svelte.js";
import { AIChatManager } from "./database/ai-chat-manager.svelte.js";
import { PaneManager } from "./database/pane-manager.svelte.js";
import { PendingChangesManager } from "./database/pending-changes.svelte.js";
import { ProviderRegistry } from "$lib/providers";
import { aiSettingsStore } from "$lib/stores/ai-settings.svelte";
import { pendingChangesSettingsStore } from "$lib/stores/pending-changes-settings.svelte";
import { editorSettingsStore } from "$lib/stores/editor-settings.svelte";
import { getDatabase } from "$lib/storage/db";

/**
 * Main database context class that orchestrates all managers.
 *
 * Usage:
 *   const db = useDatabase();
 *   db.connections.add(connection);
 *   db.queryTabs.add("My Query", "SELECT * FROM users");
 *   db.queries.execute(tabId);
 */
class UseDatabase {
  // Core state - exposes all reactive state and derived values
  readonly state: DatabaseState;

  // Managers
  readonly persistence: PersistenceManager;
  readonly projects: ProjectManager;
  readonly labels: LabelManager;
  readonly connections: ConnectionManager;
  readonly tabs: TabOrderingManager;
  readonly queries: QueryExecutionManager;
  readonly ui: UIStateManager;
  readonly queryTabs: QueryTabManager;
  readonly history: QueryHistoryManager;
  readonly savedQueries: SavedQueryManager;
  readonly schemaTabs: SchemaTabManager;
  readonly explainTabs: ExplainTabManager;
  readonly erdTabs: ErdTabManager;
  readonly statisticsTabs: StatisticsTabManager;
  readonly extensionsDuckdbTabs: ExtensionsDuckdbTabManager;
  readonly workflowTabs: WorkflowTabManager;
  readonly visualizeTabs: VisualizeTabManager;
  readonly starterTabs: StarterTabManager;
  readonly dashboardTabs: DashboardTabManager;
  readonly dashboards: DashboardManager;
  readonly workflowState: WorkflowState;
  readonly workflow: WorkflowManager;
  readonly sharedRepos: SharedRepoManager;
  readonly connectionTabs: ConnectionTabManager;
  readonly settingsTabs: SettingsTabManager;
  readonly createTableTabs: CreateTableTabManager;
  readonly dataTabs: DataTabManager;
  readonly sharedQueries: SharedQueryManager;
  readonly sharedDashboards: SharedDashboardManager;
  readonly aiChats: AIChatManager;
  readonly panes: PaneManager;
  readonly pendingChanges: PendingChangesManager;

  private _stateRestoration: StateRestorationManager;
  private _readyResolve!: () => void;
  private _readyPromise: Promise<void>;

  constructor() {
    this._readyPromise = new Promise<void>((resolve) => {
      this._readyResolve = resolve;
    });
    this.state = new DatabaseState();

    const scheduleProjectPersistence = (projectId: string | null) => {
      this.persistence.scheduleProject(projectId);
    };

    const scheduleConnectionDataPersistence = (connectionId: string | null) => {
      this.persistence.scheduleConnectionData(connectionId);
    };

    const setActiveView = (view: ActiveViewType) => {
      this.ui.setActiveView(view);
    };

    // Core infrastructure
    this.persistence = new PersistenceManager(this.state);
    this.panes = new PaneManager(this.state, scheduleProjectPersistence);
    this.tabs = new TabOrderingManager(this.state, scheduleProjectPersistence, this.panes);
    this._stateRestoration = new StateRestorationManager(this.state, this.persistence);

    // Project and label management
    this.projects = new ProjectManager(this.state, this.persistence, this._stateRestoration);
    this.labels = new LabelManager(this.state, this.persistence);

    // AI chats
    this.aiChats = new AIChatManager(
      this.state,
      (connectionId) => this.persistence.scheduleAIChats(connectionId),
      (chatId) => this._stateRestoration.loadAIChatMessages(chatId),
      (chatId) => this.persistence.persistAIChatMessages(chatId),
      (chatId) => this.persistence.removeAIChat(chatId),
    );

    // Dashboard tabs & manager (before UI, since UIStateManager needs them)
    this.dashboardTabs = new DashboardTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.dashboards = new DashboardManager(
      this.state,
      async (query: string) => {
        return await this.queries.executeRaw(query);
      },
      scheduleProjectPersistence,
      this.persistence,
    );

    // UI
    this.ui = new UIStateManager(
      this.state,
      scheduleProjectPersistence,
      (query) => this.queries.executeRaw(query),
      this.aiChats,
      (chatId) => this.persistence.persistAIChatMessages(chatId),
      this.dashboards,
      this.dashboardTabs,
    );

    // Shared provider registry (connections, query execution and CRUD, pending changes, data and create-table tabs)
    const providers = new ProviderRegistry();

    // Tab managers
    this.queryTabs = new QueryTabManager(this.state, this.tabs, scheduleProjectPersistence);
    this.schemaTabs = new SchemaTabManager(this.state, this.tabs, scheduleProjectPersistence);
    this.explainTabs = new ExplainTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.erdTabs = new ErdTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.statisticsTabs = new StatisticsTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.extensionsDuckdbTabs = new ExtensionsDuckdbTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
      async (query: string) => {
        const result = await this.queries.executeRaw(query);
        return result;
      },
    );
    this.workflowTabs = new WorkflowTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.visualizeTabs = new VisualizeTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.connectionTabs = new ConnectionTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.starterTabs = new StarterTabManager(this.state, this.tabs, scheduleProjectPersistence);
    this.settingsTabs = new SettingsTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
    );
    this.createTableTabs = new CreateTableTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
      providers,
      async (connectionId: string) => {
        await this.connections.refreshSchema(connectionId);
      },
      () => this.pendingChanges,
    );

    // Workflow
    this.workflowState = new WorkflowState();
    this.workflow = new WorkflowManager(
      this.state,
      this.workflowState,
      scheduleProjectPersistence,
      async (query: string) => {
        return await this.queries.executeRaw(query);
      },
    );

    // Query-related
    this.history = new QueryHistoryManager(
      this.state,
      scheduleConnectionDataPersistence,
      (connectionId) => this.labels.getConnectionLabelsById(connectionId),
      (connectionId) => this.state.connections.find((c) => c.id === connectionId)?.name || "",
    );
    this.savedQueries = new SavedQueryManager(
      this.state,
      scheduleProjectPersistence,
      this.persistence,
    );
    this.savedQueries.setRemoveTab((id) => this.queryTabs.remove(id));
    this.pendingChanges = new PendingChangesManager(this.state, providers, this.history);
    this.queries = new QueryExecutionManager(
      this.state,
      this.history,
      providers,
      this.pendingChanges,
    );
    this.dataTabs = new DataTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
      this.queries,
      providers,
    );

    // Shared query library
    this.sharedRepos = new SharedRepoManager(this.state, () =>
      this.persistence.scheduleSharedRepos(),
    );
    this.sharedQueries = new SharedQueryManager(this.state, this.sharedRepos);
    this.sharedDashboards = new SharedDashboardManager(this.state, this.sharedRepos);
    this.queryTabs.setSharedQueryManager(this.sharedQueries);

    // Wire up file projection: managers delegate file I/O to shared managers
    this.savedQueries.setFileProjection({
      writeQueryFile: (query) => this.sharedQueries.writeQueryFile(query),
      deleteQueryFile: (query) => this.sharedQueries.deleteQueryFile(query),
    });
    this.dashboards.setFileProjection({
      writeDashboardFile: (dashboard) => this.sharedDashboards.writeDashboardFile(dashboard),
      deleteDashboardFile: (dashboard) => this.sharedDashboards.deleteDashboardFile(dashboard),
    });

    // Connections (depends on other managers)
    this.connections = new ConnectionManager(
      this.state,
      this.persistence,
      this._stateRestoration,
      this.tabs,
      providers,
      (connectionId: string, schemas: SchemaTable[], client: EngineClient) => {
        // Every schema (re)load passes here: columns loaded for cast maps may be stale.
        this.queries.crud.forgetLoadedColumns(connectionId);
        return this.schemaTabs.loadTableMetadataInBackground(connectionId, schemas, client);
      },
      () => {
        this.queryTabs.add();
        this.ui.setActiveView("query");
      },
      () => {
        this.ui.resetAISessionState();
      },
    );

    // Set up cross-manager callbacks
    this.projects.setRemoveConnectionCallback(
      async (connectionId: string, options?: { skipUnshare?: boolean }) => {
        await this.connections.remove(connectionId, options);
      },
    );

    this.projects.setSharedRepoManager(this.sharedRepos);
    this.projects.setSharedQueryManager(this.sharedQueries);
    this.projects.setSharedDashboardManager(this.sharedDashboards);
    this.projects.setStarterTabManager(this.starterTabs);
    this.connections.setSharedRepoManager(this.sharedRepos);

    // Set up embedded explain callbacks
    this.explainTabs.setEmbeddedCallbacks(
      (tabId, result, sourceQuery, isAnalyze) => {
        this.queryTabs.setExplainResult(tabId, result, sourceQuery, isAnalyze);
      },
      (tabId, isExecuting, isAnalyze) => {
        this.queryTabs.setExplainExecuting(tabId, isExecuting, isAnalyze);
      },
    );

    // Set up embedded visualize callback
    this.visualizeTabs.setEmbeddedCallback((tabId, parsedQuery, sourceQuery, parseError) => {
      this.queryTabs.setVisualizeResult(tabId, parsedQuery, sourceQuery, parseError);
    });

    // Initialize: projects first, then connections
    void this.initializeApp();
  }

  /**
   * Returns a promise that resolves when app initialization is complete.
   */
  whenReady(): Promise<void> {
    return this._readyPromise;
  }

  /**
   * Initialize the application state.
   * Projects are loaded first, then connections, then shared repos.
   */
  private async initializeApp(): Promise<void> {
    try {
      void log.info("Initializing app");

      // Initialize projects (runs migrations if needed)
      await this.projects.initialize();
      void log.info("Projects initialized");

      // Initialize settings and connections in parallel (all independent after projects)
      const sqliteDb = await getDatabase();
      await Promise.all([
        aiSettingsStore.initialize(sqliteDb),
        pendingChangesSettingsStore.load(),
        editorSettingsStore.load(),
        this.connections.initializePersistedConnections(),
      ]);
      void log.info(
        `Settings and connections initialized (count=${this.state.connections.length})`,
      );

      // Initialize shared repos
      await this.initializeSharedRepos();
      void log.info(`Shared repos initialized (count=${this.state.sharedRepos.length})`);

      void log.info("App ready");
    } catch (error) {
      void log.error("App initialization failed");
      console.error("Failed to initialize app:", error);
    } finally {
      this._readyResolve();
    }
  }

  /**
   * Initialize shared query repositories from persisted state.
   */
  private async initializeSharedRepos(): Promise<void> {
    try {
      const { repos, activeRepoId } = await this.persistence.loadSharedRepos();

      // Convert persisted repos to runtime form
      const { deserializeRepo } = await import("$lib/types/shared-queries");
      this.state.sharedRepos = repos.map(deserializeRepo);
      this.state.activeRepoId = activeRepoId;

      // Load queries and refresh status for each repo in parallel
      await Promise.all(
        this.state.sharedRepos.map(async (repo) => {
          await this.sharedRepos.loadQueriesFromRepo(repo.id);
          await this.sharedRepos.refreshRepoStatus(repo.id);
        }),
      );

      // Reconcile git files with local state for the active project
      if (this.state.activeProjectId && this.state.activeRepoId) {
        await this.projects.reconcileGitState(this.state.activeProjectId);
      }

      // Start background refresh if there are repos
      if (this.state.sharedRepos.length > 0) {
        this.sharedRepos.startBackgroundRefresh();
      }
    } catch (error) {
      console.error("Failed to initialize shared repos:", error);
    }
  }

  async setConnectionAIModel(
    connectionId: string,
    providerId: string,
    model: string,
  ): Promise<void> {
    const conn = this.state.connections.find((c) => c.id === connectionId);
    if (!conn) return;
    const updated = { ...conn, activeAIProviderId: providerId, activeAIModel: model };
    this.state.connections = this.state.connections.map((c) =>
      c.id === connectionId ? updated : c,
    );
    await this.persistence.persistConnection(updated);
  }

  /**
   * Clean up resources when the database context is destroyed.
   *
   * Pending debounced writes are flushed rather than cancelled — dropping them
   * would lose whatever the user changed in the last debounce window.
   */
  destroy(): void {
    this.sharedRepos.stopBackgroundRefresh();
    this.dashboards.stopAllAutoRefresh();
    void this.persistence.flush();
  }
}

export const setDatabase = () => setContext("database", new UseDatabase());
export const useDatabase = () => getContext<ReturnType<typeof setDatabase>>("database");
