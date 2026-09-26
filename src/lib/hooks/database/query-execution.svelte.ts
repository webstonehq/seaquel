import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
import type {
  DatabaseConnection,
  DatabaseType,
  QueryResult,
  StatementResult,
  ParameterValue,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import {
  countQuery,
  detectQueryType,
  detectQueryTypeOrThrow,
  extractTableFromSelect,
  hasRowLimit,
  resolveColumnSources,
  splitSqlStatementsOrThrow,
  substituteParameters,
  type ParsedStatement,
  type QueryType,
} from "$lib/sql";
import { m } from "$lib/paraglide/messages.js";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseProvider } from "$lib/providers/types";
import { extractErrorMessage } from "$lib/errors";
import { log } from "$lib/utils/logger";
import { resolveQueryOrThrow } from "./resolve-query.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import type { PendingChangeOrigin } from "$lib/types";
import { getEngineClient } from "$lib/engine";
import { QueryCrudManager } from "./query-crud.svelte.js";
import { dedupeColumnNames, rowToObject } from "$lib/utils/row-access";

/**
 * Manages query execution, pagination, and CRUD operations.
 */
export class QueryExecutionManager {
  private readonly DEFAULT_PAGE_SIZE = 100;
  readonly crud: QueryCrudManager;

  /**
   * Per-tab AbortControllers for in-flight streaming queries.
   * Starting a new query or paging on a tab aborts the previous stream,
   * and explicit user cancellation goes through `cancelStream(tabId)`.
   */
  private streamControllers = new Map<string, AbortController>();

  constructor(
    private state: DatabaseState,
    private queryHistory: QueryHistoryManager,
    private providers: ProviderRegistry,
    private pendingChanges: PendingChangesManager,
  ) {
    this.crud = new QueryCrudManager(state, providers, pendingChanges);
  }

  /**
   * Look up a StatementResult *through* the Svelte 5 `$state` proxy so that
   * mutations on the returned reference are tracked and fire reactivity.
   * Streaming paths must use this helper — never mutate a raw seed reference,
   * because Svelte's proxy only tracks writes that go through it.
   */
  private getProxiedResult(tabId: string, statementIndex: number): StatementResult | undefined {
    const projectId = this.state.activeProjectId;
    if (!projectId) return undefined;
    const tabs = this.state.queryTabsByProject[projectId];
    const tab = tabs?.find((t) => t.id === tabId);
    return tab?.results?.[statementIndex];
  }

  /** Abort any in-flight stream on the given tab and drop its controller. */
  private abortTabStream(tabId: string): void {
    const controller = this.streamControllers.get(tabId);
    if (controller) {
      controller.abort();
      this.streamControllers.delete(tabId);
    }
  }

  /** Public cancel — used by the UI's cancel button next to the row counter. */
  cancelStream(tabId: string): void {
    this.abortTabStream(tabId);
    // Also mark any currently-streaming result as no longer streaming so the
    // UI flips out of the spinner state immediately. Results read through
    // state are proxied — mutating them fires reactivity automatically.
    if (!this.state.activeProjectId) return;
    const tabs = this.state.queryTabsByProject[this.state.activeProjectId];
    const tab = tabs?.find((t) => t.id === tabId);
    if (!tab?.results) return;
    for (const r of tab.results) {
      if (r.isStreaming) {
        r.isStreaming = false;
      }
    }
  }

  /**
   * Check whether a SELECT statement should go through the streaming path.
   * We stream when the app would otherwise ship an unbounded result in one
   * shot — either because the user picked "Stream all" (pageSize === 0) or
   * because the query carries its own LIMIT/OFFSET/TOP so the paginator
   * would leave it untouched.
   */
  private shouldStream(baseQuery: string, connectionType: DatabaseType, pageSize: number): boolean {
    if (pageSize === 0) return true;
    return hasRowLimit(baseQuery, connectionType);
  }

  /**
   * Resolve the source table for a SELECT query, if any — used to enable
   * inline cell editing in the result viewer.
   */
  private resolveSourceTable(
    baseQuery: string,
    connection: DatabaseConnection,
  ): QueryResult["sourceTable"] | undefined {
    const tableInfo = extractTableFromSelect(baseQuery, connection.type);
    if (!tableInfo) return undefined;

    if (tableInfo.schema) {
      const primaryKeys = this.getPrimaryKeysForTable(
        tableInfo.schema,
        tableInfo.table,
        connection.id,
      );
      if (primaryKeys.length > 0) {
        return {
          schema: tableInfo.schema,
          name: tableInfo.table,
          primaryKeys,
        };
      }
      return undefined;
    }

    // No schema specified in query — search all cached schemas for the table
    const tables = this.state.schemas[connection.id] ?? [];
    const match = tables.find((t) => t.name === tableInfo.table);
    if (!match) return undefined;
    const primaryKeys = match.columns.filter((c) => c.isPrimaryKey).map((c) => c.name);
    if (primaryKeys.length === 0) return undefined;
    return {
      schema: match.schema,
      name: match.name,
      primaryKeys,
    };
  }

  /**
   * Resolve per-column source info via the SQL AST. Used by inline cell
   * editing to route each edit to the underlying column's own table even when
   * the query is a JOIN that renames duplicate columns to `id_2` etc. Returns
   * undefined when the query isn't an explicit SELECT we can reason about
   * (bare `*`, subqueries, unparseable SQL) — callers fall back to the single
   * `sourceTable` in that case.
   */
  private resolveColumnSources(
    baseQuery: string,
    connection: DatabaseConnection,
  ): QueryResult["columnSources"] | undefined {
    const schemas = this.state.schemas[connection.id] ?? [];
    return resolveColumnSources(baseQuery, connection.type, schemas);
  }

