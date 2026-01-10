/**
 * DuckDB-WASM database provider.
 * Runs an in-browser DuckDB instance for the web demo.
 */

import type { DatabaseProvider, ConnectionConfig, ExecuteResult } from './types';

// DuckDB-WASM types - dynamically imported
type AsyncDuckDB = import('@duckdb/duckdb-wasm').AsyncDuckDB;
type AsyncDuckDBConnection = import('@duckdb/duckdb-wasm').AsyncDuckDBConnection;

/**
 * Database provider that uses DuckDB-WASM.
 * Provides an in-browser SQL database for the demo.
 */
export class DuckDBProvider implements DatabaseProvider {
	readonly id = 'duckdb';

	private db: AsyncDuckDB | null = null;
	private connections = new Map<string, AsyncDuckDBConnection>();
	private initialized = false;
	private initPromise: Promise<void> | null = null;

	isAvailable(): boolean {
		// Available in browser when not in Tauri
		return typeof window !== 'undefined' && !('__TAURI__' in window);
	}

	/**
	 * Initialize DuckDB-WASM. Called once before first connection.
	 */
	private async initialize(): Promise<void> {
		if (this.initialized) return;
		if (this.initPromise) return this.initPromise;

		this.initPromise = this.doInitialize();
		await this.initPromise;
	}

	private async doInitialize(): Promise<void> {
		const duckdb = await import('@duckdb/duckdb-wasm');

		// Use jsDelivr CDN for WASM bundles
		const JSDELIVR_BUNDLES = duckdb.getJsDelivrBundles();
		const bundle = await duckdb.selectBundle(JSDELIVR_BUNDLES);

		// Create worker
		const workerUrl = URL.createObjectURL(
			new Blob([`importScripts("${bundle.mainWorker}");`], { type: 'text/javascript' })
		);
		const worker = new Worker(workerUrl);
		const logger = new duckdb.ConsoleLogger();

		// Instantiate database
		this.db = new duckdb.AsyncDuckDB(logger, worker);
		await this.db.instantiate(bundle.mainModule, bundle.pthreadWorker);

		this.initialized = true;
	}

	async connect(config: ConnectionConfig): Promise<string> {
		await this.initialize();

		if (!this.db) {
			throw new Error('DuckDB not initialized');
		}

		const connectionId = `duckdb-${Date.now()}`;
		const conn = await this.db.connect();
		this.connections.set(connectionId, conn);

		return connectionId;
	}

	async disconnect(connectionId: string): Promise<void> {
		const conn = this.connections.get(connectionId);
		if (conn) {
			await conn.close();
			this.connections.delete(connectionId);
		}
	}

	async select<T = Record<string, unknown>>(connectionId: string, sql: string, _params?: unknown[]): Promise<T[]> {
		const conn = this.connections.get(connectionId);
		if (!conn) {
			throw new Error(`Connection not found: ${connectionId}`);
		}

		// Note: DuckDB-WASM doesn't support parameterized queries in the same way as other providers.
		// For parameterized queries, the substituteParameters utility handles MSSQL inline,
		// and for DuckDB we use substituted SQL with positional params already resolved.
		const result = await conn.query(sql);
		// eslint-disable-next-line @typescript-eslint/no-explicit-any
		return result.toArray().map((row: any) => row.toJSON() as T);
	}

	async execute(connectionId: string, sql: string, _params?: unknown[]): Promise<ExecuteResult> {
		const conn = this.connections.get(connectionId);
		if (!conn) {
			throw new Error(`Connection not found: ${connectionId}`);
		}

		// DuckDB-WASM doesn't support parameterized queries in the same way
		// For the demo, we execute the SQL directly
		const result = await conn.query(sql);

		return {
			rowsAffected: result.numRows
		};
	}

	async test(_config: ConnectionConfig): Promise<void> {
		await this.initialize();

		if (!this.db) {
			throw new Error('DuckDB not initialized');
		}

		// Test by creating and closing a connection
		const conn = await this.db.connect();
		await conn.close();
	}

	/**
	 * Execute raw SQL on a connection.
	 * Useful for loading sample data with multiple statements.
	 */
	async executeRaw(connectionId: string, sql: string): Promise<void> {
		const conn = this.connections.get(connectionId);
		if (!conn) {
			throw new Error(`Connection not found: ${connectionId}`);
		}
		await conn.query(sql);
	}

	/**
	 * Get the underlying DuckDB instance.
	 * Used for advanced operations like loading Parquet files.
	 */
	getDb(): AsyncDuckDB | null {
		return this.db;
	}

	/**
	 * Get the underlying connection.
	 * Used for advanced operations.
	 */
	getConnection(connectionId: string): AsyncDuckDBConnection | undefined {
		return this.connections.get(connectionId);
	}
}
