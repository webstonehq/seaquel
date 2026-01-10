/**
 * Central type exports for Seaquel.
 * All application types are organized into domain-specific modules.
 * @module types
 */

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
	QueryTab,
	QueryHistoryItem,
	SavedQuery,
	AIMessage
} from './query';

// EXPLAIN types
export type { ExplainPlanNode, ExplainResult, ExplainTab } from './explain';

// ERD types
export type { ErdTab } from './erd';

// Persisted state types
export type {
	PersistedQueryTab,
	PersistedSchemaTab,
	PersistedExplainTab,
	PersistedErdTab,
	PersistedQueryParameter,
	PersistedSavedQuery,
	PersistedQueryHistoryItem,
	PersistedConnectionState,
	ActiveViewType
} from './persisted';