  /**
   * Run a streaming SELECT and mutate the StatementResult at
   * `tab.results[statementIndex]` as batches arrive. The seed must already
   * be installed at that index via `updateQueryTabState` before calling
   * this method — mutations go through the Svelte 5 `$state` proxy returned
   * by `getProxiedResult`, not through the caller's raw seed reference.
   *
   * Returns a summary of how the stream terminated: `aborted` is true if
   * the user cancelled, `error` is set if sqlx or the command errored,
   * and both are false/undefined on a clean run-to-completion.
   */
  private async runStreamingStatement(
    tabId: string,
    statementIndex: number,
    sql: string,
    bindValues: unknown[] | undefined,
    connection: DatabaseConnection,
    cachedProvider?: DatabaseProvider,
  ): Promise<{ aborted: boolean; error?: string }> {
    const providerConnectionId = connection.providerConnectionId;
    if (!providerConnectionId) throw new Error("No connection established");
    const provider = cachedProvider ?? (await this.providers.getForType(connection.type));

    // Abort any previous stream on this tab (e.g. user hit Run again while a
    // stream was still filling in rows).
    this.abortTabStream(tabId);
    const controller = new AbortController();
    this.streamControllers.set(tabId, controller);

    const start = performance.now();

    const streamResult = await provider.selectStream(
      providerConnectionId,
      sql,
      bindValues,
      (batch) => {
        if (controller.signal.aborted) return false;
        const target = this.getProxiedResult(tabId, statementIndex);
        if (!target) return false;

        if (batch.columns && target.columns.length === 0) {
          // Duplicate column names (e.g. `SELECT a.id, b.id FROM a JOIN b`)
          // break every `columns.indexOf(name)` lookup downstream (charts,
          // column copy, cell-type detection). Disambiguate with `_2`, `_3`
          // suffixes — positions stay ground truth, only names change.
          target.columns = dedupeColumnNames(batch.columns);
        }
        if (batch.rows.length > 0) {
          // target is proxied — `.push` on the proxied array goes through
          // Svelte 5's array trap and fires reactivity. This is O(batch)
          // per call, unlike a spread-reassignment which would be O(N²)
          // across the full stream.
          target.rows.push(...batch.rows);
          target.rowCount = target.rows.length;
          target.totalRows = target.rows.length;
        }
        // Tick the elapsed-time display on every batch so the result
        // badge ("N rows, Tms") updates live alongside the row counter.
        target.executionTime = Math.round((performance.now() - start) * 100) / 100;
        if (batch.isFinal) {
          target.isStreaming = false;
        }
        return !controller.signal.aborted;
      },
      controller.signal,
    );

    const finalTarget = this.getProxiedResult(tabId, statementIndex);
    if (finalTarget) {
      if (streamResult.error) {
        finalTarget.isError = true;
        finalTarget.error = streamResult.error;
      }
      // Always clear the streaming flag when the stream has terminated,
      // no matter how (clean finish, abort, error, or silent termination).
      finalTarget.isStreaming = false;
      // Safety net: the happy path updates `executionTime` on every batch,
      // so this only fires when the stream terminated without ever emitting
      // a batch (e.g. Rust errored immediately, or the shim driver returned
      // an empty result).
      if (finalTarget.executionTime === 0) {
        finalTarget.executionTime = Math.round((performance.now() - start) * 100) / 100;
      }
    }

    if (this.streamControllers.get(tabId) === controller) {
      this.streamControllers.delete(tabId);
    }

    return { aborted: streamResult.aborted, error: streamResult.error };
  }

  /**
   * Build a seeded StatementResult that's ready to be mutated by a streaming
   * run. The caller installs this in the tab's results array, then passes it
   * as `target` to `runStreamingStatement`.
   */
  private createStreamingSeed(
    statementIndex: number,
    statementSql: string,
    baseQuery: string,
    pageSize: number,
    connection: DatabaseConnection,
  ): StatementResult {
    return {
      columns: [],
      rows: [],
      rowCount: 0,
      totalRows: 0,
      executionTime: 0,
      queryType: detectQueryType(baseQuery, connection.type),
      sourceTable: this.resolveSourceTable(baseQuery, connection),
      columnSources: this.resolveColumnSources(baseQuery, connection),
      page: 1,
      pageSize,
      totalPages: 1,
      statementIndex,
      statementSql,
      isError: false,
      isStreaming: true,
    };
  }

  /**
   * Update a query tab's state with proper Svelte 5 reactivity.
   */
  private updateQueryTabState(
    tabId: string,
    updates: Partial<{
      results: StatementResult[];
      activeResultIndex: number;
      isExecuting: boolean;
    }>,
  ): void {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const tabs = this.state.queryTabsByProject[projectId];
    if (!tabs) return;

    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    Object.assign(tab, updates);
    // Trigger Svelte 5 reactivity by reassigning the top-level object
    this.state.queryTabsByProject = { ...this.state.queryTabsByProject };
  }

