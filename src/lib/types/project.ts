/**
 * Project and connection label types.
 * Projects group multiple database connections together.
 * @module types/project
 */

/**
 * Predefined connection label identifiers.
 */
export type PredefinedLabel = "local" | "staging" | "prod";

/**
 * A label that can be assigned to connections.
 * Labels help identify connection environments (local, staging, prod) or custom categories.
 */
export interface ConnectionLabel {
  /** Unique identifier for the label */
  id: string;
  /** Display name */
  name: string;
  /** Whether this is a predefined label (local/staging/prod) */
  isPredefined: boolean;
  /** Hex color code (e.g., "#22c55e") */
  color: string;
}

/**
 * Predefined labels with their default colors.
 * - Local: Green (#22c55e)
 * - Staging: Amber (#f59e0b)
 * - Production: Red (#ef4444)
 */
export const PREDEFINED_LABELS: Record<PredefinedLabel, ConnectionLabel> = {
  local: { id: "local", name: "Local", isPredefined: true, color: "#22c55e" },
  staging: { id: "staging", name: "Staging", isPredefined: true, color: "#f59e0b" },
  prod: { id: "prod", name: "Production", isPredefined: true, color: "#ef4444" },
};

/**
 * Default project ID used for backwards compatibility.
 * Existing connections are migrated to this project.
 */
export const DEFAULT_PROJECT_ID = "default-seaquel";

/**
 * Default project name.
 */
export const DEFAULT_PROJECT_NAME = "Seaquel";

/**
 * Represents a project containing database connections.
 * Projects help organize connections into logical groups (e.g., by application or team).
 */
export interface Project {
  /** Unique identifier */
  id: string;
  /** User-friendly display name */
  name: string;
  /** Optional description */
  description?: string;
  /** When the project was created */
  createdAt: Date;
  /** When the project was last modified */
  updatedAt: Date;
  /** Custom labels defined for this project (in addition to predefined labels) */
  customLabels: ConnectionLabel[];
  /** Optional path to a Git directory for team sharing */
  gitRepoPath?: string;
}

/**
 * Persisted project data.
 * Uses ISO string dates for JSON serialization.
 */
export interface PersistedProject {
  /** Unique identifier */
  id: string;
  /** User-friendly display name */
  name: string;
  /** Optional description */
  description?: string;
  /** When the project was created (ISO 8601 string) */
  createdAt: string;
  /** When the project was last modified (ISO 8601 string) */
  updatedAt: string;
  /** Custom labels defined for this project */
  customLabels: ConnectionLabel[];
  /** Optional path to a Git directory for team sharing */
  gitRepoPath?: string;
}

/**
 * Persisted project state for tabs and UI.
 * This replaces PersistedConnectionState as tabs are now per-project.
 */
export interface PersistedProjectState {
  /** Project identifier */
  projectId: string;
  /** Query editor tabs */
  queryTabs: import("./persisted").PersistedQueryTab[];
  /** Schema browser tabs */
  schemaTabs: import("./persisted").PersistedSchemaTab[];
  /** EXPLAIN viewer tabs */
  explainTabs: import("./persisted").PersistedExplainTab[];
  /** ERD viewer tabs */
  erdTabs: import("./persisted").PersistedErdTab[];
  /** Statistics dashboard tabs */
  statisticsTabs?: import("./persisted").PersistedStatisticsTab[];
  /** Workflow tabs */
  workflowTabs?: import("./persisted").PersistedWorkflowTab[];
  /** Ordered list of all tab IDs for drag-drop ordering (`null` when stored JSON `null`) */
  tabOrder: string[] | null;
  /** Ordered list of connection IDs for drag-drop ordering (per project) */
  connectionOrder?: string[] | null;
  /** Currently active query tab */
  activeQueryTabId: string | null;
  /** Currently active schema tab */
  activeSchemaTabId: string | null;
  /** Currently active explain tab */
  activeExplainTabId: string | null;
  /** Currently active ERD tab */
  activeErdTabId: string | null;
  /** Currently active statistics tab */
  activeStatisticsTabId?: string | null;
  /** Currently active workflow tab */
  activeWorkflowTabId?: string | null;
  /** Which view type is currently active */
  activeView: import("./persisted").ActiveViewType;
  /** Active connection ID within this project */
  activeConnectionId: string | null;
  /** Starter tabs shown when no connection is active */
  starterTabs?: import("./persisted").PersistedStarterTab[];
  /** Currently active starter tab */
  activeStarterTabId?: string | null;
  /** Saved workflows for this project */
  savedWorkflows?: import("./workflow").SavedWorkflow[];
  /** Connection tabs (transient, not restored) */
  connectionTabs?: import("./persisted").PersistedConnectionTab[];
  /** Active connection tab ID (transient, not restored) */
  activeConnectionTabId?: string | null;
  /** Dashboard tabs */
  dashboardTabs?: import("./persisted").PersistedDashboardTab[];
  /** Currently active dashboard tab */
  activeDashboardTabId?: string | null;
  /** IDs of shared queries that are starred */
  starredSharedQueryIds?: string[] | null;
  /** IDs of shared dashboards that are starred */
  starredSharedDashboardIds?: string[] | null;
  /** Create table tabs */
  createTableTabs?: import("./persisted").PersistedCreateTableTab[];
  /** Currently active create table tab */
  activeCreateTableTabId?: string | null;
  /** Data viewer tabs */
  dataTabs?: import("./persisted").PersistedDataTab[];
  /** Currently active data tab */
  activeDataTabId?: string | null;
  /** DuckDB extensions tabs */
  extensionsDuckdbTabs?: { id: string; name: string; connectionId: string }[];
  /** Currently active DuckDB extensions tab */
  activeExtensionsDuckdbTabId?: string | null;
  /** Split pane layout */
  paneLayout?: {
    panes: { id: string; tabIds: string[]; activeTabId: string | null }[];
    activePaneId: string;
  } | null;
}
