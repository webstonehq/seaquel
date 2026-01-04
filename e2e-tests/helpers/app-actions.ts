import { homedir } from "os";
import { join } from "path";
import { existsSync, mkdirSync, writeFileSync } from "fs";

/**
 * Get the platform-specific Tauri store directory.
 */
function getStoreDir(): string {
	const home = homedir();
	const appId = "app.seaquel.desktop";

	if (process.platform === "darwin") {
		return join(home, "Library", "Application Support", appId);
	} else if (process.platform === "win32") {
		return join(home, "AppData", "Roaming", appId);
	}
	return join(home, ".local", "share", appId);
}

/**
 * Get filename with worker suffix for parallel test isolation.
 * Each worker uses unique filenames to avoid conflicts.
 */
function getStoreFilename(basename: string): string {
	const workerId = process.env.SEAQUEL_TEST_WORKER_ID;
	if (workerId) {
		const lastDot = basename.lastIndexOf(".");
		if (lastDot > 0) {
			const name = basename.substring(0, lastDot);
			const ext = basename.substring(lastDot);
			return `${name}_${workerId}${ext}`;
		}
		return `${basename}_${workerId}`;
	}
	return basename;
}

/**
 * Clear worker-specific app state.
 * Each worker clears only its own state files.
 */
export async function clearAppState(): Promise<void> {
	const storeDir = getStoreDir();
	if (!existsSync(storeDir)) {
		mkdirSync(storeDir, { recursive: true });
	}

	// Clear worker-specific connections file by writing empty state
	const connectionsFile = join(storeDir, getStoreFilename("database_connections.json"));
	writeFileSync(connectionsFile, JSON.stringify({ connections: [] }, null, 2));
}

/**
 * Connection data structure for seeding test state.
 */
export interface TestConnection {
	id: string;
	name: string;
	type: "postgres" | "mysql" | "mariadb" | "sqlite" | "mongodb" | "mssql";
	host: string;
	port: number;
	databaseName: string;
	username: string;
	sslMode?: string;
	connectionString?: string;
	lastConnected?: Date;
}

/**
 * Seed the app state with test connections.
 * Uses worker-specific filenames for parallel test isolation.
 */
export async function seedConnectionState(
	connections: TestConnection[]
): Promise<void> {
	const storeDir = getStoreDir();

	if (!existsSync(storeDir)) {
		mkdirSync(storeDir, { recursive: true });
	}

	// Prepare persisted connections
	const persistedConnections = connections.map((conn) => ({
		id: conn.id,
		name: conn.name,
		type: conn.type,
		host: conn.host,
		port: conn.port,
		databaseName: conn.databaseName,
		username: conn.username,
		sslMode: conn.sslMode || "disable",
		connectionString: conn.connectionString,
		lastConnected: conn.lastConnected?.toISOString() || new Date().toISOString(),
		sshTunnel: null,
	}));

	// Write worker-specific connections file
	const connectionsFile = join(storeDir, getStoreFilename("database_connections.json"));
	const connectionsData = { connections: persistedConnections };
	writeFileSync(connectionsFile, JSON.stringify(connectionsData, null, 2));
}

/**
 * Create a test connection object with defaults.
 */
export function createTestConnection(
	overrides: Partial<TestConnection> = {}
): TestConnection {
	return {
		id: `test-conn-${Date.now()}`,
		name: "Test PostgreSQL",
		type: "postgres",
		host: "localhost",
		port: 5432,
		databaseName: "testdb",
		username: "testuser",
		lastConnected: new Date(),
		...overrides,
	};
}
