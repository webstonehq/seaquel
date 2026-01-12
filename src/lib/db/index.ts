import type { DatabaseType, SchemaTable, SchemaColumn, SchemaIndex, TableSizeInfo, IndexUsageInfo, DatabaseOverview } from "$lib/types";
import { MssqlAdapter } from "./mssql";
import { PostgresAdapter } from "./postgres";
import { SqliteAdapter } from "./sqlite";
import { DuckDBAdapter } from "./duckdb";

export interface ExplainNode {
	type: string;
	label: string;
	cost?: number;
	rows?: number;
	actualTime?: number;
	actualRows?: number;
	children?: ExplainNode[];
}

export interface DatabaseAdapter {
	/** SQL query to list all tables in the database */
	getSchemaQuery(): string;

	/** SQL query to get column metadata for a table */
	getColumnsQuery(table: string, schema: string): string;

	/** SQL query to get index information for a table */
	getIndexesQuery(table: string, schema: string): string;

	/** SQL query to get foreign key information for a table (optional, some DBs include in columns query) */
	getForeignKeysQuery?(table: string, schema: string): string;

	/** Build the EXPLAIN query for this database type */
	getExplainQuery(query: string, analyze: boolean): string;

	/** Parse EXPLAIN results into a common format */
	parseExplainResult(rows: unknown[], analyze: boolean): ExplainNode;

	/** Transform raw schema query results to SchemaTable[] */
	parseSchemaResult(rows: unknown[]): SchemaTable[];

	/** Transform raw columns query results to SchemaColumn[] */
	parseColumnsResult(rows: unknown[], foreignKeys?: unknown[]): SchemaColumn[];

	/** Transform raw indexes query results to SchemaIndex[] */
	parseIndexesResult(rows: unknown[]): SchemaIndex[];

	// === STATISTICS METHODS (optional) ===

	/** SQL query to get table sizes */
	getTableSizesQuery?(): string;

	/** SQL query to get index usage statistics */
	getIndexUsageQuery?(): string;

	/** SQL query to get database overview statistics */
	getDatabaseOverviewQuery?(): string;

	/** Parse table sizes query results */
	parseTableSizesResult?(rows: unknown[]): TableSizeInfo[];

	/** Parse index usage query results */
	parseIndexUsageResult?(rows: unknown[]): IndexUsageInfo[];

	/** Parse database overview query results */
	parseDatabaseOverviewResult?(rows: unknown[]): DatabaseOverview;

	/** SQL query to get row count for a specific table (for DBs that need per-table queries) */
	getTableRowCountQuery?(table: string, schema: string): string;
}

/**
 * Validates and sanitizes a SQL identifier (table name, schema name, column name).
 * Throws an error if the identifier contains invalid characters.
 * Allows Unicode letters/digits for international table names.
 */
export function validateIdentifier(name: string): string {
	// Allow alphanumeric, underscore, and common international characters
	// Also allow dollar sign which PostgreSQL supports
	if (!/^[\p{L}\p{N}_$][\p{L}\p{N}_$]*$/u.test(name)) {
		throw new Error(`Invalid SQL identifier: "${name}"`);
	}
	return name;
}

const adapters: Partial<Record<DatabaseType, DatabaseAdapter>> = {
	mssql: new MssqlAdapter(),
	postgres: new PostgresAdapter(),
	sqlite: new SqliteAdapter(),
	duckdb: new DuckDBAdapter(),
};

export function getAdapter(type: DatabaseType): DatabaseAdapter {
	const adapter = adapters[type];
	if (!adapter) {
		throw new Error(`Database type "${type}" is not supported yet`);
	}
	return adapter;
}
