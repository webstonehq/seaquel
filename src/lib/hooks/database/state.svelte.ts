import type {
  DatabaseConnection,
  SchemaTable,
  QueryTab,
  QueryHistoryItem,
  AIChat,
  AIMessage,
  SchemaTab,
  Query,
  ExplainTab,
  ErdTab,
  StatisticsTab,
  WorkflowTab,
  VisualizeTab,
  ConnectionTab,
  Project,
  StarterTab,
  SettingsTab,
  SharedQueryRepo,
  SharedQuery,
  SyncState,
  DashboardTab,
  Dashboard,
  SharedProject,
  SharedConnection,
  SharedDashboard,
  ActiveViewType,
  QueryVersion,
  DashboardVersion,
  CreateTableTab,
  DataTab,
  ExtensionsDuckdbTab,
  PendingChange,
} from "$lib/types";
import type { PaneLayout } from "$lib/types";
import type { ConnectionLabel } from "$lib/types/project";
import type { SavedWorkflowSummary } from "$lib/types/workflow";
import type { EventsUnavailableReason } from "$lib/core/client";
import { RowSeqs } from "./library/seqs";

/**
 * Central state container for the database module.
 * All reactive state and derived values are declared here.
 * Modules receive this instance and read/write state through it.
 *
 * State is organized using Records (objects) instead of Maps for simpler
 * reactivity updates using spread syntax.
 *
 * Tabs are organized per-PROJECT (not per-connection) to allow:
 * - Switching between projects with separate tab sets
 * - Executing queries against different connections within the same project
 *
 * Query history and saved queries remain per-CONNECTION since they are
 * tied to the specific connection that executed them.
 */
export class DatabaseState {
  // === LIBRARY SYNC (phase 5d-1) ===
  /** The `seq` last applied per library row, and this page's writes in flight. */
  readonly librarySeqs = new RowSeqs();
  /**
   * How many times another window's change was applied to a library row,
   * by `rowKey` (`connection:<id>`, `project:<id>`). A form
   * editing a row compares it with the count it opened with, to say
   * "Changed in another window".
   */
  libraryRemoteRevision = $state<Record<string, number>>({});
  /** Why other windows' changes stopped arriving, or `null` while they arrive. */
  libraryUpdatesUnavailable = $state<EventsUnavailableReason | null>(null);

  // === PROJECT STATE ===
  projects = $state<Project[]>([]);
  projectsLoading = $state(true);
  activeProjectId = $state<string | null>(null);

  // === CONNECTION STATE ===
  connections = $state<DatabaseConnection[]>([]);
  connectionsLoading = $state(true);
  schemas = $state<Record<string, SchemaTable[]>>({});

  // Active connection tracked per project
  activeConnectionIdByProject = $state<Record<string, string | null>>({});

  // === TABS STATE (per-project) ===
  queryTabsByProject = $state<Record<string, QueryTab[]>>({});
  activeQueryTabIdByProject = $state<Record<string, string | null>>({});

  schemaTabsByProject = $state<Record<string, SchemaTab[]>>({});
  activeSchemaTabIdByProject = $state<Record<string, string | null>>({});

  explainTabsByProject = $state<Record<string, ExplainTab[]>>({});
  activeExplainTabIdByProject = $state<Record<string, string | null>>({});

  erdTabsByProject = $state<Record<string, ErdTab[]>>({});
  activeErdTabIdByProject = $state<Record<string, string | null>>({});

  statisticsTabsByProject = $state<Record<string, StatisticsTab[]>>({});
  activeStatisticsTabIdByProject = $state<Record<string, string | null>>({});

  workflowTabsByProject = $state<Record<string, WorkflowTab[]>>({});
  activeWorkflowTabIdByProject = $state<Record<string, string | null>>({});

  visualizeTabsByProject = $state<Record<string, VisualizeTab[]>>({});
  activeVisualizeTabIdByProject = $state<Record<string, string | null>>({});

  connectionTabsByProject = $state<Record<string, ConnectionTab[]>>({});
  activeConnectionTabIdByProject = $state<Record<string, string | null>>({});

  // Saved workflows per project
  /** Each project's saved workflows, without their bodies (opening one reads it). */
  savedWorkflowsByProject = $state<Record<string, SavedWorkflowSummary[]>>({});

