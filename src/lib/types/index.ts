/**
 * Central type exports for Seaquel.
 * All application types are organized into domain-specific modules.
 * @module types
 */

// Project and label types
export type { PredefinedLabel, ConnectionLabel, Project, PersistedProject, PersistedProjectState } from './project';
export { PREDEFINED_LABELS, DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from './project';

// Database connection types
export type { DatabaseType, SSHAuthMethod, SSHTunnelConfig, DatabaseConnection } from './database';

// Schema types
export type {
	ForeignKeyRef,
	SchemaColumn,
	SchemaIndex,
	SchemaTable,
	SchemaTab
} from './schema';

// Query execution types
export type {
	QueryParameterType,
	QueryParameter,
	ParameterValue,
	SourceTableInfo,
	QueryResult,
	StatementResult,
	EmbeddedExplainResult,
	EmbeddedVisualizeResult,
	QueryTab,
	QueryHistoryItem,
	SavedQuery,
	AIMessage
} from './query';

// EXPLAIN types
export type { ExplainPlanNode, ExplainResult, ExplainTab } from './explain';

// ERD types
export type { ErdTab } from './erd';

// Chart types
export type { ChartType, ChartConfig, ResultViewMode } from './chart';

// Canvas types
export type { CanvasTab } from './canvas';

// Visualize types
export type {
	VisualizeTab,
	ParsedQueryVisual,
	QuerySource,
	QueryJoin,
	QueryFilter,
	QueryProjection,
	QueryOrderBy
} from './visualize';

// Statistics types
export type {
	TableSizeInfo,
	IndexUsageInfo,
	DatabaseOverview,
	DatabaseStatistics,
	StatisticsTab
} from './statistics';

// Starter tab types
export type { StarterTabType, StarterTab } from './starter-tabs';
export { DEFAULT_STARTER_TABS } from './starter-tabs';

// Persisted state types
export type {
	PersistedQueryTab,
	PersistedSchemaTab,
	PersistedExplainTab,
	PersistedErdTab,
	PersistedStatisticsTab,
	PersistedCanvasTab,
	PersistedStarterTab,
	PersistedQueryParameter,
	PersistedSavedQuery,
	PersistedQueryHistoryItem,
	PersistedConnectionState,
	ActiveViewType
} from './persisted';
