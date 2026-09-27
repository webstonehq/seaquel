/**
 * DuckDB-WASM database provider.
 * Runs an in-browser DuckDB instance for the web demo.
 */

import type { DatabaseProvider, ConnectionConfig, ExecuteResult, ReadOnlyRows } from "./types";
import { dedupeColumnNames } from "$lib/utils/row-access";
import { queryCancelled } from "./wire";
import { absoluteUrl, duckdbBundles, startWithin } from "./duckdb-bundles";

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
 * An Arrow result as row objects. Duplicate column names (e.g.
 * `SELECT a.id, b.id FROM a JOIN b`) are deduped as `id`, `id_2`: Arrow's
 * `toJSON()` iterates by field name and would silently overwrite them.
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function tableRows(result: any): Record<string, unknown>[] {
  const fieldNames: string[] = result.schema?.fields?.map((f: { name: string }) => f.name) ?? [];
  const hasDupes = fieldNames.length > 0 && new Set(fieldNames).size !== fieldNames.length;
  if (!hasDupes) {
    // Fast path — Arrow's JSON serializer handles this correctly and preserves
    // its own type coercions (BigInt→string, timestamp formatting, etc.).
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    return result.toArray().map((row: any) => row.toJSON() as Record<string, unknown>);
  }
  const columns = dedupeColumnNames(fieldNames);
  const vectors = columns.map((_, i) => result.getChildAt(i));
  const numRows = Number(result.numRows);
  return Array.from({ length: numRows }, (_, r) => {
    const obj: Record<string, unknown> = {};
    for (let c = 0; c < columns.length; c++) {
      obj[columns[c]] = vectors[c]?.get(r) ?? null;
    }
    return obj;
  });
}

/** DuckDB's own read-only refusals, reported with code `READ_ONLY` as native DuckDB does. */
const READ_ONLY_REFUSALS = [
  "Expected a single SELECT statement",
  "transaction is launched in read-only mode",
];

/** A read-only query's error as `"CODE: message"`, as the Rust transports send it. */
function readOnlyError(error: unknown): Error {
  const message = error instanceof Error ? error.message : String(error);
  const code = READ_ONLY_REFUSALS.some((refusal) => message.includes(refusal))
    ? "READ_ONLY"
    : "QUERY_ERROR";
  return new Error(`${code}: ${message}`);
}

/**
 * The demo's read-only query (the AI's `run_query` and dashboard widgets),
 * the same mechanism as native DuckDB: a fresh connection, a read-only
 * transaction, and the SQL bound as the one parameter of
 * `SELECT * FROM query(?)`, with `LIMIT limit` when given (a number the
 * caller chose, never user SQL). `query()` parses with DuckDB's own parser and
 * runs exactly one SELECT (also `WITH`, `FROM t`, `VALUES`, `DESCRIBE`,
 * `SHOW`); anything else, and any second statement, is refused. That also
 * stops `COMMIT; INSERT …` from ending the transaction, and the `COPY`,
 * `ATTACH`, `INSTALL`, `SET` and `EXPORT` a read-only transaction allows.
 * The connection is rolled back and closed, so nothing outlives the call.
 *
 * Accepted gap (plan, Decision 1): `read_csv('https://…')` and other file or
 * URL reads are SELECTs, so they run; in the browser that's a `fetch`.
 *
 * `query()` names duplicate columns itself (`a`, `a_1`), so they come back
 * that way rather than as `select`'s `a`, `a_2`.
 *
 * Cancelling: an aborted `signal` rejects at once with an `AbortError`.
 * DuckDB-WASM 1.32 runs a prepared statement to completion inside its
 * worker (`cancelSent` only interrupts `conn.send`, which takes no
 * parameters), so the query itself finishes in the background before the
 * connection is rolled back and closed. The demo's data is small.
 */