  // === DASHBOARD TABS STATE (per-project) ===
  dashboardTabsByProject = $state<Record<string, DashboardTab[]>>({});
  activeDashboardTabIdByProject = $state<Record<string, string | null>>({});

  // === DASHBOARD DATA STATE (per-project) ===
  dashboardsByProject = $state<Record<string, Dashboard[]>>({});

  // === STARTER TABS STATE (per-project) ===
  // Shown when no connection is active
  starterTabsByProject = $state<Record<string, StarterTab[]>>({});
  activeStarterTabIdByProject = $state<Record<string, string | null>>({});

  // === SETTINGS TABS STATE (per-project) ===
  settingsTabsByProject = $state<Record<string, SettingsTab[]>>({});
  activeSettingsTabIdByProject = $state<Record<string, string | null>>({});

  // === CREATE TABLE TABS STATE (per-project) ===
  createTableTabsByProject = $state<Record<string, CreateTableTab[]>>({});
  activeCreateTableTabIdByProject = $state<Record<string, string | null>>({});

  // === DATA TABS STATE (per-project) ===
  dataTabsByProject = $state<Record<string, DataTab[]>>({});
  activeDataTabIdByProject = $state<Record<string, string | null>>({});

  // === DUCKDB EXTENSIONS TABS STATE (per-project) ===
  extensionsDuckdbTabsByProject = $state<Record<string, ExtensionsDuckdbTab[]>>({});
  activeExtensionsDuckdbTabIdByProject = $state<Record<string, string | null>>({});

  // Tab ordering state (stores ordered array of all tab IDs per project)
  tabOrderByProject = $state<Record<string, string[]>>({});

  // Connection ordering state (stores ordered array of connection IDs per project)
  connectionOrderByProject = $state<Record<string, string[]>>({});

  // Pane layout state (stores split pane configuration per project)
  paneLayoutByProject = $state<Record<string, PaneLayout>>({});

  // === QUERY DATA STATE ===
  queryHistoryByConnection = $state<Record<string, QueryHistoryItem[]>>({});
  queriesByProject = $state<Record<string, Query[]>>({});
  queryVersionsByProject = $state<Record<string, QueryVersion[]>>({});
  dashboardVersionsByProject = $state<Record<string, DashboardVersion[]>>({});

  // === SHARED QUERY LIBRARY STATE ===
  sharedRepos = $state<SharedQueryRepo[]>([]);
  activeRepoId = $state<string | null>(null);
  /** Internal scan cache: raw .sql file contents from git repos. Used for reconciliation only. */
  sharedQueriesByRepo = $state<Record<string, SharedQuery[]>>({});
  sharedDashboardsByRepo = $state<Record<string, SharedDashboard[]>>({});
  syncStateByRepo = $state<Record<string, SyncState>>({});

  // === PROJECT GIT SYNC STATE ===
  /** Git sync state per project (for projects with gitRepoPath) */
  projectGitSyncState = $state<Record<string, SyncState>>({});

  // === SHARED CONFIG STATE (from .seaquel/ directories) ===
  /** Repo-wide shared labels from labels.yaml, keyed by repo ID */
  sharedLabelsByRepo = $state<Record<string, ConnectionLabel[]>>({});
  /** Shared projects from .seaquel/projects/, keyed by repo ID */
  sharedProjectsByRepo = $state<Record<string, SharedProject[]>>({});
  /** Shared connections keyed by shared project ID */
  sharedConnectionsByProject = $state<Record<string, SharedConnection[]>>({});

  // === AI STATE ===
  aiChatsByConnection = $state<Record<string, AIChat[]>>({});
  activeAIChatIdByConnection = $state<Record<string, string | null>>({});
  aiMessagesByChat = $state<Record<string, AIMessage[]>>({});
  isAIStreaming = $state(false);
  /** The chat whose turn is streaming, if any: deleting it aborts the turn, and a close flush saves it. */
  aiStreamingChatId = $state<string | null>(null);
  /**
   * Chats the web's budget has filled (`max_chat_bytes`, Q17): a refused
   * turn, or stored bytes at the budget when opened. Sending is off there.
   */
  aiChatFull = $state<Record<string, true>>({});
  /**
   * Per chat, each message as it was last loaded or sent (its stored form,
   * as JSON text): a put sends only the ones that differ (Decision 24).
   * Not reactive: nothing shows it.
   */
  readonly aiMessagesSent = new Map<string, Map<string, string>>();
  /**
   * Per project, the connection order as last read or stored: a view-state
   * save first stores the page's order when it differs (a connection was
   * appended since). Not reactive.
   */
  readonly connectionOrderStored = new Map<string, string[]>();
  isDashboardFullscreen = $state(false);

