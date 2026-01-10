/**
 * Persisted state types for storing data across app restarts.
 * These types use ISO string dates instead of Date objects for JSON serialization.
 * @module types/persisted
 */

/**
 * Persisted query tab state.
 * Stores query content but not execution results.
 */
export interface PersistedQueryTab {
	/** Tab identifier */
	id: string;
	/** Tab display name */
	name: string;
	/** SQL query text */
	query: string;
	/** ID of the saved query this tab was loaded from */
	savedQueryId?: string;
}

/**
 * Persisted schema tab state.
 * Stores reference to the viewed table.
 */
export interface PersistedSchemaTab {
	/** Tab identifier */
	id: string;
	/** Name of the table being viewed */
	tableName: string;
	/** Schema name of the table */
	schemaName: string;
}

/**
 * Persisted EXPLAIN tab state.
 */
export interface PersistedExplainTab {
	/** Tab identifier */
	id: string;
	/** Tab display name */
	name: string;
	/** The original query that was explained */
	sourceQuery: string;
}

/**
 * Persisted ERD tab state.
 */
export interface PersistedErdTab {
	/** Tab identifier */
	id: string;
	/** Tab display name */
	name: string;
}

/**
 * Persisted query parameter definition.
 */
export interface PersistedQueryParameter {
	/** Parameter name */
	name: string;
	/** Data type */
	type: 'text' | 'number' | 'date' | 'datetime' | 'boolean';
	/** Optional default value */
	defaultValue?: string;
	/** Description/label */
	description?: string;
}

/**
 * Persisted saved query.
 * Uses ISO strings for dates.
 */
export interface PersistedSavedQuery {
	/** Query identifier */
	id: string;
	/** User-defined name */
	name: string;
	/** SQL query text */
	query: string;
	/** Connection this query belongs to */
	connectionId: string;
	/** When first saved (ISO 8601 string) */
	createdAt: string;
	/** When last modified (ISO 8601 string) */
	updatedAt: string;
	/** Optional parameter definitions */
	parameters?: PersistedQueryParameter[];
}

/**
 * Persisted query history entry.
 * Uses ISO strings for dates.
 */
export interface PersistedQueryHistoryItem {
	/** Entry identifier */
	id: string;
	/** The executed SQL query */
	query: string;
	/** When executed (ISO 8601 string) */
	timestamp: string;
	/** Execution time in milliseconds */
	executionTime: number;
	/** Number of rows returned or affected */
	rowCount: number;
	/** Connection this query was run on */
	connectionId: string;
	/** Whether marked as favorite */
	favorite: boolean;
}

/**
 * View type options for the main workspace.
 */
export type ActiveViewType = 'query' | 'schema' | 'explain' | 'erd';

/**
 * Complete persisted state for a single connection.
 * Stores all tabs, history, and UI state.
 */
export interface PersistedConnectionState {
	/** Connection identifier */
	connectionId: string;
	/** Query editor tabs */
	queryTabs: PersistedQueryTab[];
	/** Schema browser tabs */
	schemaTabs: PersistedSchemaTab[];
	/** EXPLAIN viewer tabs */
	explainTabs: PersistedExplainTab[];
	/** ERD viewer tabs */
	erdTabs: PersistedErdTab[];
	/** Ordered list of all tab IDs for drag-drop ordering */
	tabOrder: string[];
	/** Currently active query tab */
	activeQueryTabId: string | null;
	/** Currently active schema tab */
	activeSchemaTabId: string | null;
	/** Currently active explain tab */
	activeExplainTabId: string | null;
	/** Currently active ERD tab */
	activeErdTabId: string | null;
	/** Which view type is currently active */
	activeView: ActiveViewType;
	/** Saved queries for this connection */
	savedQueries: PersistedSavedQuery[];
	/** Query execution history */
	queryHistory: PersistedQueryHistoryItem[];
}
