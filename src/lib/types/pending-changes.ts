/**
 * Pending changes types for queue mode.
 * @module types/pending-changes
 */

import type { Change } from "./generated/Change";
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
 *
 * `change` is what applying sends back (phase 5c, Decision 2): an edit
 * intent, which Core builds again from fresh metadata when it runs, or SQL
 * the editor deferred or the table editor generated. The other fields are
 * for display: `sql` and `bindValues` are what Core planned (or the typed
 * text), and Core classifies each change again when it applies.
 */
export interface PendingChange {
  /** Unique identifier; also `change.id` */
  id: string;
  /** Which connection this targets */
  connectionId: string;
  /** What applying sends back, in the cell wire format. */
  change: Change;
  /** The SQL statement, for display */
  sql: string;
  /** Type of query, for display */
  queryType: QueryType;
  /** Counts as DML (phase 5c, Decision 5): a batch of only these applies in one transaction. */
  dml: boolean;
  /** When it was queued */
  addedAt: Date;
  /** Human-readable summary */
  description: string;
  /** Originating query tab, if from query editor */
  sourceTabId?: string;
  /** Bind values for parameterized queries, decoded, for display */
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
  | "drop-view"
  | "truncate-table"
  /** A history row with values, queued to run again (cleanup pass B). */
  | "history";

/**
 * View mode for the pending changes panel.
 */
export type PendingChangeViewMode = "sql" | "visual";
