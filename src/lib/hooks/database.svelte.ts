import { setContext, getContext } from "svelte";
import type { SchemaTable } from "$lib/types";
import type { DatabaseAdapter } from "$lib/db";
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
import { CanvasTabManager } from "./database/canvas-tabs.svelte.js";
import { VisualizeTabManager } from "./database/visualize-tabs.svelte.js";
import { ProjectManager } from "./database/project-manager.svelte.js";
import { LabelManager } from "./database/label-manager.svelte.js";
import { StarterTabManager } from "./database/starter-tabs.svelte.js";
import { CanvasState } from "./database/canvas-state.svelte.js";
import { CanvasManager } from "./database/canvas-manager.svelte.js";
import { SharedRepoManager } from "./database/shared-repo-manager.svelte.js";
import { SharedQueryManager } from "./database/shared-query-manager.svelte.js";

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
  readonly canvasTabs: CanvasTabManager;
  readonly visualizeTabs: VisualizeTabManager;
  readonly starterTabs: StarterTabManager;
  readonly canvasState: CanvasState;
  readonly canvas: CanvasManager;
  readonly sharedRepos: SharedRepoManager;
  readonly sharedQueries: SharedQueryManager;

  private _stateRestoration: StateRestorationManager;

  constructor() {
    this.state = new DatabaseState();

    const scheduleProjectPersistence = (projectId: string | null) => {
      this.persistence.scheduleProject(projectId);
    };

    const scheduleConnectionDataPersistence = (connectionId: string | null) => {
      this.persistence.scheduleConnectionData(connectionId);
    };

    const setActiveView = (view: "query" | "schema" | "explain" | "erd" | "statistics" | "canvas" | "visualize") => {
      this.ui.setActiveView(view);
    };

    // Core infrastructure
    this.persistence = new PersistenceManager(this.state);
    this.tabs = new TabOrderingManager(this.state, scheduleProjectPersistence);
    this._stateRestoration = new StateRestorationManager(this.state, this.persistence);

    // Project and label management
    this.projects = new ProjectManager(this.state, this.persistence);
    this.labels = new LabelManager(this.state, this.persistence);

    // UI
    this.ui = new UIStateManager(this.state, scheduleProjectPersistence);

    // Tab managers
    this.queryTabs = new QueryTabManager(this.state, this.tabs, scheduleProjectPersistence);
    this.schemaTabs = new SchemaTabManager(this.state, this.tabs, scheduleProjectPersistence);
    this.explainTabs = new ExplainTabManager(this.state, this.tabs, scheduleProjectPersistence, setActiveView);
    this.erdTabs = new ErdTabManager(this.state, this.tabs, scheduleProjectPersistence, setActiveView);
    this.statisticsTabs = new StatisticsTabManager(
      this.state,
      this.tabs,
      scheduleProjectPersistence,
      setActiveView,
      async (query: string) => {
        // Execute query on the active connection and return raw results
        const result = await this.queries.executeRaw(query);
        return result;
      }
    );
    this.canvasTabs = new CanvasTabManager(this.state, this.tabs, scheduleProjectPersistence, setActiveView);
    this.visualizeTabs = new VisualizeTabManager(this.state, this.tabs, scheduleProjectPersistence, setActiveView);
    this.starterTabs = new StarterTabManager(this.state, scheduleProjectPersistence);

    // Canvas
    this.canvasState = new CanvasState();
    this.canvas = new CanvasManager(
      this.state,
      this.canvasState,
      scheduleProjectPersistence,
      async (query: string) => {
        return await this.queries.executeRaw(query);
      }
    );

    // Query-related
    this.history = new QueryHistoryManager(
      this.state,
      scheduleConnectionDataPersistence,
      (connectionId) => this.labels.getConnectionLabelsById(connectionId),
      (connectionId) => this.state.connections.find((c) => c.id === connectionId)?.name || ""
    );
    this.savedQueries = new SavedQueryManager(this.state, scheduleConnectionDataPersistence, scheduleProjectPersistence);
    this.queries = new QueryExecutionManager(this.state, this.history);

    // Shared query library
    this.sharedRepos = new SharedRepoManager(this.state, () => this.persistence.scheduleSharedRepos());
    this.sharedQueries = new SharedQueryManager(this.state, this.sharedRepos);

    // Connections (depends on other managers)
    this.connections = new ConnectionManager(
      this.state,
      this.persistence,
      this._stateRestoration,
      this.tabs,
      (connectionId: string, schemas: SchemaTable[], adapter: DatabaseAdapter, providerConnectionId?: string, mssqlConnectionId?: string) => {
        this.schemaTabs.loadTableMetadataInBackground(connectionId, schemas, adapter, providerConnectionId, mssqlConnectionId);
      },
      () => this.queryTabs.add()
    );

    // Set up cross-manager callbacks
    this.projects.setRemoveConnectionCallback((connectionId: string) => {
      this.connections.remove(connectionId);
    });

    this.projects.setInitializeStarterTabsCallback((projectId: string) => {
      this.starterTabs.initializeDefaults(projectId);
    });

    // Set up embedded explain callbacks
    this.explainTabs.setEmbeddedCallbacks(
      (tabId, result, sourceQuery, isAnalyze) => {
        this.queryTabs.setExplainResult(tabId, result, sourceQuery, isAnalyze);
      },
      (tabId, isExecuting, isAnalyze) => {
        this.queryTabs.setExplainExecuting(tabId, isExecuting, isAnalyze);
      }
    );

    // Set up embedded visualize callback
    this.visualizeTabs.setEmbeddedCallback(
      (tabId, parsedQuery, sourceQuery, parseError) => {
        this.queryTabs.setVisualizeResult(tabId, parsedQuery, sourceQuery, parseError);
      }
    );

    // Initialize: projects first, then connections
    this.initializeApp();
  }

  /**
   * Initialize the application state.
   * Projects are loaded first, then connections, then shared repos.
   */
  private async initializeApp(): Promise<void> {
    try {
      // Initialize projects (runs migrations if needed)
      await this.projects.initialize();

      // Initialize connections
      await this.connections.initializePersistedConnections();

      // Initialize shared repos
      await this.initializeSharedRepos();
    } catch (error) {
      console.error("Failed to initialize app:", error);
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

      // Load queries from each repo
      for (const repo of this.state.sharedRepos) {
        await this.sharedRepos.loadQueriesFromRepo(repo.id);
        await this.sharedRepos.refreshRepoStatus(repo.id);
      }

      // Start background refresh if there are repos
      if (this.state.sharedRepos.length > 0) {
        this.sharedRepos.startBackgroundRefresh();
      }
    } catch (error) {
      console.error("Failed to initialize shared repos:", error);
    } finally {
      this.state.sharedReposLoading = false;
    }
  }

  /**
   * Clean up resources when the database context is destroyed.
   */
  destroy(): void {
    this.sharedRepos.stopBackgroundRefresh();
  }
}

export const setDatabase = () => setContext("database", new UseDatabase());
export const useDatabase = () => getContext<ReturnType<typeof setDatabase>>("database");