  /**
   * Create a standardized error result for failed statement execution.
   */
  private createErrorResult(
    statementSql: string,
    error: unknown,
    statementIndex: number,
    pageSize: number = 1,
  ): StatementResult {
    return {
      columns: ["Error"],
      rows: [[extractErrorMessage(error)]],
      rowCount: 1,
      totalRows: 1,
      executionTime: 0,
      page: 1,
      pageSize,
      totalPages: 1,
      statementIndex,
      statementSql,
      error: extractErrorMessage(error),
      isError: true,
    };
  }

  /**
   * Filter out utility results and re-index for display.
   * Keeps utility results only if ALL results are utility (so the user sees something).
   */
  private filterAndIndexResults(allResults: StatementResult[]): StatementResult[] {
    const displayResults = allResults.filter((r) => !r.isUtility);
    const results = displayResults.length > 0 ? displayResults : allResults;
    return results.map((r, idx) => ({ ...r, statementIndex: idx }));
  }

  /**
   * Get primary keys for a table, from the cached schema of `connectionId`
   * (the active connection by default).
   */
  getPrimaryKeysForTable(
    schema: string,
    tableName: string,
    connectionId: string | null = this.state.activeConnectionId,
  ): string[] {
    if (!connectionId) return [];
    const tables = this.state.schemas[connectionId] ?? [];
    const table = tables.find((t) => t.name === tableName && t.schema === schema);
    if (!table) return [];
    return table.columns.filter((c) => c.isPrimaryKey).map((c) => c.name);
  }

  /**
   * Execute a single SQL statement and return the result.
   * @param sql The SQL statement to execute (may contain $1, $2, etc. placeholders)
   * @param page Page number for pagination
   * @param pageSize Number of rows per page
   * @param connection Database connection to use
   * @param bindValues Optional bind values for parameterized queries (PostgreSQL/SQLite only)
   */
  private async executeStatement(
    sql: string,
    page: number,
    pageSize: number,
    connection: DatabaseConnection,
    bindValues?: unknown[],
    cachedProvider?: DatabaseProvider,
  ): Promise<QueryResult> {
    const start = performance.now();
    const baseQuery = sql.replace(/;$/, "").trim();
    // Strict: "other" would run a SELECT unpaged, as a utility statement.
    const queryType = detectQueryTypeOrThrow(baseQuery, connection.type);
    const providerConnectionId = connection.providerConnectionId;
    if (!providerConnectionId) {
      throw new Error("No connection established");
    }
    const provider = cachedProvider ?? (await this.providers.getForType(connection.type));
    const client = getEngineClient(connection, this.state);

    // Handle utility/DDL statements (SET, PRAGMA, CREATE, ALTER, DROP, etc.)
    // These are not SELECT and not write queries — execute them without pagination.
    // Use select() (not execute()) so statements like SET properly modify connection state
    // in backends like DuckDB where conn.execute() may not apply settings.
    const isWrite = queryType === "insert" || queryType === "update" || queryType === "delete";
    if (queryType !== "select" && !isWrite) {
      await provider.select(providerConnectionId, baseQuery, bindValues);

      const totalMs = performance.now() - start;
      return {
        columns: [],
        rows: [],
        rowCount: 0,
        totalRows: 0,
        executionTime: Math.round(totalMs * 100) / 100,
        queryType,
        page: 1,
        pageSize,
        totalPages: 1,
        isUtility: true,
      };
    }

    // Handle write queries (INSERT/UPDATE/DELETE)
    if (isWrite) {
      let rowsAffected = 0;
      let lastInsertId: number | undefined;

      const executeResult = await provider.execute(providerConnectionId, baseQuery, bindValues);
      rowsAffected = executeResult?.rowsAffected ?? 0;
      lastInsertId = executeResult?.lastInsertId;

      const totalMs = performance.now() - start;

      return {
        columns: ["Result"],
        rows: [[`${rowsAffected} row(s) affected`]],
        rowCount: 1,
        totalRows: 1,
        executionTime: Math.round(totalMs * 100) / 100,
        affectedRows: rowsAffected,
        lastInsertId,
        queryType,
        page: 1,
        pageSize: 1,
        totalPages: 1,
      };
    }

    // Handle SELECT queries
    // A query with its own top-level LIMIT/OFFSET/FETCH (TOP on SQL Server)
    // isn't paged.
    const hasPagination = hasRowLimit(baseQuery, connection.type);

    let totalRows = 0;
    let paginatedQuery = baseQuery;

    let dbResult: Record<string, unknown>[];

    if (hasPagination || pageSize === 0) {
      // NOTE: under normal flow this branch is unreachable for SELECTs —
      // `executeCurrent`, `execute`, and `executeStatementAtIndex` all
      // detect `shouldStream(...)` matches before they call into
      // `executeStatement` and route them through `runStreamingStatement`
      // instead. This fallback exists only as a safety net in case a new
      // caller passes a streamable query straight through, and to keep
      // the `dbResult` assignment total.
      dbResult = await provider.select<Record<string, unknown>>(
        providerConnectionId,
        baseQuery,
        bindValues,
      );
      totalRows = dbResult?.length ?? 0;
    } else {
      // Optimistic pagination: fetch pageSize + 1 rows to detect whether more pages exist.
      // Only run an expensive COUNT query when the result fills the page.
      const offset = (page - 1) * pageSize;
      const probeLimit = pageSize + 1;

      paginatedQuery = await client.paginate(baseQuery, probeLimit, offset);

      dbResult = await provider.select<Record<string, unknown>>(
        providerConnectionId,
        paginatedQuery,
        bindValues,
      );

      if ((dbResult?.length ?? 0) <= pageSize) {
        // All results fit on this page — no COUNT needed
        totalRows = offset + (dbResult?.length ?? 0);
      } else {
        // More rows exist — trim the extra probe row and run COUNT
        dbResult = dbResult.slice(0, pageSize);
        try {
          const countResult = await provider.select<{ total: string | number }>(
            providerConnectionId,
            countQuery(baseQuery, connection.type),
            bindValues,
          );
          totalRows = parseInt(String(countResult[0]?.total ?? "0"), 10);
        } catch (error) {
          // The COUNT query (or building it) failed: estimate the total.
          void log.warn(`Row count failed on ${connection.id}: ${extractErrorMessage(error)}`);
          totalRows = offset + pageSize + 1;
        }
      }
    }

    const resultColumns = (dbResult?.length ?? 0) > 0 ? Object.keys(dbResult[0]) : [];
    // Convert the row-object result from `provider.select()` into the columnar
    // shape that `QueryResult.rows` now uses. This is the non-streaming
    // fallback path — only runs for small, paginated queries, so the O(n·m)
    // conversion cost is negligible.
    const columnarRows: unknown[][] = (dbResult ?? []).map((row) =>
      resultColumns.map((c) => row[c]),
    );
    const totalMs = performance.now() - start;

    const totalPages =
      hasPagination || pageSize === 0 ? 1 : Math.max(1, Math.ceil(totalRows / pageSize));

    // Try to extract source table info for CRUD operations
    const sourceTable = this.resolveSourceTable(baseQuery, connection);

    // Generate results
    return {
      columns: resultColumns,
      rows: columnarRows,
      rowCount: columnarRows.length,
      totalRows,
      executionTime: Math.round(totalMs * 100) / 100,
      queryType,
      sourceTable,
      columnSources: this.resolveColumnSources(baseQuery, connection),
      page,
      pageSize,
      totalPages,
    };
  }