  // === RIGHT PANEL STATE ===
  /** Which right-side panel is currently open, or null if none */
  activeRightPanel = $state<"ai" | "pendingChanges" | null>(null);

  get isAIOpen() {
    return this.activeRightPanel === "ai";
  }
  set isAIOpen(v: boolean) {
    this.activeRightPanel = v
      ? "ai"
      : this.activeRightPanel === "ai"
        ? null
        : this.activeRightPanel;
  }

  get isPendingChangesOpen() {
    return this.activeRightPanel === "pendingChanges";
  }
  set isPendingChangesOpen(v: boolean) {
    this.activeRightPanel = v
      ? "pendingChanges"
      : this.activeRightPanel === "pendingChanges"
        ? null
        : this.activeRightPanel;
  }

  // === PENDING CHANGES STATE (per-connection) ===
  pendingChangesByConnection = $state<Record<string, PendingChange[]>>({});
  /**
   * Connections whose last apply ended without an answer (a dropped
   * request, a closed socket): some of their queued changes may already
   * have run. Cleared by the next apply that answers, or by clearing the queue.
   */
  pendingChangesInterrupted = $state<Record<string, boolean>>({});

  // === VIEW STATE ===
  activeView = $state<
    | "query"
    | "schema"
    | "explain"
    | "erd"
    | "statistics"
    | "workflow"
    | "visualize"
    | "connection"
    | "dashboard"
    | "starter"
    | "settings"
    | "createTable"
    | "data"
    | "extensionsDuckdb"
  >("query");
  /**
   * The view each project shows or was left on. `activeView` is the active
   * project's; a project's state is saved with its own entry here, so a save
   * that runs after a switch doesn't store another project's view.
   */
  activeViewByProject: Record<string, ActiveViewType> = {};

  // === PROJECT DERIVED VALUES ===

  // Derived: active project object
  activeProject = $derived(this.projects.find((p) => p.id === this.activeProjectId) || null);

  // Derived: connections for active project, ordered by connectionOrderByProject.
  // Connections whose IDs are not yet in the order array are appended at the end
  // in their natural (load/insertion) order.
  projectConnections = $derived.by(() => {
    if (!this.activeProjectId) return [];
    const filtered = this.connections.filter((c) => c.projectId === this.activeProjectId);
    const order = this.connectionOrderByProject[this.activeProjectId] ?? [];
    if (order.length === 0) return filtered;
    const indexById = new Map(order.map((id, i) => [id, i]));
    const known: DatabaseConnection[] = [];
    const unknown: DatabaseConnection[] = [];
    for (const c of filtered) {
      if (indexById.has(c.id)) known.push(c);
      else unknown.push(c);
    }
    known.sort((a, b) => indexById.get(a.id)! - indexById.get(b.id)!);
    return [...known, ...unknown];
  });

  // === CONNECTION DERIVED VALUES ===

  // Derived: active connection ID for current project
  activeConnectionId = $derived(
    this.activeProjectId ? (this.activeConnectionIdByProject[this.activeProjectId] ?? null) : null,
  );

  // Derived: active connection object
  activeConnection = $derived(
    this.connections.find((c) => c.id === this.activeConnectionId) || null,
  );

  // Derived: schema for active connection
  activeSchema = $derived(
    this.activeConnectionId ? (this.schemas[this.activeConnectionId] ?? []) : [],
  );

  // === PENDING CHANGES DERIVED VALUES ===

  /**
   * The connection whose pending changes the sheet and the header badge
   * show: the one the user is looking at. That's the focused data tab's
   * connection, or the focused query result's, which can differ from the
   * active one in the sidebar; otherwise the active connection.
   */
  pendingConnectionId = $derived.by((): string | null => {
    if (this.activeView === "data" && this.activeDataTab) return this.activeDataTab.connectionId;
    if (this.activeView === "query" && this.activeQueryResult?.connectionId) {
      return this.activeQueryResult.connectionId;
    }
    return this.activeConnectionId;
  });

  activePendingChanges = $derived(
    this.pendingConnectionId
      ? (this.pendingChangesByConnection[this.pendingConnectionId] ?? [])
      : [],
  );

