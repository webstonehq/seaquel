import type { DatabaseType, SchemaTable, SchemaColumn, SchemaIndex } from "$lib/types";
import { PostgresAdapter } from "./postgres";
import { SqliteAdapter } from "./sqlite";

export interface DatabaseAdapter {
	/** SQL query to list all tables in the database */
	getSchemaQuery(): string;

	/** SQL query to get column metadata for a table */
	getColumnsQuery(table: string, schema: string): string;

	/** SQL query to get index information for a table */
	getIndexesQuery(table: string, schema: string): string;

	/** SQL query to get foreign key information for a table (optional, some DBs include in columns query) */
	getForeignKeysQuery?(table: string, schema: string): string;

	/** Transform raw schema query results to SchemaTable[] */
	parseSchemaResult(rows: unknown[]): SchemaTable[];

	/** Transform raw columns query results to SchemaColumn[] */
	parseColumnsResult(rows: unknown[], foreignKeys?: unknown[]): SchemaColumn[];

	/** Transform raw indexes query results to SchemaIndex[] */
	parseIndexesResult(rows: unknown[]): SchemaIndex[];
}

const adapters: Partial<Record<DatabaseType, DatabaseAdapter>> = {
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
