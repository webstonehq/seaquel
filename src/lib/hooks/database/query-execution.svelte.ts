import { toast } from "svelte-sonner";
import type { QueryResult, StatementResult } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { QueryHistoryManager } from "./query-history.svelte.js";
import { detectQueryType, isWriteQuery, extractTableFromSelect } from "$lib/db/query-utils";
import { splitSqlStatements } from "$lib/db/sql-parser";
import { updateMapArrayItem } from "./map-utils.js";
import { m } from "$lib/paraglide/messages.js";

/**
 * Manages query execution, pagination, and CRUD operations.
 */
export class QueryExecutionManager {
  private readonly DEFAULT_PAGE_SIZE = 100;

  constructor(
    private state: DatabaseState,
    private queryHistory: QueryHistoryManager
  ) {}

  /**
   * Formats an unknown error into a user-friendly string message.
   */
  private formatError(error: unknown): string {
    if (error instanceof Error) {
      return error.message;
    }
    return String(error);
  }

  /**
   * Update a query tab's state with proper Svelte 5 reactivity.
   */
  private updateQueryTabState(
    tabId: string,
    updates: Partial<{ results: StatementResult[]; activeResultIndex: number; isExecuting: boolean }>
  ): void {
    if (!this.state.activeConnectionId) return;
    updateMapArrayItem(
      () => this.state.queryTabsByConnection,
      (m) => (this.state.queryTabsByConnection = m),
      this.state.activeConnectionId,
      tabId,
      updates
    );
  }

  /**
   * Get primary keys for a table.
   */
  getPrimaryKeysForTable(schema: string, tableName: string): string[] {
    if (!this.state.activeConnectionId) return [];
    const tables = this.state.schemas.get(this.state.activeConnectionId) || [];
    const table = tables.find((t) => t.name === tableName && t.schema === schema);
    if (!table) return [];
    return table.columns.filter((c) => c.isPrimaryKey).map((c) => c.name);
  }

  /**
   * Execute a single SQL statement and return the result.
   */
  private async executeStatement(
    sql: string,
    page: number,
    pageSize: number,
    database: NonNullable<typeof this.state.activeConnection>["database"]
  ): Promise<QueryResult> {
    const start = performance.now();
    const baseQuery = sql.replace(/;$/, "").trim();
    const queryType = detectQueryType(baseQuery);

    // Handle write queries (INSERT/UPDATE/DELETE)
    if (isWriteQuery(baseQuery)) {
      const executeResult = await database!.execute(baseQuery);
      const totalMs = performance.now() - start;

      return {
        columns: ["Result"],
        rows: [{ Result: `${executeResult?.rowsAffected ?? 0} row(s) affected` }],
        rowCount: 1,
        totalRows: 1,
        executionTime: Math.round(totalMs * 100) / 100,
        affectedRows: executeResult?.rowsAffected ?? 0,
        lastInsertId: executeResult?.lastInsertId,
        queryType,
        page: 1,
        pageSize: 1,
        totalPages: 1,
      };
    }

    // Handle SELECT queries
    // Check if query already has LIMIT clause - if so, skip pagination
    const hasLimit = /\bLIMIT\b/i.test(baseQuery);

    let totalRows = 0;
    let paginatedQuery = baseQuery;

    if (!hasLimit) {
      // Get total count first by wrapping in a subquery
      const countQuery = `SELECT COUNT(*) as total FROM (${baseQuery}) AS count_query`;
      try {
        const countResult = (await database!.select(countQuery)) as { total: string | number }[];
        totalRows = parseInt(String(countResult[0]?.total ?? "0"), 10);
      } catch {
        // If count fails, just run the query without pagination
        totalRows = -1;
      }

      // Add LIMIT/OFFSET if we successfully got a count (it's a SELECT)
      if (totalRows >= 0) {
        const offset = (page - 1) * pageSize;
        paginatedQuery = `${baseQuery} LIMIT ${pageSize} OFFSET ${offset}`;
      }
    } else {
      // Query has its own LIMIT, don't paginate
      totalRows = -1;
    }

    const dbResult = (await database!.select(paginatedQuery)) as Record<string, unknown>[];
    const totalMs = performance.now() - start;

    // If count failed or query had LIMIT, use result length as total
    if (totalRows < 0) {
      totalRows = dbResult?.length ?? 0;
    }

    const totalPages = hasLimit ? 1 : Math.max(1, Math.ceil(totalRows / pageSize));

    // Try to extract source table info for CRUD operations
    const tableInfo = extractTableFromSelect(baseQuery);
    let sourceTable: QueryResult["sourceTable"] | undefined;

    if (tableInfo) {
      const schema = tableInfo.schema || "public";
      const primaryKeys = this.getPrimaryKeysForTable(schema, tableInfo.table);
      if (primaryKeys.length > 0) {
        sourceTable = {
          schema,
          name: tableInfo.table,
          primaryKeys,
        };
      }
    }

    // Generate results
    return {
      columns: (dbResult?.length ?? 0) > 0 ? Object.keys(dbResult[0]) : [],
      rows: dbResult || [],
      rowCount: dbResult?.length ?? 0,
      totalRows,
      executionTime: Math.round(totalMs * 100) / 100,
      queryType,
      sourceTable,
      page,
      pageSize,
      totalPages,
    };
  }

