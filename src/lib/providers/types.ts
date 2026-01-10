/**
 * Database provider abstraction layer.
 * Enables the same codebase to work with Tauri (desktop) and DuckDB-WASM (web).
 */

import type { DatabaseType } from '$lib/types';

/**
 * Configuration for establishing a database connection.
 */
export interface ConnectionConfig {
	/** Database engine type */
	type: DatabaseType;
	/** Database server hostname */
	host?: string;
	/** Database server port */
	port?: number;
	/** Database name */
	databaseName: string;
	/** Username for authentication */
	username?: string;
	/** Password for authentication */
	password?: string;
	/** Full connection string (used by Tauri for PostgreSQL/SQLite) */
	connectionString?: string;
	/** SSL mode */
	sslMode?: string;
}

/**
 * Result of an execute operation (INSERT, UPDATE, DELETE).
 */
export interface ExecuteResult {
	/** Number of rows affected by the operation */
	rowsAffected: number;
	/** ID of the last inserted row (if applicable) */
	lastInsertId?: number;
}

/**
 * Unified interface for database operations.
 * Implementations handle the specifics of each backend (Tauri, DuckDB-WASM).
 */
export interface DatabaseProvider {
	/** Provider identifier */
	readonly id: string;

	/**
	 * Check if this provider is available in the current environment.
	 */
	isAvailable(): boolean;

	/**
	 * Establish a database connection.
	 * @param config Connection configuration
	 * @returns Connection ID for subsequent operations
	 */
	connect(config: ConnectionConfig): Promise<string>;

	/**
	 * Close a database connection.
	 * @param connectionId Connection ID from connect()
	 */
	disconnect(connectionId: string): Promise<void>;

	/**
	 * Execute a SELECT query and return rows.
	 * @param connectionId Connection ID from connect()
	 * @param sql SQL query to execute
	 * @returns Array of result rows
	 */
	select<T = Record<string, unknown>>(connectionId: string, sql: string): Promise<T[]>;

	/**
	 * Execute a write query (INSERT, UPDATE, DELETE).
	 * @param connectionId Connection ID from connect()
	 * @param sql SQL query to execute
	 * @param params Optional parameterized query values
	 * @returns Execute result with rowsAffected
	 */
	execute(connectionId: string, sql: string, params?: unknown[]): Promise<ExecuteResult>;

	/**
	 * Test a connection without persisting it.
	 * @param config Connection configuration
	 * @throws Error if connection fails
	 */
	test(config: ConnectionConfig): Promise<void>;
}
