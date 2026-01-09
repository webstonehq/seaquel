/**
 * Query execution and results types.
 * @module types/query
 */

import type { QueryType } from '../db/query-utils';

/**
 * Source table information for editable query results.
 * Used to identify which table a result set came from for UPDATE/DELETE operations.
 */
export interface SourceTableInfo {
	/** Schema name of the source table */
	schema: string;
	/** Table name */
	name: string;
	/** Primary key column names for row identification */
	primaryKeys: string[];
}

/**
 * Result of a query execution with pagination support.
 */
export interface QueryResult {
	/** Column names in the result set */
	columns: string[];
	/** Row data as key-value objects */
	rows: Record<string, unknown>[];
	/** Number of rows in the current page */
	rowCount: number;
	/** Total number of rows matching the query */
	totalRows: number;
	/** Query execution time in milliseconds */
	executionTime: number;
	/** Number of rows affected (for INSERT/UPDATE/DELETE) */
	affectedRows?: number;
	/** Last inserted ID (for INSERT with auto-increment) */
	lastInsertId?: number;
	/** Type of query that was executed */
	queryType?: QueryType;
	/** Source table info for editable results */
	sourceTable?: SourceTableInfo;
	/** Current page number (1-indexed) */
	page: number;
	/** Number of rows per page */
	pageSize: number;
	/** Total number of pages */
	totalPages: number;
}

/**
 * Result of a single SQL statement within a multi-statement query.
 * Extends QueryResult with statement-specific metadata.
 */
export interface StatementResult extends QueryResult {
	/** Index of this statement in the batch (0-indexed) */
	statementIndex: number;
	/** The SQL text of this specific statement */
	statementSql: string;
	/** Error message if this statement failed */
	error?: string;
	/** Whether this statement resulted in an error */
	isError: boolean;
}

/**
 * Represents an open query editor tab.
 */
export interface QueryTab {
	/** Unique tab identifier */
	id: string;
	/** Tab display name */
	name: string;
	/** SQL query text in the editor */
	query: string;
	/** Results from executing the query (one per statement) */
	results?: StatementResult[];
	/** Index of the currently displayed result (for multi-statement queries) */
	activeResultIndex?: number;
	/** Whether a query is currently executing */
	isExecuting: boolean;
	/** ID of the saved query this tab was loaded from, if any */
	savedQueryId?: string;
}

/**
 * An entry in the query execution history.
 */
export interface QueryHistoryItem {
	/** Unique identifier */
	id: string;
	/** The executed SQL query */
	query: string;
	/** When the query was executed */
	timestamp: Date;
	/** Execution time in milliseconds */
	executionTime: number;
	/** Number of rows returned or affected */
	rowCount: number;
	/** ID of the connection this query was run on */
	connectionId: string;
	/** Whether this query is marked as a favorite */
	favorite: boolean;
}

/**
 * A saved query for quick access.
 */
export interface SavedQuery {
	/** Unique identifier */
	id: string;
	/** User-defined name for the query */
	name: string;
	/** The SQL query text */
	query: string;
	/** ID of the connection this query belongs to */
	connectionId: string;
	/** When the query was first saved */
	createdAt: Date;
	/** When the query was last modified */
	updatedAt: Date;
}

/**
 * A message in the AI assistant conversation.
 */
export interface AIMessage {
	/** Unique identifier */
	id: string;
	/** Who sent the message */
	role: 'user' | 'assistant';
	/** Message content */
	content: string;
	/** When the message was sent */
	timestamp: Date;
	/** SQL query suggested or discussed, if any */
	query?: string;
}
