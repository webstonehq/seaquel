import type { DatabaseType } from "$lib/types";

/**
 * DBeaver connection configuration as stored in data-sources.json
 */
export interface DbeaverConnectionConfig {
	host?: string;
	port?: string;
	database?: string;
	user?: string;
	url?: string;
}

/**
 * DBeaver connection entry from data-sources.json
 */
export interface DbeaverConnection {
	id: string;
	provider: string;
	driver: string;
	name: string;
	configuration: DbeaverConnectionConfig;
}

/**
 * Root structure of DBeaver's data-sources.json file
 */
export interface DbeaverDataSources {
	folders?: Record<string, unknown>;
	connections: Record<string, Omit<DbeaverConnection, "id">>;
}

/**
 * A DBeaver connection that has been processed and is ready for import
 */
export interface ImportableConnection {
	original: DbeaverConnection;
	name: string;
	type: DatabaseType;
	host: string;
	port: number;
	databaseName: string;
	username: string;
	isDuplicate: boolean;
	selected: boolean;
}
