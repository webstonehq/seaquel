/**
 * Re-exports all types from the types module.
 * This file maintains backward compatibility with existing imports.
 *
 * Types are now organized in src/lib/types/:
 * - database.ts: DatabaseConnection, DatabaseType, SSHTunnelConfig
 * - schema.ts: SchemaTable, SchemaColumn, SchemaIndex
 * - query.ts: QueryTab, QueryResult, QueryHistoryItem, SavedQuery
 * - explain.ts: ExplainTab, ExplainPlanNode, ExplainResult
 * - erd.ts: ErdTab
 * - persisted.ts: All PersistedXxx types
 *
 * @module types
 */

export * from './types/index';
