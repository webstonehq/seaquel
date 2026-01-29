/**
 * DuckDB provider for Tauri desktop app.
 * Uses custom Rust backend with DuckDB crate.
 */

import {
	duckdbConnect,
	duckdbDisconnect,
	duckdbQuery,
	duckdbExecute,
	duckdbTest,
	type DuckDBConnectResult,
	type DuckDBQueryResult,
	type DuckDBExecuteResult
} from '$lib/api/tauri';
import type { DatabaseProvider, ConnectionConfig, ExecuteResult } from './types';

/**
 * DuckDB provider implementation for Tauri (desktop).
 * Calls Rust backend commands for DuckDB operations.
 */
export class DuckDBTauriProvider implements DatabaseProvider {
	readonly id = 'duckdb-tauri';

	/**
	 * Check if Tauri API is available.
	 */
	isAvailable(): boolean {
		return typeof window !== 'undefined' && '__TAURI__' in window;
	}

	/**
	 * Connect to a DuckDB database.
	 * @param config Connection configuration with databaseName as file path or ":memory:"
	 */
	async connect(config: ConnectionConfig): Promise<string> {
		// Extract path from connection string or use databaseName
		let path = config.databaseName || ':memory:';

		if (config.connectionString) {
			path = config.connectionString
				.replace(/^duckdb:\/\//, '')
				.replace(/^duckdb:/, '') || ':memory:';
		}

		const result = await duckdbConnect(path);
		return result.connection_id;
	}

	/**
	 * Disconnect from a DuckDB database.
	 */
	async disconnect(connectionId: string): Promise<void> {
		await duckdbDisconnect(connectionId);
	}

	/**
	 * Execute a SELECT query and return results.
	 */
	async select<T = Record<string, unknown>>(
		connectionId: string,
		sql: string,
		_params?: unknown[]
	): Promise<T[]> {
		const result = await duckdbQuery(connectionId, sql);

		// Convert array rows to objects using column names
		return result.rows.map((row) => {
			const obj: Record<string, unknown> = {};
			result.columns.forEach((col, i) => {
				obj[col] = row[i];
			});
			return obj as T;
		});
	}

	/**
	 * Execute a write query (INSERT, UPDATE, DELETE, CREATE, etc.).
	 */
	async execute(
		connectionId: string,
		sql: string,
		_params?: unknown[]
	): Promise<ExecuteResult> {
		const result = await duckdbExecute(connectionId, sql);

		return {
			rowsAffected: result.rows_affected,
		};
	}

	/**
	 * Test a DuckDB connection by opening and immediately closing it.
	 */
	async test(config: ConnectionConfig): Promise<void> {
		let path = config.databaseName || ':memory:';

		if (config.connectionString) {
			path = config.connectionString
				.replace(/^duckdb:\/\//, '')
				.replace(/^duckdb:/, '') || ':memory:';
		}

		await duckdbTest(path);
	}
}