  /**
   * Execute all statements in a query tab.
   */
  async execute(tabId: string, page: number = 1, pageSize?: number): Promise<void> {
    if (!this.state.activeConnectionId) return;

    const connection = this.state.activeConnection;
    const database = connection?.database;
    if (!database) {
      toast.error("Not connected to database. Please reconnect.");
      return;
    }

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    // Mark as executing with proper reactivity
    this.updateQueryTabState(tabId, { isExecuting: true });

    // Get effective page size from the first result or use default
    const effectivePageSize = pageSize ?? tab.results?.[0]?.pageSize ?? this.DEFAULT_PAGE_SIZE;

    // Get database type for parsing
    const dbType = connection?.type ?? "postgres";

    // Parse SQL into individual statements
    const statements = splitSqlStatements(tab.query, dbType);

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

    const results: StatementResult[] = [];

    // Execute each statement
    for (let i = 0; i < statements.length; i++) {
      const stmt = statements[i];
      try {
        const result = await this.executeStatement(stmt.sql, page, effectivePageSize, database);
        results.push({
          ...result,
          statementIndex: i,
          statementSql: stmt.sql,
          isError: false,
        });
      } catch (error) {
        // Continue on error - add error result
        results.push({
          columns: ["Error"],
          rows: [{ Error: this.formatError(error) }],
          rowCount: 1,
          totalRows: 1,
          executionTime: 0,
          page: 1,
          pageSize: 1,
          totalPages: 1,
          statementIndex: i,
          statementSql: stmt.sql,
          error: this.formatError(error),
          isError: true,
        });
      }
    }

    // Update tab with all results
    this.updateQueryTabState(tabId, {
      results,
      activeResultIndex: 0,
      isExecuting: false,
    });

    // Add to history (only on first page to avoid duplicates, use first result's timing)
    if (page === 1 && results.length > 0) {
      this.queryHistory.addToHistory(tab.query, results[0]);
    }
  }

  /**
   * Set the active result tab index.
   */
  setActiveResult(tabId: string, resultIndex: number): void {
    if (!this.state.activeConnectionId) return;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results || resultIndex < 0 || resultIndex >= tab.results.length) return;

    this.updateQueryTabState(tabId, { activeResultIndex: resultIndex });
  }

  /**
   * Navigate to a specific page for a specific result.
   */
  async goToPage(tabId: string, page: number, resultIndex?: number): Promise<void> {
    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId!) || [];
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
    pageSize: number
  ): Promise<void> {
    if (!this.state.activeConnectionId) return;

    const connection = this.state.activeConnection;
    const database = connection?.database;
    if (!database) return;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results || resultIndex >= tab.results.length) return;

    const existingResult = tab.results[resultIndex];

    try {
      const result = await this.executeStatement(existingResult.statementSql, page, pageSize, database);

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
        error: this.formatError(error),
        isError: true,
      };
      this.updateQueryTabState(tabId, { results: newResults });
    }
  }

  /**
   * Set page size and re-execute query.
   */
  async setPageSize(tabId: string, pageSize: number, resultIndex?: number): Promise<void> {
    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId!) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results) return;

    const activeIndex = resultIndex ?? tab.activeResultIndex ?? 0;
    await this.executeStatementAtIndex(tabId, activeIndex, 1, pageSize);
  }

  /**
   * Update a cell value in the database.
   */
  async updateCell(
    tabId: string,
    resultIndex: number,
    rowIndex: number,
    column: string,
    newValue: unknown,
    sourceTable: { schema: string; name: string; primaryKeys: string[] }
  ): Promise<{ success: boolean; error?: string }> {
    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId!) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results || resultIndex >= tab.results.length) {
      return { success: false, error: "No results" };
    }

    const result = tab.results[resultIndex];
    const row = result.rows[rowIndex];
    if (!row) return { success: false, error: "Row not found" };

    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: "No primary key found" };
    }

    // Build parameterized query
    const whereConditions = sourceTable.primaryKeys.map((pk, i) => `"${pk}" = $${i + 2}`);
    const query = `UPDATE "${sourceTable.schema}"."${sourceTable.name}" SET "${column}" = $1 WHERE ${whereConditions.join(" AND ")}`;
    const bindValues = [newValue, ...sourceTable.primaryKeys.map((pk) => row[pk])];

    try {
      await this.state.activeConnection?.database!.execute(query, bindValues);
      // Update the local row data
      row[column] = newValue;
      return { success: true };
    } catch (error) {
      return { success: false, error: this.formatError(error) };
    }
  }

  /**
   * Insert a new row into the database.
   */
  async insertRow(
    sourceTable: { schema: string; name: string },
    values: Record<string, unknown>
  ): Promise<{ success: boolean; error?: string; lastInsertId?: number }> {
    const columns = Object.keys(values);
    if (columns.length === 0) {
      return { success: false, error: "No values provided" };
    }

    const columnNames = columns.map((c) => `"${c}"`).join(", ");
    const placeholders = columns.map((_, i) => `$${i + 1}`).join(", ");
    const query = `INSERT INTO "${sourceTable.schema}"."${sourceTable.name}" (${columnNames}) VALUES (${placeholders})`;

    try {
      const result = await this.state.activeConnection?.database!.execute(query, Object.values(values));
      return { success: true, lastInsertId: result?.lastInsertId };
    } catch (error) {
      return { success: false, error: this.formatError(error) };
    }
  }

  /**
   * Delete a row from the database.
   */
  async deleteRow(
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>
  ): Promise<{ success: boolean; error?: string }> {
    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: "No primary key found" };
    }

    const whereConditions = sourceTable.primaryKeys.map((pk, i) => `"${pk}" = $${i + 1}`);
    const query = `DELETE FROM "${sourceTable.schema}"."${sourceTable.name}" WHERE ${whereConditions.join(" AND ")}`;
    const bindValues = sourceTable.primaryKeys.map((pk) => row[pk]);

    try {
      await this.state.activeConnection?.database!.execute(query, bindValues);
      return { success: true };
    } catch (error) {
      return { success: false, error: this.formatError(error) };
    }
  }
}
