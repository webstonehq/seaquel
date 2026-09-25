/**
 * EXPLAIN/ANALYZE query plan types.
 *
 * The wire types are generated from `crates/seaquel-types` (see `./generated`);
 * edit them there and run `npm run types:gen`.
 * @module types/explain
 */

import type { ExplainResult } from "./generated/ExplainResult";

export type { ExplainPlanNode } from "./generated/ExplainPlanNode";
export type { ExplainResult } from "./generated/ExplainResult";

/**
 * Represents an open EXPLAIN plan viewer tab.
 */
export interface ExplainTab {
  /** Unique tab identifier */
  id: string;
  /** Tab display name */
  name: string;
  /** The original query that was explained */
  sourceQuery: string;
  /** The explain result, if available */
  result?: ExplainResult;
  /** Whether the explain is currently running */
  isExecuting: boolean;
}
