/**
 * Tauri database provider.
 * Wraps tauri-plugin-sql for PostgreSQL and SQLite connections in the desktop app.
 */

import Database from '@tauri-apps/plugin-sql';
import type { DatabaseProvider, ConnectionConfig, ExecuteResult } from './types';

/**
 * Database provider that uses Tauri's SQL plugin.
 * Supports PostgreSQL and SQLite via connection strings.
 */
export class TauriDatabaseProvider implements DatabaseProvider {
	readonly id = 'tauri';

	/** Map of connection IDs to Database instances */
	private connections = new Map<string, Database>();

	isAvailable(): boolean {
		return typeof window !== 'undefined' && '__TAURI__' in window;
	}

	async connect(config: ConnectionConfig): Promise<string> {
		if (!config.connectionString) {
			throw new Error('Connection string is required for Tauri provider');
		}

		const connectionId = `tauri-${config.type}-${Date.now()}`;
		const db = await Database.load(config.connectionString);
		this.connections.set(connectionId, db);
		return connectionId;
	}

	async disconnect(connectionId: string): Promise<void> {
		const db = this.connections.get(connectionId);
		if (db) {
			await db.close();
			this.connections.delete(connectionId);
		}
	}

	async select<T = Record<string, unknown>>(connectionId: string, sql: string, params?: unknown[]): Promise<T[]> {
		const db = this.connections.get(connectionId);
		if (!db) {
			throw new Error(`Connection not found: ${connectionId}`);
		}
		return db.select(sql, params) as Promise<T[]>;
	}

	async execute(connectionId: string, sql: string, params?: unknown[]): Promise<ExecuteResult> {
		const db = this.connections.get(connectionId);
		if (!db) {
			throw new Error(`Connection not found: ${connectionId}`);
		}
		const result = await db.execute(sql, params);
		return {
			rowsAffected: result?.rowsAffected ?? 0,
			lastInsertId: result?.lastInsertId
		};
	}

	async test(config: ConnectionConfig): Promise<void> {
		if (!config.connectionString) {
			throw new Error('Connection string is required for Tauri provider');
		}
		const db = await Database.load(config.connectionString);
		await db.close();
	}

	/**
	 * Get the underlying Database instance for a connection.
	 * Used for backward compatibility during migration.
	 */
	getDatabase(connectionId: string): Database | undefined {
		return this.connections.get(connectionId);
	}
}
