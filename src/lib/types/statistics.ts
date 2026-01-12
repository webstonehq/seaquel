/**
 * Database statistics and dashboard types.
 * @module types/statistics
 */

/**
 * Information about a table's size and storage.
 */
export interface TableSizeInfo {
	/** Schema name */
	schema: string;
	/** Table name */
	name: string;
	/** Number of rows in the table */
	rowCount: number;
	/** Human-readable total size (e.g., "1.2 GB") */
	totalSize: string;
	/** Total size in bytes for sorting */
	totalSizeBytes: number;
	/** Human-readable data size */
	dataSize?: string;
	/** Human-readable index size */
	indexSize?: string;
}

/**
 * Information about index usage and performance.
 */
export interface IndexUsageInfo {
	/** Schema name */
	schema: string;
	/** Table the index belongs to */
	table: string;
	/** Index name */
	indexName: string;
	/** Human-readable index size */
	size: string;
	/** Number of index scans performed */
	scans: number;
	/** Number of rows read via this index */
	rowsRead?: number;
	/** Whether the index has never been used */
	unused: boolean;
}

/**
 * Overview statistics for the entire database.
 */
export interface DatabaseOverview {
	/** Database name */
	databaseName: string;
	/** Human-readable total database size */
	totalSize: string;
	/** Total size in bytes */
	totalSizeBytes?: number;
	/** Number of tables */
	tableCount: number;
	/** Number of indexes */
	indexCount: number;
	/** Number of active connections (if available) */
	connectionCount?: number;
}

/**
 * Complete statistics data for a database.
 */
export interface DatabaseStatistics {
	/** Overview metrics */
	overview: DatabaseOverview;
	/** Size information for each table */
	tableSizes: TableSizeInfo[];
	/** Usage information for each index */
	indexUsage: IndexUsageInfo[];
}

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
