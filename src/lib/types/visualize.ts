/**
 * Query Visualizer types.
 * @module types/visualize
 */
import type { ParsedQueryVisual } from "./generated/ParsedQueryVisual";

// The parsed query's shape comes from `seaquel-sql` (Rust), which parses it
// for the Visual tab through `$lib/sql`.
export type { ParsedQueryVisual } from "./generated/ParsedQueryVisual";
export type { QuerySource } from "./generated/QuerySource";
export type { QueryJoin } from "./generated/QueryJoin";
export type { QueryFilter } from "./generated/QueryFilter";
export type { QueryProjection } from "./generated/QueryProjection";
export type { QueryOrderBy } from "./generated/QueryOrderBy";

/**
 * Represents an open query visualizer tab.
 */
export interface VisualizeTab {
  /** Unique tab identifier */
  id: string;
  /** Tab display name */
  name: string;
  /** The original SQL query being visualized */
  sourceQuery: string;
  /** Parsed query structure for visualization */
  parsedQuery: ParsedQueryVisual | null;
  /** Error message if parsing failed */
  parseError?: string;
}
