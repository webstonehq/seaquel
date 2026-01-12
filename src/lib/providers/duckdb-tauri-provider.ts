/**
 * DuckDB provider for Tauri desktop app.
 * Uses custom Rust backend with DuckDB crate.
 */

import { invoke } from '@tauri-apps/api/core';
import type { DatabaseProvider, ConnectionConfig, ExecuteResult } from './types';

interface DuckDBConnectResult {
	connection_id: string;
}

interface DuckDBQueryResult {
	columns: string[];
	rows: unknown[][];
}

interface DuckDBExecuteResultRust {
	rows_affected: number;
}

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

		const result = await invoke<DuckDBConnectResult>('duckdb_connect', { path });
		return result.connection_id;
	}

	/**
	 * Disconnect from a DuckDB database.
	 */
	async disconnect(connectionId: string): Promise<void> {
		await invoke('duckdb_disconnect', { connectionId });
	}

	/**
	 * Execute a SELECT query and return results.
	 */
	async select<T = Record<string, unknown>>(
		connectionId: string,
		sql: string,
		_params?: unknown[]
	): Promise<T[]> {
		const result = await invoke<DuckDBQueryResult>('duckdb_query', {
			connectionId,
			sql,
		});

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
		const result = await invoke<DuckDBExecuteResultRust>('duckdb_execute', {
			connectionId,
			sql,
		});

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

		await invoke('duckdb_test', { path });
	}
}
