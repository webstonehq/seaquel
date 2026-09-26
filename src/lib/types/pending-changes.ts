/**
 * Pending changes types for queue mode.
 * @module types/pending-changes
 */

import type { QueryType } from "./generated/QueryType";

/**
 * Structured metadata identifying the target of a pending change.
 * Used to match changes against displayed rows/cells in the UI.
 */
export interface PendingChangeTarget {
  /** Target schema name */
  schema: string;
  /** Target table name */
  table: string;
  /** Target column (for cell updates) */
  column?: string;
  /** Primary key values identifying the target row */
  primaryKeyValues?: Record<string, unknown>;
  /** The new value being set (for cell updates) */
  newValue?: unknown;
  /** Column→value map for INSERT operations */
  insertValues?: Record<string, unknown>;
}

/**
 * A single queued database mutation awaiting review and execution.
 */
export interface PendingChange {
  /** Unique identifier */
  id: string;
  /** Which connection this targets */
  connectionId: string;
  /** The constructed SQL statement */
  sql: string;
  /** Type of query */
  queryType: QueryType;
  /** When it was queued */
  addedAt: Date;
  /** Human-readable summary */
  description: string;
  /** Originating query tab, if from query editor */
  sourceTabId?: string;
  /** Bind values for parameterized queries */
  bindValues?: unknown[];
  /** Where this change originated */
  origin: PendingChangeOrigin;
  /** Structured target for UI matching */
  target?: PendingChangeTarget;
}

/**
 * Origin of a pending change, indicating where the mutation was triggered from.
 */
export type PendingChangeOrigin =
  | "query-editor"
  | "inline-edit"
  | "insert-row"
  | "delete-row"
  | "set-default"
  | "create-table"
  | "alter-table"
  | "drop-table"
  | "truncate-table";

/**
 * View mode for the pending changes panel.
 */
export type PendingChangeViewMode = "sql" | "visual";
