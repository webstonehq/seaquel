/**
 * Query execution and results types.
 * @module types/query
 */

import type { QueryType } from "./generated/QueryType";
import type { ConnectionLabel } from "./project";
import type { ExplainResult } from "./explain";
import type { ParsedQueryVisual } from "./visualize";
import type { DatabaseType } from "./database";

/**
 * Supported data types for query parameters.
 */
export type QueryParameterType = "text" | "number" | "date" | "datetime" | "boolean";

/**
 * Definition of a query parameter.
 * Used for parameterized queries with {{name}} placeholders.
 */
export interface QueryParameter {
  /** Parameter name (extracted from {{name}} placeholder) */
  name: string;
  /** Data type of the parameter */
  type: QueryParameterType;
  /** Optional default value */
  defaultValue?: string;
  /** Description/label for the parameter */
  description?: string;
}

/**
 * Runtime parameter value for query execution.
 */
export interface ParameterValue {
  /** Parameter name */
  name: string;
  /** The value entered by the user */
  value: unknown;
}

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
 * Source table + underlying column for a single output column in a query result.
 * Used to route inline cell edits to the right table for multi-table queries (JOINs),
 * where the displayed column name may differ from the underlying column name
 * (e.g. `SELECT a.id, b.id` emits columns `id` and `id_2` but both map back to `id`
 * in their respective tables).
 */
export interface ColumnSourceInfo {
  /** Schema name of the column's source table */
  schema: string;
  /** Source table name */
  table: string;
  /** Primary key column names for the source table, used to build WHERE clauses */
  primaryKeys: string[];
  /** The actual column name in the source table (independent of display alias/dedupe) */
  column: string;
}

/**
 * Result of a query execution with pagination support.
 */