  /**
   * Execute only the statement at the cursor position, with optional parameter substitution.
   */
  async executeCurrent(
    tabId: string,
    cursorOffset: number,
    page: number = 1,
    pageSize?: number,
    parameterValues?: ParameterValue[],
  ): Promise<void> {
    if (!this.state.activeProjectId) return;

    const connection = this.state.activeConnection;
    const isConnected = !!connection?.providerConnectionId;
    if (!connection || !isConnected) {
      errorToast("Not connected to database. Please reconnect.");
      return;
    }

    // Resolve the statement at cursor once (without params) to get the original SQL
    let baseResolved: ReturnType<typeof resolveQueryOrThrow>;
    try {
      baseResolved = resolveQueryOrThrow(this.state, tabId, cursorOffset);
    } catch (error) {
      // Don't fall back to the whole buffer: it may hold statements the
      // user didn't pick.
      errorToast(m.statement_at_cursor_failed({ error: extractErrorMessage(error) }));
      return;
    }
    if (!baseResolved) {
      toast.info(m.query_no_executable_statements());
      return;
    }

    const originalSql = baseResolved.query;
    const dbType = connection.type ?? "postgres";

    // Substitute parameters if provided
    let query = originalSql;
    let bindValues: unknown[] | undefined;
    if (parameterValues) {
      try {
        const substituted = substituteParameters(originalSql, parameterValues, dbType);
        query = substituted.sql;
        bindValues = substituted.bindValues;
      } catch (error) {
        errorToast(extractErrorMessage(error));
        return;
      }
    }

    // Cancel any in-flight stream for this tab before starting a new query.
    this.abortTabStream(tabId);

    // Mark as executing
    this.updateQueryTabState(tabId, { isExecuting: true });

    // Get effective page size: use the first SELECT-type result's pageSize from previous execution, or default.
    // Avoid inheriting pageSize from error or utility results — a previous run that errored stores
    // a placeholder pageSize=1 in createErrorResult, which would otherwise contaminate re-runs.
    const previousSelectResult = baseResolved.tab.results?.find(
      (r) => !r.isError && !r.isUtility && r.queryType === "select",
    );
    const effectivePageSize = pageSize ?? previousSelectResult?.pageSize ?? this.DEFAULT_PAGE_SIZE;

    // The statement's kind, and whether a SELECT goes through the streaming
    // path. Both checks throw if the SQL module fails.
    const baseQuery = query.replace(/;$/, "").trim();
    let queryType: QueryType;
    let isStreamingSelect: boolean;
    try {
      queryType = detectQueryTypeOrThrow(query, dbType);
      isStreamingSelect =
        queryType === "select" && this.shouldStream(baseQuery, dbType, effectivePageSize);
    } catch (error) {
      // A check couldn't run: report it rather than guess.
      this.updateQueryTabState(tabId, {
        results: [this.createErrorResult(originalSql, error, 0, effectivePageSize)],
        activeResultIndex: 0,
        isExecuting: false,
      });
      return;
    }

    // Queue non-SELECT statements when pending changes is enabled
    if (this.pendingChanges.isEnabled() && queryType !== "select") {
      const origin: PendingChangeOrigin = "query-editor";
      this.pendingChanges.add(connection.id, query, queryType, origin, tabId, bindValues);
      this.updateQueryTabState(tabId, { isExecuting: false });
      toast.info("Statement added to pending changes");
      this.pendingChanges.openSheet();
      return;
    }

    if (isStreamingSelect) {
      // Seed a streaming result and install it in tab state BEFORE awaiting
      // the stream, so the UI shows an empty table + "streaming…" indicator
      // while rows are flowing in.
      const seed = this.createStreamingSeed(
        0,
        originalSql,
        baseQuery,
        effectivePageSize,
        connection,
      );
      this.updateQueryTabState(tabId, {
        results: [seed],
        activeResultIndex: 0,
      });

      try {
        const { aborted, error: streamError } = await this.runStreamingStatement(
          tabId,
          0,
          baseQuery,
          bindValues,
          connection,
        );
        const finalResult = this.getProxiedResult(tabId, 0);
        if (aborted) {
          void log.info(
            `Streaming query cancelled on ${connection.id}: ${finalResult?.rowCount ?? 0} rows before cancel`,
          );
        } else if (streamError || finalResult?.isError) {
          void log.error(`Streaming query failed on ${connection.id}`);
        } else if (finalResult) {
          void log.info(
            `Streaming query on ${connection.id}: ${finalResult.rowCount} rows in ${finalResult.executionTime}ms`,
          );
          // Only record successful runs in history — skip cancelled and
          // errored ones so the "rerun this query" UX doesn't resurrect
          // partial results as if they were real.
          if (page === 1) {
            this.queryHistory.addToHistory(originalSql, finalResult);
          }
        }
      } catch (error) {
        void log.error(`Streaming query failed on ${connection.id}`);
        this.updateQueryTabState(tabId, {
          results: [this.createErrorResult(originalSql, error, 0, effectivePageSize)],
          activeResultIndex: 0,
        });
      } finally {
        this.updateQueryTabState(tabId, { isExecuting: false });
      }
      return;
    }

    try {
      const result = await this.executeStatement(
        query,
        page,
        effectivePageSize,
        connection,
        bindValues,
      );
      void log.info(
        `Query executed on ${connection.id}: ${result.rowCount} rows in ${result.executionTime}ms`,
      );
      const results: StatementResult[] = [
        {
          ...result,
          statementIndex: 0,
          statementSql: originalSql,
          isError: false,
        },
      ];

      this.updateQueryTabState(tabId, {
        results,
        activeResultIndex: 0,
        isExecuting: false,
      });

      // Add to history
      if (page === 1) {
        this.queryHistory.addToHistory(originalSql, results[0]);
      }
    } catch (error) {
      void log.error(`Query execution failed on ${connection.id}`);
      this.updateQueryTabState(tabId, {
        results: [this.createErrorResult(originalSql, error, 0, effectivePageSize)],
        activeResultIndex: 0,
        isExecuting: false,
      });
    }
  }

