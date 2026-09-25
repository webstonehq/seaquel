/**
 * DuckDB-WASM database provider.
 * Runs an in-browser DuckDB instance for the web demo.
 */

import type { DatabaseProvider, ConnectionConfig, ExecuteResult } from "./types";
import { dedupeColumnNames } from "$lib/utils/row-access";

// DuckDB-WASM types - dynamically imported
type AsyncDuckDB = import("@duckdb/duckdb-wasm").AsyncDuckDB;
type AsyncDuckDBConnection = import("@duckdb/duckdb-wasm").AsyncDuckDBConnection;

/** The parts of an Arrow result `rowsAffected` reads. */
interface CountResult {
  numRows: number;
  schema: { fields: { name: string }[] };
  getChildAt(index: number): { get(index: number): unknown } | null;
}

/**
 * The rows an INSERT, UPDATE or DELETE affected. DuckDB answers them with one
 * row in one column, `Count`; `numRows` is the size of that answer (always 1),
 * which would hide an UPDATE that matched nothing. Anything else (DDL) keeps
 * reporting `numRows`.
 */
export function rowsAffected(result: CountResult): number {
  const fields = result.schema.fields;
  if (fields.length === 1 && fields[0].name === "Count" && result.numRows === 1) {
    const count = result.getChildAt(0)?.get(0);
    if (typeof count === "bigint" || typeof count === "number") return Number(count);
  }
  return result.numRows;
}

/**
 * Database provider that uses DuckDB-WASM.
 * Provides an in-browser SQL database for the demo.
 */
export class DuckDBProvider implements DatabaseProvider {
  readonly id = "duckdb";

  private db: AsyncDuckDB | null = null;
  private connections = new Map<string, AsyncDuckDBConnection>();
  private initialized = false;
  private initPromise: Promise<void> | null = null;

  isAvailable(): boolean {
    // Available in browser when not in Tauri
    return typeof window !== "undefined" && !("__TAURI__" in window);
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
    const duckdb = await import("@duckdb/duckdb-wasm");

    // Use jsDelivr CDN for WASM bundles
    const JSDELIVR_BUNDLES = duckdb.getJsDelivrBundles();
    const bundle = await duckdb.selectBundle(JSDELIVR_BUNDLES);

    // Create worker
    const workerUrl = URL.createObjectURL(
      new Blob([`importScripts("${bundle.mainWorker}");`], { type: "text/javascript" }),
    );
    const worker = new Worker(workerUrl);
    const logger = new duckdb.ConsoleLogger();

    // Instantiate database
    this.db = new duckdb.AsyncDuckDB(logger, worker);
    await this.db.instantiate(bundle.mainModule, bundle.pthreadWorker);

    this.initialized = true;
  }

  async connect(_config: ConnectionConfig): Promise<string> {
    await this.initialize();

    if (!this.db) {
      throw new Error("DuckDB not initialized");
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

  async select<T = Record<string, unknown>>(
    connectionId: string,
    sql: string,
    _params?: unknown[],
  ): Promise<T[]> {
    const conn = this.connections.get(connectionId);
    if (!conn) {
      throw new Error(`Connection not found: ${connectionId}`);
    }

    // Note: DuckDB-WASM doesn't support parameterized queries in the same way as other providers.
    // For parameterized queries, the substituteParameters utility handles MSSQL inline,
    // and for DuckDB we use substituted SQL with positional params already resolved.
    const result = await conn.query(sql);
    // Detect duplicate column names (e.g. `SELECT a.id, b.id FROM a JOIN b`)
    // before building row objects. Arrow's `toJSON()` iterates by field name
    // and silently overwrites when names collide; the positional fallback
    // below preserves every value under a suffixed name (`id`, `id_2`, …).
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const fieldNames: string[] =
      (result as any).schema?.fields?.map((f: { name: string }) => f.name) ?? [];
    const hasDupes = fieldNames.length > 0 && new Set(fieldNames).size !== fieldNames.length;
    if (!hasDupes) {
      // Fast path — Arrow's JSON serializer handles this correctly and preserves
      // its own type coercions (BigInt→string, timestamp formatting, etc.).
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      return result.toArray().map((row: any) => row.toJSON() as T);
    }
    const columns = dedupeColumnNames(fieldNames);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const vectors = columns.map((_, i) => (result as any).getChildAt(i));
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const numRows = Number((result as any).numRows);
    return Array.from({ length: numRows }, (_, r) => {
      const obj: Record<string, unknown> = {};
      for (let c = 0; c < columns.length; c++) {
        obj[columns[c]] = vectors[c]?.get(r) ?? null;
      }
      return obj as T;
    });
  }

  async selectStream(
    connectionId: string,
    sql: string,
    params: unknown[] | undefined,
    onBatch: (batch: {
      columns: string[] | null;
      rows: unknown[][];
      isFinal: boolean;
    }) => boolean | Promise<boolean>,
    signal?: AbortSignal,
  ): Promise<{ aborted: boolean; error?: string }> {
    // DuckDB-WASM doesn't surface a row-by-row iterator through its arrow
    // result wrapper, so we take the whole result and emit a single terminal
    // batch. The demo datasets are small enough that this is fine.
    try {
      const rowObjects = await this.select<Record<string, unknown>>(connectionId, sql, params);
      if (signal?.aborted) return { aborted: true };
      const columns = rowObjects.length > 0 ? Object.keys(rowObjects[0]) : [];
      const rows: unknown[][] = rowObjects.map((row) => columns.map((c) => row[c]));
      await onBatch({ columns, rows, isFinal: true });
      return { aborted: false };
    } catch (error) {
      return {
        aborted: false,
        error: error instanceof Error ? error.message : String(error),
      };
    }
  }

  async execute(connectionId: string, sql: string, _params?: unknown[]): Promise<ExecuteResult> {
    const conn = this.connections.get(connectionId);
    if (!conn) {
      throw new Error(`Connection not found: ${connectionId}`);
    }

    // DuckDB-WASM doesn't support parameterized queries in the same way
    // For the demo, we execute the SQL directly
    const result = await conn.query(sql);

    return { rowsAffected: rowsAffected(result) };
  }

  async test(_config: ConnectionConfig): Promise<void> {
    await this.initialize();

    if (!this.db) {
      throw new Error("DuckDB not initialized");
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
