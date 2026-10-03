/**
 * Database provider abstraction layer: the interface the managers use.
 * `CoreProvider` implements it over Seaquel Core (desktop and web);
 * `DuckDBProvider` over DuckDB-WASM (the demo and the browser tutorial).
 */

import type { ConnectParams } from "$lib/types/generated/ConnectParams";

/**
 * What `connect` and `test` take: Core's `db.connect` params, a target
 * (`{type:"saved",id}` or `{type:"form",form}`) plus the secrets the caller
 * holds, a trusted SSH host key after the prompt, and SQLite's
 * `createIfMissing`.
 */
export type ConnectRequest = ConnectParams;

/**
 * Result of an execute operation (INSERT, UPDATE, DELETE).
 */
export interface ExecuteResult {
  /** Number of rows affected by the operation */
  rowsAffected: number;
  /** ID of the last inserted row (if applicable) */
  lastInsertId?: number;
}

/** What `DatabaseProvider.selectReadOnly` resolves with. */
export interface ReadOnlyRows {
  /** Row objects, column names deduped as `select` does (`id`, `id_2`). */
  rows: Record<string, unknown>[];
  /**
   * The query had more rows than the `maxRows` it was run with, and only
   * the first `maxRows` came back. Always false without `maxRows`.
   */
  truncated: boolean;
}

/**
 * Unified interface for database operations.
 * Implementations handle the specifics of each backend (Core, DuckDB-WASM).
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
   * @param request What to connect, and the secrets for it
   * @returns Connection ID for subsequent operations
   */
  connect(request: ConnectRequest): Promise<string>;

  /**
   * Close a database connection.
   * @param connectionId Connection ID from connect()
   */
  disconnect(connectionId: string): Promise<void>;

  /**
   * Record that a connection opened before its saved row existed (`add`)
   * is that row's (phase 6 Task 7, `db.bindSaved`), so the assistant runs
   * on it. Core only; others have no assistant.
   */
  bindSaved?(connectionId: string, savedConnectionId: string): Promise<void>;

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
   * Run one query in the read-only mode the database enforces: the AI's
   * `run_query` tool and dashboard widgets. Callers go through
   * `QueryCrud.executeReadOnly`, which checks the connection and runs the
   * token check first; nothing else calls this.
   *
   * The backend runs the same token check, then the engine's
   * `query_read_only` (Core's `query_stream` with `read_only`). Exactly one
   * statement runs, and the database refuses writes.
   *
   * @param connectionId Provider connection ID from connect()
   * @param sql One read-only statement. No bind parameters.
   * @param signal Aborting it cancels the query (`db.cancel` with its stream
   *   id) and rejects the promise.
   * @param maxRows Return at most this many rows, with `truncated` set when
   *   the query had more (Core's `max_rows`). Without it, a result past the
   *   engine's row cap fails with `RESULT_TOO_LARGE`.
   * @returns The rows, and whether `maxRows` cut them short.
   * @throws Error with the stream's error message, e.g. the database's
   *   read-only refusal (code `READ_ONLY`).
   */
  selectReadOnly(
    connectionId: string,
    sql: string,
    signal?: AbortSignal,
    maxRows?: number,
  ): Promise<ReadOnlyRows>;

  /**
   * Execute a write query (INSERT, UPDATE, DELETE).
   * @param connectionId Connection ID from connect()
   * @param sql SQL query to execute
   * @param params Optional parameterized query values
   * @returns Execute result with rowsAffected
   */
  execute(connectionId: string, sql: string, params?: unknown[]): Promise<ExecuteResult>;

  /**
   * Test a connection without keeping it open.
   * @param request What to connect, and the secrets for it
   * @throws Error if connection fails
   */
  test(request: ConnectRequest): Promise<void>;
}
