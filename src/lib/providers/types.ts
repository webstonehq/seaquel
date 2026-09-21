/**
 * Database provider abstraction layer.
 * Enables the same codebase to work with Tauri (desktop) and DuckDB-WASM (web).
 */

import type { DatabaseType } from "$lib/types";

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
  /** SQLite only: create the database file if it doesn't exist */
  createIfMissing?: boolean;
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
   * @param params Optional parameterized query values
   * @returns Array of result rows
   */
  select<T = Record<string, unknown>>(
    connectionId: string,
    sql: string,
    params?: unknown[],
  ): Promise<T[]>;

  /**
   * Execute a SELECT query and stream rows in batches as they become available.
   * Use this for unbounded or large result sets where the caller wants to
   * render rows incrementally instead of waiting for the whole result.
   *
   * The `onBatch` callback is invoked for each batch delivered from the
   * backend. Returning `false` (or aborting `signal`) cancels the stream —
   * the backend will stop fetching rows and release its database connection.
   *
   * @param connectionId Connection ID from connect()
   * @param sql SQL query to execute
   * @param params Optional parameterized query values
   * @param onBatch Called for each batch. Return false to cancel.
   * @param signal Optional AbortSignal to cancel the stream.
   * @returns Summary with `aborted` flag and optional terminal error message.
   */
  selectStream(
    connectionId: string,
    sql: string,
    params: unknown[] | undefined,
    /**
     * Invoked once per incoming batch. `columns` is non-null ONLY on the
     * first batch (and on the terminal batch when the result set is empty);
     * later batches carry `null` — callers must capture the first
     * non-null value and reuse it for the rest of the stream.
     *
     * `rows` is columnar: `rows[i][j]` is the value in column `columns[j]`.
     */
    onBatch: (batch: {
      columns: string[] | null;
      rows: unknown[][];
      isFinal: boolean;
    }) => boolean | Promise<boolean>,
    signal?: AbortSignal,
  ): Promise<{ aborted: boolean; error?: string }>;

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