export async function selectReadOnlyOn(
  db: AsyncDuckDB,
  sql: string,
  signal?: AbortSignal,
  limit?: number,
): Promise<Record<string, unknown>[]> {
  if (signal?.aborted) throw queryCancelled();
  if (limit !== undefined && !(Number.isSafeInteger(limit) && limit >= 0)) {
    throw new Error(`QUERY_ERROR: invalid row limit ${limit}`);
  }
  const wrapper =
    limit === undefined ? "SELECT * FROM query(?)" : `SELECT * FROM query(?) LIMIT ${limit}`;
  const run = (async () => {
    const conn = await db.connect();
    try {
      await conn.query("BEGIN TRANSACTION READ ONLY");
      try {
        const statement = await conn.prepare(wrapper);
        try {
          return tableRows(await statement.query(sql));
        } finally {
          await statement.close();
        }
      } finally {
        // Nothing ran that could write; closing the connection below would
        // roll back too. A failure here mustn't hide the query's own error.
        await conn.query("ROLLBACK").catch(() => {});
      }
    } catch (error) {
      throw readOnlyError(error);
    } finally {
      // Like ROLLBACK above: a failed close mustn't hide the query's result.
      await conn.close().catch((error: unknown) => {
        console.warn("[DuckDB] closing a read-only connection failed:", error);
      });
    }
  })();
  if (!signal) return run;
  return new Promise((resolve, reject) => {
    const onAbort = () => reject(queryCancelled());
    signal.addEventListener("abort", onAbort, { once: true });
    run.then(resolve, reject).finally(() => signal.removeEventListener("abort", onAbort));
  });
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
    if (!this.initPromise) {
      // A failed start is forgotten, so the next call tries again.
      this.initPromise = this.doInitialize().catch((error: unknown) => {
        this.initPromise = null;
        throw error;
      });
    }
    await this.initPromise;
  }

  private async doInitialize(): Promise<void> {
    const duckdb = await import("@duckdb/duckdb-wasm");

    // Web serves its own copy; the demo uses jsDelivr (see duckdbBundles).
    const bundle = await duckdb.selectBundle(await duckdbBundles(duckdb.getJsDelivrBundles));

    // Create worker
    const workerUrl = URL.createObjectURL(
      new Blob([`importScripts("${absoluteUrl(bundle.mainWorker!)}");`], {
        type: "text/javascript",
      }),
    );
    const worker = new Worker(workerUrl);
    const logger = new duckdb.ConsoleLogger();

    // Instantiate database. A worker that can't load its script never
    // answers, so give up on its error event or after a timeout.
    const db = new duckdb.AsyncDuckDB(logger, worker);
    try {
      await startWithin(
        db.instantiate(
          absoluteUrl(bundle.mainModule),
          bundle.pthreadWorker ? absoluteUrl(bundle.pthreadWorker) : undefined,
        ),
        worker,
      );
    } catch (error) {
      worker.terminate();
      throw error;
    } finally {
      URL.revokeObjectURL(workerUrl);
    }

    this.db = db;
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
    return tableRows(await conn.query(sql)) as T[];
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

  /**
   * See {@link selectReadOnlyOn}; `connectionId` must be open. With
   * `maxRows`, it fetches one row more (`LIMIT maxRows + 1`) to tell a
   * truncated result, as native DuckDB does. The demo has no row cap
   * without it.
   */
  async selectReadOnly(
    connectionId: string,
    sql: string,
    signal?: AbortSignal,
    maxRows?: number,
  ): Promise<ReadOnlyRows> {
    if (!this.connections.has(connectionId) || !this.db) {
      throw new Error(`Connection not found: ${connectionId}`);
    }
    if (maxRows === undefined) {
      return { rows: await selectReadOnlyOn(this.db, sql, signal), truncated: false };
    }
    if (!(Number.isSafeInteger(maxRows) && maxRows >= 0)) {
      throw new Error(`QUERY_ERROR: invalid row limit ${maxRows}`);
    }
    const rows = await selectReadOnlyOn(this.db, sql, signal, maxRows + 1);
    return { rows: rows.slice(0, maxRows), truncated: rows.length > maxRows };
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
