import { invoke } from "@tauri-apps/api/core";
import type { DatabaseType } from "$lib/types";
import type {
	DbeaverDataSources,
	DbeaverConnection,
	ImportableConnection,
} from "$lib/types/dbeaver";

/**
 * Maps DBeaver provider names to Seaquel database types
 */
const PROVIDER_MAP: Record<string, DatabaseType> = {
	postgresql: "postgres",
	postgres: "postgres",
	mysql: "mysql",
	mariadb: "mariadb",
	sqlite: "sqlite",
	mongodb: "mongodb",
	mssql: "mssql",
	sqlserver: "mssql",
};

/**
 * Default ports for each database type
 */
const DEFAULT_PORTS: Record<DatabaseType, number> = {
	postgres: 5432,
	mysql: 3306,
	mariadb: 3306,
	sqlite: 0,
	mongodb: 27017,
	mssql: 1433,
	duckdb: 0,
};

/**
 * Reads and parses DBeaver's data-sources.json using a Rust command
 * Returns an array of connections or empty array if not found
 */
export async function parseDbeaverConnections(): Promise<DbeaverConnection[]> {
	try {
		const content = await invoke<string | null>("read_dbeaver_config");

		if (!content) {
			return [];
		}

		const data = JSON.parse(content) as DbeaverDataSources;

		if (!data.connections) {
			return [];
		}

		return Object.entries(data.connections).map(([id, conn]) => ({
			...conn,
			id,
		}));
	} catch (error) {
		console.error("Failed to read DBeaver config:", error);
		return [];
	}
}

/**
 * Maps a DBeaver connection to an importable connection format
 * Returns null if the database type is not supported
 */
export function mapToImportable(
	dbeaverConn: DbeaverConnection,
	existingConnectionIds: string[]
): ImportableConnection | null {
	const type = PROVIDER_MAP[dbeaverConn.provider?.toLowerCase()];
	if (!type) {
		return null; // Unsupported database type
	}

	const config = dbeaverConn.configuration || {};
	const host = config.host || "localhost";
	const port = parseInt(config.port || String(DEFAULT_PORTS[type]), 10);
	const databaseName = config.database || "";
	const username = config.user || "";

	// Generate the connection ID that Seaquel would use
	const expectedId =
		type === "sqlite"
			? `conn-sqlite-${databaseName}`
			: `conn-${host}-${port}`;

	const isDuplicate = existingConnectionIds.includes(expectedId);

	return {
		original: dbeaverConn,
		name: dbeaverConn.name,
		type,
		host,
		port,
		databaseName,
		username,
		isDuplicate,
		selected: !isDuplicate, // Pre-select non-duplicates
	};
}

/**
 * Discovers and parses all DBeaver connections, filtering for supported types
 */
export async function discoverDbeaverConnections(
	existingConnectionIds: string[]
): Promise<ImportableConnection[]> {
	const dbeaverConnections = await parseDbeaverConnections();

	return dbeaverConnections
		.map((conn) => mapToImportable(conn, existingConnectionIds))
		.filter((conn): conn is ImportableConnection => conn !== null);
}
