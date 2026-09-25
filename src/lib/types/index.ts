/**
 * Central type exports for Seaquel.
 * All application types are organized into domain-specific modules.
 * @module types
 */

// Project and label types
export type {
  PredefinedLabel,
  ConnectionLabel,
  Project,
  PersistedProject,
  PersistedProjectState,
} from "./project";
export { PREDEFINED_LABELS, DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from "./project";

// Database connection types
export type { DatabaseType, SSHAuthMethod, SSHTunnelConfig, DatabaseConnection } from "./database";

// Schema types
export type {
  ForeignKeyRef,
  SchemaColumn,
  SchemaIndex,
  SchemaTable,
  SchemaTab,
  TableKind,
} from "./schema";

// Query execution types
export type {
  QueryParameterType,
  QueryParameter,
  ParameterValue,
  SourceTableInfo,
  ColumnSourceInfo,
  QueryResult,
  StatementResult,
  EmbeddedExplainResult,
  EmbeddedVisualizeResult,
  QueryTab,
  QueryHistoryItem,
  Query,
  SavedQuery,
  QueryVersion,
  ResolvedQueryVersion,
  AIChat,
  AIMessage,
  QueryExecutor,
} from "./query";

// EXPLAIN types
export type { ExplainPlanNode, ExplainResult, ExplainTab } from "./explain";

// ERD types
export type { ErdTab } from "./erd";

// Connection tab types
export type { ConnectionTab, ConnectionTabMode, ConnectionFormData } from "./connection-tab";

// Chart types
export type { ChartType, ChartConfig, ResultViewMode } from "./chart";

// Workflow types
export type { WorkflowTab } from "./workflow";

// Visualize types
export type {
  VisualizeTab,
  ParsedQueryVisual,
  QuerySource,
  QueryJoin,
  QueryFilter,
  QueryProjection,
  QueryOrderBy,
} from "./visualize";

// Dashboard types
export type {
  Dashboard,
  DashboardWidget,
  DashboardTab,
  KpiConfig,
  DashboardSnapshot,
  DashboardVersion,
  ResolvedDashboardVersion,
} from "./dashboard";

// Statistics types
export type {
  TableSizeInfo,
  IndexUsageInfo,
  DatabaseOverview,
  DatabaseStatistics,
  StatisticsTab,
} from "./statistics";

// DuckDB extensions types
export type { DuckDBExtension, CommunityExtension, ExtensionsDuckdbTab } from "./extensions-duckdb";

// Pane layout types
export type { Pane, PaneLayout } from "./pane";

// Starter tab types
export type { StarterTabType, StarterTab } from "./starter-tabs";
export { DEFAULT_STARTER_TABS } from "./starter-tabs";

// Settings tab types
export type { SettingsTabKind, SettingsTab } from "./settings-tab";

// Create Table types
export type {
  ColumnCategory,
  ColumnTypeInfo,
  CreateTableColumn,
  CreateTableIndex,
  CreateTableForeignKey,
  CreateTableDefinition,
  CreateTableTab,
} from "./create-table";

// Data Viewer types
export type { DataFilterOperator, DataFilter, DataSort, DataTab } from "./data-tab";

// Persisted state types
export type {
  PersistedQueryTab,
  PersistedSchemaTab,
  PersistedExplainTab,
  PersistedErdTab,
  PersistedStatisticsTab,
  PersistedWorkflowTab,
  PersistedStarterTab,
  PersistedQueryParameter,
  PersistedSavedQuery,
  PersistedQueryVersion,
  PersistedDashboardVersion,
  PersistedQueryHistoryItem,
  PersistedConnectionTab,
  PersistedConnectionState,
  PersistedDashboardTab,
  PersistedAIChat,
  PersistedAIMessage,
  PersistedCreateTableTab,
  PersistedDataTab,
  ActiveViewType,
} from "./persisted";

// Shared query library types
export type {
  RepoSyncStatus,
  SharedQueryRepo,
  SharedQuery,
  SharedDashboard,
  SyncState,
  GitCredentials,
  SyncResult,
  RepoStatus,
  ConflictContent,
  QueryFrontmatter,
  SharedQueryFolder,
  PersistedSharedQueryRepo,
  SharedLabels,
  SharedProject,
  SharedConnection,
  SharedSSHTunnelConfig,
  ConnectionOverride,
} from "./shared-queries";
export { serializeRepo, deserializeRepo, CREDENTIAL_FIELDS } from "./shared-queries";

// Pending changes types
export type {
  PendingChange,
  PendingChangeOrigin,
  PendingChangeTarget,
  PendingChangeViewMode,
} from "./pending-changes";

// AI types
export * from "./ai";

// Query builder types (interactive SELECT tutorial)
export type {
  TutorialColumn,
  TutorialTable,
  CanvasTable,
  JoinType,
  CanvasJoin,
  FilterOperator,
  FilterCondition,
  SortDirection,
  SortCondition,
  GroupByCondition,
  AggregateFunction,
  HavingOperator,
  HavingCondition,
  QueryBuilderSnapshot,
  ChallengeCriterion,
  Challenge,
  TutorialLesson,
  SelectAggregate,
  ColumnAggregate,
  DisplayAggregate,
  SubqueryRole,
  SubqueryInnerState,
  CanvasSubquery,
  CanvasCTE,
  QueryBuilderForeignKey,
  QueryBuilderColumn,
  QueryBuilderTable,
} from "./query-builder";