  activePendingChangesCount = $derived(this.activePendingChanges.length);

  // === QUERY TAB DERIVED VALUES ===

  // Derived: query tabs for active project
  queryTabs = $derived(
    this.activeProjectId ? (this.queryTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active query tab ID for active project
  activeQueryTabId = $derived(
    this.activeProjectId ? (this.activeQueryTabIdByProject[this.activeProjectId] ?? null) : null,
  );

  // Derived: active query tab object
  activeQueryTab = $derived(this.queryTabs.find((t) => t.id === this.activeQueryTabId) || null);

  // Derived: active query result (for multi-statement support)
  activeQueryResult = $derived(
    this.activeQueryTab?.results?.[this.activeQueryTab.activeResultIndex ?? 0] || null,
  );

  // === SCHEMA TAB DERIVED VALUES ===

  // Derived: schema tabs for active project
  schemaTabs = $derived(
    this.activeProjectId ? (this.schemaTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active schema tab ID for active project
  activeSchemaTabId = $derived(
    this.activeProjectId ? (this.activeSchemaTabIdByProject[this.activeProjectId] ?? null) : null,
  );

  // Derived: active schema tab object
  activeSchemaTab = $derived(this.schemaTabs.find((t) => t.id === this.activeSchemaTabId) || null);

  // === EXPLAIN TAB DERIVED VALUES ===

  // Derived: explain tabs for active project
  explainTabs = $derived(
    this.activeProjectId ? (this.explainTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active explain tab ID for active project
  activeExplainTabId = $derived(
    this.activeProjectId ? (this.activeExplainTabIdByProject[this.activeProjectId] ?? null) : null,
  );

  // Derived: active explain tab object
  activeExplainTab = $derived(
    this.explainTabs.find((t) => t.id === this.activeExplainTabId) || null,
  );

  // === ERD TAB DERIVED VALUES ===

  // Derived: ERD tabs for active project
  erdTabs = $derived(
    this.activeProjectId ? (this.erdTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active ERD tab ID for active project
  activeErdTabId = $derived(
    this.activeProjectId ? (this.activeErdTabIdByProject[this.activeProjectId] ?? null) : null,
  );

  // Derived: active ERD tab object
  activeErdTab = $derived(this.erdTabs.find((t) => t.id === this.activeErdTabId) || null);

  // === STATISTICS TAB DERIVED VALUES ===

  // Derived: statistics tabs for active project
  statisticsTabs = $derived(
    this.activeProjectId ? (this.statisticsTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active statistics tab ID for active project
  activeStatisticsTabId = $derived(
    this.activeProjectId
      ? (this.activeStatisticsTabIdByProject[this.activeProjectId] ?? null)
      : null,
  );

  // Derived: active statistics tab object
  activeStatisticsTab = $derived(
    this.statisticsTabs.find((t) => t.id === this.activeStatisticsTabId) || null,
  );

  // === WORKFLOW TAB DERIVED VALUES ===

  // Derived: workflow tabs for active project
  workflowTabs = $derived(
    this.activeProjectId ? (this.workflowTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active workflow tab ID for active project
  activeWorkflowTabId = $derived(
    this.activeProjectId ? (this.activeWorkflowTabIdByProject[this.activeProjectId] ?? null) : null,
  );

  // Derived: active workflow tab object
  activeWorkflowTab = $derived(
    this.workflowTabs.find((t) => t.id === this.activeWorkflowTabId) || null,
  );

  // Derived: saved workflows for active project
  savedWorkflows = $derived(
    this.activeProjectId ? (this.savedWorkflowsByProject[this.activeProjectId] ?? []) : [],
  );

  // === VISUALIZE TAB DERIVED VALUES ===

  // Derived: visualize tabs for active project
  visualizeTabs = $derived(
    this.activeProjectId ? (this.visualizeTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active visualize tab ID for active project
  activeVisualizeTabId = $derived(
    this.activeProjectId
      ? (this.activeVisualizeTabIdByProject[this.activeProjectId] ?? null)
      : null,
  );

  // Derived: active visualize tab object
  activeVisualizeTab = $derived(
    this.visualizeTabs.find((t) => t.id === this.activeVisualizeTabId) || null,
  );

  // === CONNECTION TAB DERIVED VALUES ===

  // Derived: connection tabs for active project
  connectionTabs = $derived(
    this.activeProjectId ? (this.connectionTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active connection tab ID for active project
  activeConnectionTabId = $derived(
    this.activeProjectId
      ? (this.activeConnectionTabIdByProject[this.activeProjectId] ?? null)
      : null,
  );

  // Derived: active connection tab object
  activeConnectionTab = $derived(
    this.connectionTabs.find((t) => t.id === this.activeConnectionTabId) || null,
  );

  // === DASHBOARD TAB DERIVED VALUES ===

  // Derived: dashboard tabs for active project
  dashboardTabs = $derived(
    this.activeProjectId ? (this.dashboardTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active dashboard tab ID for active project
  activeDashboardTabId = $derived(
    this.activeProjectId
      ? (this.activeDashboardTabIdByProject[this.activeProjectId] ?? null)
      : null,
  );

  // Derived: active dashboard tab object
  activeDashboardTab = $derived(
    this.dashboardTabs.find((t) => t.id === this.activeDashboardTabId) || null,
  );

  // Derived: dashboards for active project
  projectDashboards = $derived(
    this.activeProjectId ? (this.dashboardsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: local (non-shared) dashboards for active project
  projectLocalDashboards = $derived(this.projectDashboards.filter((d) => !d.shared));

  // Derived: shared dashboards for active project
  projectSharedDashboards = $derived(this.projectDashboards.filter((d) => d.shared));

  // === STARTER TAB DERIVED VALUES ===

  // Derived: starter tabs for active project
  starterTabs = $derived(
    this.activeProjectId ? (this.starterTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active starter tab ID for active project
  activeStarterTabId = $derived(
    this.activeProjectId ? (this.activeStarterTabIdByProject[this.activeProjectId] ?? null) : null,
  );

  // Derived: active starter tab object
  activeStarterTab = $derived(
    this.starterTabs.find((t) => t.id === this.activeStarterTabId) || null,
  );

  // === SETTINGS TAB DERIVED VALUES ===

  // Derived: settings tabs for active project
  settingsTabs = $derived(
    this.activeProjectId ? (this.settingsTabsByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: active settings tab ID for active project
  activeSettingsTabId = $derived(
    this.activeProjectId ? (this.activeSettingsTabIdByProject[this.activeProjectId] ?? null) : null,
  );

  // Derived: active settings tab object
  activeSettingsTab = $derived(
    this.settingsTabs.find((t) => t.id === this.activeSettingsTabId) || null,
  );

  // === CREATE TABLE TAB DERIVED VALUES ===

  createTableTabs = $derived(
    this.activeProjectId ? (this.createTableTabsByProject[this.activeProjectId] ?? []) : [],
  );

  activeCreateTableTabId = $derived(
    this.activeProjectId
      ? (this.activeCreateTableTabIdByProject[this.activeProjectId] ?? null)
      : null,
  );

  activeCreateTableTab = $derived(
    this.createTableTabs.find((t) => t.id === this.activeCreateTableTabId) || null,
  );

  // === DATA TAB DERIVED VALUES ===

  dataTabs = $derived(
    this.activeProjectId ? (this.dataTabsByProject[this.activeProjectId] ?? []) : [],
  );

  activeDataTabId = $derived(
    this.activeProjectId ? (this.activeDataTabIdByProject[this.activeProjectId] ?? null) : null,
  );

  activeDataTab = $derived(this.dataTabs.find((t) => t.id === this.activeDataTabId) || null);

  // === DUCKDB EXTENSIONS TAB DERIVED VALUES ===

  extensionsDuckdbTabs = $derived(
    this.activeProjectId ? (this.extensionsDuckdbTabsByProject[this.activeProjectId] ?? []) : [],
  );

  activeExtensionsDuckdbTabId = $derived(
    this.activeProjectId
      ? (this.activeExtensionsDuckdbTabIdByProject[this.activeProjectId] ?? null)
      : null,
  );

  activeExtensionsDuckdbTab = $derived(
    this.extensionsDuckdbTabs.find((t) => t.id === this.activeExtensionsDuckdbTabId) || null,
  );

  // === TAB TYPE UTILITIES ===

  /** Get the active tab ID for a given view type */
  getActiveTabId(type: ActiveViewType): string | null {
    switch (type) {
      case "query":
        return this.activeQueryTabId;
      case "schema":
        return this.activeSchemaTabId;
      case "explain":
        return this.activeExplainTabId;
      case "erd":
        return this.activeErdTabId;
      case "statistics":
        return this.activeStatisticsTabId;
      case "workflow":
        return this.activeWorkflowTabId;
      case "visualize":
        return this.activeVisualizeTabId;
      case "connection":
        return this.activeConnectionTabId;
      case "dashboard":
        return this.activeDashboardTabId;
      case "starter":
        return this.activeStarterTabId;
      case "settings":
        return this.activeSettingsTabId;
      case "createTable":
        return this.activeCreateTableTabId;
      case "data":
        return this.activeDataTabId;
      case "extensionsDuckdb":
        return this.activeExtensionsDuckdbTabId;
      default:
        return null;
    }
  }

  // === QUERY DATA DERIVED VALUES ===

  // Derived: query history for active connection
  activeConnectionQueryHistory = $derived(
    this.activeConnectionId ? (this.queryHistoryByConnection[this.activeConnectionId] ?? []) : [],
  );

  // Derived: all queries for active project
  projectQueries = $derived(
    this.activeProjectId ? (this.queriesByProject[this.activeProjectId] ?? []) : [],
  );

  // Derived: local (non-shared) queries for active project
  projectLocalQueries = $derived(this.projectQueries.filter((q) => !q.shared));

  // Derived: shared queries for active project
  projectSharedQueries = $derived(this.projectQueries.filter((q) => q.shared));

  // === PROJECT GIT DERIVED VALUES ===

  // Derived: sync state for active project's git directory
  activeProjectSyncState = $derived(
    this.activeProjectId ? (this.projectGitSyncState[this.activeProjectId] ?? null) : null,
  );

  // Derived: whether active project has a git directory configured
  activeProjectHasGit = $derived(!!this.activeProject?.gitRepoPath);

  // === SHARED QUERY LIBRARY DERIVED VALUES ===

  // Derived: active shared query repo object
  activeRepo = $derived(this.sharedRepos.find((r) => r.id === this.activeRepoId) || null);

  // Derived: shared dashboards for active repo
  activeRepoDashboards = $derived(
    this.activeRepoId ? (this.sharedDashboardsByRepo[this.activeRepoId] ?? []) : [],
  );

  // Derived: sync state for active repo
  activeRepoSyncState = $derived(
    this.activeRepoId ? (this.syncStateByRepo[this.activeRepoId] ?? null) : null,
  );

  // Derived: all shared queries across all projects (for search)
  allSharedQueries = $derived(
    Object.values(this.queriesByProject)
      .flat()
      .filter((q) => q.shared),
  );

  // Derived: all shared dashboards across all projects (for search)
  allSharedDashboards = $derived(
    Object.values(this.dashboardsByProject)
      .flat()
      .filter((d) => d.shared),
  );

  // === SHARED CONFIG DERIVED VALUES ===

  // Derived: shared labels for active repo
  activeRepoSharedLabels = $derived(
    this.activeRepoId ? (this.sharedLabelsByRepo[this.activeRepoId] ?? []) : [],
  );

  // Derived: shared projects for active repo
  activeRepoSharedProjects = $derived(
    this.activeRepoId ? (this.sharedProjectsByRepo[this.activeRepoId] ?? []) : [],
  );

  // Derived: all shared projects across all repos
  allSharedProjects = $derived(Object.values(this.sharedProjectsByRepo).flat());

  // Derived: all shared connections across all projects
  allSharedConnections = $derived(Object.values(this.sharedConnectionsByProject).flat());

  // Derived: all shared labels across all repos
  allSharedLabels = $derived(Object.values(this.sharedLabelsByRepo).flat());

  // === AI DERIVED VALUES ===

  // Derived: chats for active connection
  activeConnectionAIChats = $derived(
    this.activeConnectionId ? (this.aiChatsByConnection[this.activeConnectionId] ?? []) : [],
  );

  // Derived: active chat ID for active connection
  activeAIChatId = $derived(
    this.activeConnectionId
      ? (this.activeAIChatIdByConnection[this.activeConnectionId] ?? null)
      : null,
  );

  // Derived: active chat object
  activeAIChat = $derived(
    this.activeConnectionAIChats.find((c) => c.id === this.activeAIChatId) || null,
  );

  // Derived: messages for active chat (same name as old flat array for minimal UI changes)
  aiMessages = $derived(
    this.activeAIChatId ? (this.aiMessagesByChat[this.activeAIChatId] ?? []) : [],
  );
}
