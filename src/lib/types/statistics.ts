/**
 * Database statistics and dashboard types.
 *
 * The wire types are generated from `crates/seaquel-types` (see `./generated`);
 * edit them there and run `npm run types:gen`.
 * @module types/statistics
 */

import type { DatabaseStatistics } from "./generated/DatabaseStatistics";

export type { TableSizeInfo } from "./generated/TableSizeInfo";
export type { IndexUsageInfo } from "./generated/IndexUsageInfo";
export type { DatabaseOverview } from "./generated/DatabaseOverview";
export type { DatabaseStatistics } from "./generated/DatabaseStatistics";

/**
 * Represents an open statistics dashboard tab.
 */
export interface StatisticsTab {
  /** Unique tab identifier */
  id: string;
  /** Tab display name */
  name: string;
  /** ID of the connection this tab shows statistics for */
  connectionId: string;
  /** Loaded statistics data */
  data?: DatabaseStatistics;
  /** Whether statistics are currently being loaded */
  isLoading: boolean;
  /** When the statistics were last refreshed */
  lastRefreshed?: Date;
  /** Error message if loading failed */
  error?: string;
}