  /** @deprecated Use executeCurrent with parameterValues param */
  executeCurrentWithParams(
    tabId: string,
    cursorOffset: number,
    parameterValues: ParameterValue[],
    page?: number,
    pageSize?: number,
  ) {
    return this.executeCurrent(tabId, cursorOffset, page, pageSize, parameterValues);
  }

  /**
   * Execute all statements in a query tab, with optional parameter substitution.
   */
  async execute(
    tabId: string,
    page: number = 1,
    pageSize?: number,
    parameterValues?: ParameterValue[],
  ): Promise<void> {
    if (!this.state.activeProjectId) return;

    const connection = this.state.activeConnection;
    const isConnected = !!connection?.providerConnectionId;
    if (!connection || !isConnected) {
      errorToast("Not connected to database. Please reconnect.");
      return;
    }

    const tabs = this.state.queryTabsByProject[this.state.activeProjectId] ?? [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    // Cancel any in-flight stream for this tab before starting a new batch.
    this.abortTabStream(tabId);

    // Mark as executing with proper reactivity
    this.updateQueryTabState(tabId, { isExecuting: true });

    // Get effective page size: use the first SELECT-type result's pageSize from previous execution, or default.
    // Avoid inheriting pageSize from error or utility results.
    const previousSelectResult = tab.results?.find(
      (r) => !r.isError && !r.isUtility && r.queryType === "select",
    );
    const effectivePageSize = pageSize ?? previousSelectResult?.pageSize ?? this.DEFAULT_PAGE_SIZE;

    // Get database type for parsing
    const dbType = connection.type ?? "postgres";

    // Parse SQL into individual statements. Strict: `[]` would read as
    // "nothing to run".
    let statements: ParsedStatement[];
    try {
      statements = splitSqlStatementsOrThrow(tab.query, dbType);
    } catch (error) {
      this.updateQueryTabState(tabId, {
        results: [this.createErrorResult(tab.query, error, 0, effectivePageSize)],
        activeResultIndex: 0,
        isExecuting: false,
      });
      return;
    }

    // Handle case where all statements are comments
    if (statements.length === 0) {
      this.updateQueryTabState(tabId, {
        results: [],
        activeResultIndex: 0,
        isExecuting: false,
      });
      toast.info(m.query_no_executable_statements());
      return;
    }

    const allResults: StatementResult[] = [];
    let queuedCount = 0;
    // Track whether any streaming statement in this batch was aborted or
    // errored — if so, we skip the history add at the bottom so partial
    // results don't resurface as "rerun this query" candidates.
    let anyStreamAbortedOrErrored = false;
    const provider = await this.providers.getForType(connection.type);

    // Execute each statement, updating the UI incrementally
    for (let i = 0; i < statements.length; i++) {
      const stmt = statements[i];
      try {
        // Substitute parameters if provided
        let sql = stmt.sql;
        let bindValues: unknown[] | undefined;
        if (parameterValues) {
          const substituted = substituteParameters(stmt.sql, parameterValues, dbType);
          sql = substituted.sql;
          bindValues = substituted.bindValues;
        }

        // Strict: a module failure becomes this statement's error result.
        const queryType = detectQueryTypeOrThrow(sql, dbType);

        // Queue write/DDL statements when pending changes is enabled
        if (this.pendingChanges.isEnabled() && queryType !== "select") {
          const origin: PendingChangeOrigin = "query-editor";
          this.pendingChanges.add(connection.id, sql, queryType, origin, tabId, bindValues);
          queuedCount++;
          continue;
        }

        const baseQueryForDetection = sql.replace(/;$/, "").trim();
        const isStreamingSelect =
          queryType === "select" &&
          this.shouldStream(baseQueryForDetection, dbType, effectivePageSize);

        if (isStreamingSelect) {
          // Seed a streaming result, install it so the UI shows progress,
          // then let runStreamingStatement mutate the proxied copy in
          // tab state as batches land.
          const seed = this.createStreamingSeed(
            i,
            stmt.sql,
            baseQueryForDetection,
            effectivePageSize,
            connection,
          );
          const seedIndex = allResults.length;
          allResults.push(seed);
          this.updateQueryTabState(tabId, {
            results: [...allResults],
            activeResultIndex: 0,
          });
          try {
            const { aborted, error: streamError } = await this.runStreamingStatement(
              tabId,
              seedIndex,
              baseQueryForDetection,
              bindValues,
              connection,
              provider,
            );
            if (aborted || streamError) {
              anyStreamAbortedOrErrored = true;
            }
            // Sync the streamed result back into allResults so the outer
            // loop's `[...allResults]` reassignment preserves it, and the
            // end-of-batch `filterAndIndexResults` sees the final rows.
            const streamed = this.getProxiedResult(tabId, seedIndex);
            if (streamed) {
              allResults[seedIndex] = streamed;
            }
          } catch (streamError) {
            // Replace the seed in place with a proper error result so the
            // outer catch below doesn't push a duplicate.
            allResults[seedIndex] = this.createErrorResult(
              stmt.sql,
              streamError,
              i,
              effectivePageSize,
            );
          }
        } else {
          const result = await this.executeStatement(
            sql,
            page,
            effectivePageSize,
            connection,
            bindValues,
            provider,
          );
          allResults.push({
            ...result,
            statementIndex: i,
            statementSql: stmt.sql, // Keep original SQL for display
            isError: false,
          });
        }
      } catch (error) {
        // Continue on error - add error result
        allResults.push(this.createErrorResult(stmt.sql, error, i, effectivePageSize));
      }

      // Update UI incrementally after each statement completes
      this.updateQueryTabState(tabId, {
        results: [...allResults],
        activeResultIndex: 0,
      });
    }

    // Show toast for queued statements
    if (queuedCount > 0) {
      toast.info(`${queuedCount} statement${queuedCount > 1 ? "s" : ""} added to pending changes`);
      this.pendingChanges.openSheet();
    }

    // Log summary for all statements
    const totalTime = allResults.reduce((sum, r) => sum + (r.executionTime ?? 0), 0);
    const totalRows = allResults.reduce((sum, r) => sum + (r.isError ? 0 : r.rowCount), 0);
    const errorCount = allResults.filter((r) => r.isError).length;
    if (errorCount > 0) {
      void log.warn(
        `Query batch on ${connection.id}: ${allResults.length} statements, ${errorCount} failed, ${totalRows} rows in ${Math.round(totalTime * 100) / 100}ms`,
      );
    } else {
      void log.info(
        `Query batch on ${connection.id}: ${allResults.length} statements, ${totalRows} rows in ${Math.round(totalTime * 100) / 100}ms`,
      );
    }

    // Filter utility results and mark execution as complete
    const indexedResults = this.filterAndIndexResults(allResults);
    this.updateQueryTabState(tabId, {
      results: indexedResults,
      isExecuting: false,
    });

    // Add to history (only on first page to avoid duplicates, use first meaningful result).
    // Skip when any streaming statement was aborted or errored — partial
    // results aren't a meaningful history entry the user would want to rerun.
    if (page === 1 && indexedResults.length > 0 && !anyStreamAbortedOrErrored) {
      const historyResult = indexedResults.find((r) => !r.isUtility) ?? indexedResults[0];
      this.queryHistory.addToHistory(tab.query, historyResult);
    }
  }

  /** @deprecated Use execute with parameterValues param */
  executeWithParams(
    tabId: string,
    parameterValues: ParameterValue[],
    page?: number,
    pageSize?: number,
  ) {
    return this.execute(tabId, page, pageSize, parameterValues);
  }

  /**
   * Set the active result tab index.
   */
  setActiveResult(tabId: string, resultIndex: number): void {
    if (!this.state.activeProjectId) return;

    const tabs = this.state.queryTabsByProject[this.state.activeProjectId] ?? [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results || resultIndex < 0 || resultIndex >= tab.results.length) return;

    this.updateQueryTabState(tabId, { activeResultIndex: resultIndex });
  }

  /**
   * Navigate to a specific page for a specific result.
   */
  async goToPage(tabId: string, page: number, resultIndex?: number): Promise<void> {
    const tabs = this.state.queryTabsByProject[this.state.activeProjectId!] ?? [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results) return;

    const activeIndex = resultIndex ?? tab.activeResultIndex ?? 0;
    const result = tab.results[activeIndex];
    if (!result) return;

    const targetPage = Math.max(1, Math.min(page, result.totalPages));

    // Re-execute the specific statement with new page
    await this.executeStatementAtIndex(tabId, activeIndex, targetPage, result.pageSize);
  }

  /**
   * Re-execute a specific statement at a given index with pagination.
   */
  private async executeStatementAtIndex(
    tabId: string,
    resultIndex: number,
    page: number,
    pageSize: number,
  ): Promise<void> {
    if (!this.state.activeProjectId) return;

    const connection = this.state.activeConnection;
    const isConnected = !!connection?.providerConnectionId;
    if (!connection || !isConnected) return;

    const tabs = this.state.queryTabsByProject[this.state.activeProjectId] ?? [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results || resultIndex >= tab.results.length) return;

    const existingResult = tab.results[resultIndex];

    // Cancel any in-flight stream for this tab before re-running.
    this.abortTabStream(tabId);

    const baseQuery = existingResult.statementSql.replace(/;$/, "").trim();
    let isStreamingSelect: boolean;
    try {
      isStreamingSelect =
        detectQueryTypeOrThrow(existingResult.statementSql, connection.type) === "select" &&
        this.shouldStream(baseQuery, connection.type, pageSize);
    } catch (error) {
      // A check couldn't run: report it rather than guess.
      const errResults = [...tab.results];
      errResults[resultIndex] = {
        ...existingResult,
        error: extractErrorMessage(error),
        isError: true,
      };
      this.updateQueryTabState(tabId, { results: errResults });
      return;
    }

    if (isStreamingSelect) {
      const seed = this.createStreamingSeed(
        resultIndex,
        existingResult.statementSql,
        baseQuery,
        pageSize,
        connection,
      );
      const newResults = [...tab.results];
      newResults[resultIndex] = seed;
      this.updateQueryTabState(tabId, { results: newResults });

      try {
        await this.runStreamingStatement(tabId, resultIndex, baseQuery, undefined, connection);
      } catch (error) {
        const errResults = [...(tab.results ?? [])];
        errResults[resultIndex] = {
          ...existingResult,
          error: extractErrorMessage(error),
          isError: true,
          isStreaming: false,
        };
        this.updateQueryTabState(tabId, { results: errResults });
      }
      return;
    }

    try {
      const result = await this.executeStatement(
        existingResult.statementSql,
        page,
        pageSize,
        connection,
      );

      // Update only this specific result in the array
      const newResults = [...tab.results];
      newResults[resultIndex] = {
        ...result,
        statementIndex: resultIndex,
        statementSql: existingResult.statementSql,
        isError: false,
      };

      this.updateQueryTabState(tabId, { results: newResults });
    } catch (error) {
      // Update with error
      const newResults = [...tab.results];
      newResults[resultIndex] = {
        ...existingResult,
        error: extractErrorMessage(error),
        isError: true,
      };
      this.updateQueryTabState(tabId, { results: newResults });
    }
  }

  /**
   * Set page size and re-execute query.
   */
  async setPageSize(tabId: string, pageSize: number, resultIndex?: number): Promise<void> {
    const tabs = this.state.queryTabsByProject[this.state.activeProjectId!] ?? [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results) return;

    const activeIndex = resultIndex ?? tab.activeResultIndex ?? 0;
    await this.executeStatementAtIndex(tabId, activeIndex, 1, pageSize);
  }

  /**
   * Update a cell value in the database (query tab version).
   * Resolves the row from tab results, then delegates to crud.updateCellDirect.
   */
  async updateCell(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
    column: string,
    newValue: unknown,
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
  ): Promise<{ success: boolean; error?: string; queued?: boolean }> {
    const editTarget = this.resolveEditTarget(tabId, resultIndex, rowIndex, column, sourceTable);
    if (!editTarget) return { success: false, error: "Row not found" };
    if (editTarget.error) return { success: false, error: editTarget.error };

    void log.debug(`Cell update on ${this.state.activeConnection?.id}`);
    const result = await this.crud.updateCellDirect(
      editTarget.sourceTable,
      editTarget.row,
      editTarget.column,
      newValue,
    );
    if (result.success) {
      // Write the new value back into the columnar store so the UI reflects
      // the edit. `row` above is a disposable materialized copy — updating
      // it alone wouldn't propagate. We replace the whole inner row array
      // rather than mutating by index: streamed rows arrive from the Tauri
      // Channel as plain arrays and are pushed into the proxied outer array,
      // so we can't assume the inner array is deeply-tracked. Replacing the
      // slot fires the outer array's set trap, which reliably re-renders
      // the affected row.
      const tabs = this.state.queryTabsByProject[this.state.activeProjectId!] ?? [];
      const tab = tabs.find((t) => t.id === tabId);
      const target = tab?.results?.[resultIndex];
      if (target) {
        const colIdx = target.columns.indexOf(column);
        const existing = target.rows[rowIndex];
        if (colIdx !== -1 && existing) {
          const next = existing.slice();
          next[colIdx] = newValue;
          target.rows[rowIndex] = next;
        }
      }
    }
    return result;
  }

  /**
   * Set a cell value to its column DEFAULT (query tab version).
   * Resolves the row from tab results, then delegates to crud.setCellDefaultDirect.
   */
  async setCellDefault(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
    column: string,
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
  ): Promise<{ success: boolean; error?: string; queued?: boolean }> {
    const editTarget = this.resolveEditTarget(tabId, resultIndex, rowIndex, column, sourceTable);
    if (!editTarget) return { success: false, error: "Row not found" };
    if (editTarget.error) return { success: false, error: editTarget.error };

    void log.debug(`Cell set default on ${this.state.activeConnection?.id}`);
    const result = await this.crud.setCellDefaultDirect(
      editTarget.sourceTable,
      editTarget.row,
      editTarget.column,
    );
    if (result.success && !result.queued) {
      // Re-fetch the row to get the actual default value
      await this.execute(tabId);
    }
    return result;
  }

  // --- Delegated CRUD methods (preserve public API) ---

  insertRow(sourceTable: { schema: string; name: string }, values: Record<string, unknown>) {
    return this.crud.insertRow(sourceTable, values);
  }

  deleteRow(
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
  ) {
    return this.crud.deleteRow(sourceTable, row);
  }

  updateCellDirect(
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
    column: string,
    newValue: unknown,
  ) {
    return this.crud.updateCellDirect(sourceTable, row, column, newValue, {
      deduplicatePending: true,
    });
  }

  setCellDefaultDirect(
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>,
    column: string,
  ) {
    return this.crud.setCellDefaultDirect(sourceTable, row, column, { deduplicatePending: true });
  }

  executeRaw(query: string) {
    return this.crud.executeRaw(query);
  }

  /** See `QueryCrudManager.executeReadOnly`: AI and dashboard SQL only. */
  executeReadOnly(
    connectionId: string,
    sql: string,
    signal?: AbortSignal,
    connectionName?: string,
  ) {
    return this.crud.executeReadOnly(connectionId, sql, signal, connectionName);
  }

  executeRawDdl(query: string) {
    return this.crud.executeRawDdl(query);
  }

  /**
   * Resolve a row from a query tab's results.
   *
   * Rows are stored columnar (`unknown[][]`); CRUD helpers still expect
   * `Record<string, unknown>`, so we materialize one here on demand. This
   * only fires on user-initiated cell edits / deletes, so the conversion
   * cost is trivial.
   */
  private getRowFromTab(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
  ): Record<string, unknown> | undefined {
    const tabs = this.state.queryTabsByProject[this.state.activeProjectId!] ?? [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results || resultIndex >= tab.results.length) return undefined;
    const result = tab.results[resultIndex];
    const row = result.rows[rowIndex];
    if (!row) return undefined;
    return rowToObject(row, result.columns);
  }

  /**
   * Resolve the actual UPDATE/SET-DEFAULT target for a cell edit.
   *
   * The user edits a cell under its *display* column name, which may have been
   * dedupe-suffixed (e.g. `id_2` for the second `id` in a JOIN'd result) or
   * aliased via `SELECT x AS y`. We use the query's `columnSources` to route
   * the edit to the underlying table+column, and to reshape the row into a
   * `Record<actualPKName, value>` that the WHERE-clause builder can consume
   * directly.
   *
   * Falls back to the caller-supplied `sourceTable` and literal `column` name
   * when per-column info is unavailable (unparseable query, `SELECT *`, etc.),
   * preserving the pre-existing single-table behavior.
   */
  private resolveEditTarget(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
    column: string,
    fallbackSourceTable: { schema: string; name: string; primaryKeys: string[] },
  ):
    | {
        sourceTable: { schema: string; name: string; primaryKeys: string[] };
        row: Record<string, unknown>;
        column: string;
        error?: string;
      }
    | undefined {
    const tabs = this.state.queryTabsByProject[this.state.activeProjectId!] ?? [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results || resultIndex >= tab.results.length) return undefined;
    const result = tab.results[resultIndex];
    const rawRow = result.rows[rowIndex];
    if (!rawRow) return undefined;

    // No per-column info parsed — fall back to single-table routing, keyed by
    // display names (which is what the WHERE builder has always seen).
    const colIdx = result.columns.indexOf(column);
    const sources = result.columnSources;
    if (!sources || colIdx === -1 || !sources[colIdx]) {
      return {
        sourceTable: fallbackSourceTable,
        row: rowToObject(rawRow, result.columns),
        column,
      };
    }

    const src = sources[colIdx]!;
    // Pull the target table's PK values out of the row by scanning for any
    // output columns that map back to one of its PK columns. A query that
    // doesn't project all of the target table's PKs can't be updated safely —
    // we'd have no way to identify the specific row in a WHERE clause.
    const rowObj: Record<string, unknown> = {};
    const foundPks = new Set<string>();
    for (let i = 0; i < result.columns.length; i++) {
      const s = sources[i];
      if (!s) continue;
      if (s.schema !== src.schema || s.table !== src.table) continue;
      if (src.primaryKeys.includes(s.column)) {
        rowObj[s.column] = rawRow[i];
        foundPks.add(s.column);
      }
    }
    const missingPks = src.primaryKeys.filter((pk) => !foundPks.has(pk));
    if (missingPks.length > 0) {
      return {
        sourceTable: fallbackSourceTable,
        row: rowToObject(rawRow, result.columns),
        column,
        error: `Cannot edit ${src.table}.${src.column}: primary key ${missingPks.join(", ")} is not in the result`,
      };
    }

    return {
      sourceTable: {
        schema: src.schema,
        name: src.table,
        primaryKeys: src.primaryKeys,
      },
      row: rowObj,
      column: src.column,
    };
  }
}