export interface QueryResult {
  /** Column names in the result set */
  columns: string[];
  /** Row data as columnar arrays — `rows[i][j]` is the value in column `columns[j]`. */
  rows: unknown[][];
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
  /**
   * Per-column source info for routing inline cell edits to the correct
   * underlying table. Entries align positionally with `columns` — index `i`
   * describes column `columns[i]`. `undefined` entries mean the column is a
   * computed expression, aggregate, subquery, or otherwise not directly tied
   * to a base-table column, so it can't be edited.
   *
   * Populated only when the SELECT list was explicit enough to reason about
   * (no `*`/`t.*`, and the query AST parsed successfully). When absent,
   * callers fall back to the single `sourceTable` above.
   */
  columnSources?: (ColumnSourceInfo | undefined)[];
  /** Current page number (1-indexed) */
  page: number;
  /** Number of rows per page */
  pageSize: number;
  /** Total number of pages */
  totalPages: number;
  /** Whether this is a utility/DDL statement (SET, PRAGMA, CREATE, etc.) */
  isUtility?: boolean;
  /** True while a streaming query is still delivering rows. Cleared on final batch. */
  isStreaming?: boolean;
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
 * Embedded explain result within a query tab.
 */
export interface EmbeddedExplainResult {
  /** The explain result data */
  result: ExplainResult;
  /** The query that was explained (for staleness detection) */
  sourceQuery: string;
  /** Whether this was EXPLAIN ANALYZE vs plain EXPLAIN */
  isAnalyze: boolean;
  /** Whether the explain is currently executing */
  isExecuting: boolean;
}

/**
 * Embedded visualize result within a query tab.
 */
export interface EmbeddedVisualizeResult {
  /** Parsed query structure for visualization */
  parsedQuery: ParsedQueryVisual | null;
  /** The query that was visualized (for staleness detection) */
  sourceQuery: string;
  /** Error message if parsing failed */
  parseError?: string;
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
  /** ID of the query this tab was loaded from, if any */
  queryId?: string;
  /** Embedded explain result displayed below the editor */
  explainResult?: EmbeddedExplainResult;
  /** Embedded visualize result displayed below the editor */
  visualizeResult?: EmbeddedVisualizeResult;
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
  /** Snapshot of connection labels at execution time */
  connectionLabelsSnapshot: ConnectionLabel[];
  /** Connection name at execution time (in case it changes later) */
  connectionNameSnapshot: string;
}

/**
 * A query (local or shared). SQLite is the source of truth.
 * When shared=true, a .sql file is also maintained as a git projection.
 */
export interface Query {
  /** Unique identifier (stable across share/unshare) */
  id: string;
  /** User-defined name for the query */
  name: string;
  /** The SQL query text */
  query: string;
  /** ID of the project this query belongs to */
  projectId: string;
  /** When the query was first saved */
  createdAt: Date;
  /** When the query was last modified */
  updatedAt: Date;
  /** Optional parameter definitions for parameterized queries */
  parameters?: QueryParameter[];
  /** Whether this query is starred for quick access */
  starred?: boolean;
  /** Whether this query is shared via git */
  shared: boolean;
  /** Optional description (used in shared .sql frontmatter) */
  description?: string;
  /** Target database type (postgresql, mysql, etc.) */
  databaseType?: string;
  /** Tags for categorization */
  tags?: string[];
  /** Folder path for organization within the queries directory */
  folder?: string;
}

/** @deprecated Use Query instead */
export type SavedQuery = Query;

/**
 * A single version entry for a saved query.
 * Keyframe versions store a full snapshot; delta versions store a diff patch.
 */
export interface QueryVersion {
  /** Unique identifier */
  id: string;
  /** The query this version belongs to */
  queryId: string;
  /** Monotonically increasing version number (1-based) */
  version: number;
  /** Full query text on keyframes, null otherwise */
  snapshot: string | null;
  /** Patch text on deltas, null on keyframes */
  diff: string | null;
  /** When this version was created */
  createdAt: Date;
}

/**
 * A query version with the full query text reconstructed from snapshots and diffs.
 */
export interface ResolvedQueryVersion {
  /** Unique identifier */
  id: string;
  /** The query this version belongs to */
  queryId: string;
  /** Version number */
  version: number;
  /** Reconstructed full query text */
  query: string;
  /** When this version was created */
  createdAt: Date;
}

/**
 * An AI chat conversation belonging to a specific database connection.
 */
export interface AIChat {
  /** Unique identifier */
  id: string;
  /** The connection this chat belongs to */
  connectionId: string;
  /** Chat title (auto-generated from first message) */
  title: string;
  /** When the chat was created */
  createdAt: Date;
  /** When the chat was last active */
  updatedAt: Date;
}

/**
 * A message in the AI assistant conversation.
 */
export interface AIMessage {
  /** Unique identifier */
  id: string;
  /** The chat this message belongs to */
  chatId?: string;
  /** Who sent the message */
  role: "user" | "assistant";
  /** Message content */
  content: string;
  /** When the message was sent */
  timestamp: Date;
  /** SQL query suggested or discussed, if any */
  query?: string;
  /** Dashboard ID created or referenced during this message turn (persisted for follow-ups) */
  dashboardId?: string;
  /** Set while waiting for the user to approve an AI-requested query. Cleared once resolved. */
  pendingApproval?: {
    /** One per approval: a reply can ask for several, one after another. */
    id: string;
    query: string;
    /** The chat's connection, which the query runs on. */
    connectionName: string;
    /** Its engine, for the approval card's hint on engines with read-only gaps. */
    connectionType: DatabaseType;
    approve: () => void;
    deny: () => void;
  } | null;
  /** Set when no AI model is selected — stores the user prompt to retry once a model is chosen. */
  pendingModelSelection?: string;
}

/**
 * Interface for query execution backends.
 * Allows the query editor to work with different database backends.
 */
export interface QueryExecutor {
  /** Execute a SQL query and return rows */
  execute(sql: string): Promise<Record<string, unknown>[]>;
  /** Database type for parameter substitution style (defaults to inline) */
  dbType?: DatabaseType;
}
