import type { DatabaseType, SchemaTable, SchemaColumn, SchemaIndex } from "$lib/types";
import { MssqlAdapter } from "./mssql";
import { PostgresAdapter } from "./postgres";
import { SqliteAdapter } from "./sqlite";

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
};

export function getAdapter(type: DatabaseType): DatabaseAdapter {
	const adapter = adapters[type];
	if (!adapter) {
		throw new Error(`Database type "${type}" is not supported yet`);
	}
	return adapter;
}
