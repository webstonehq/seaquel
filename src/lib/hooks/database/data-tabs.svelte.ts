import type { DataTab, DataFilter, DataSort, SchemaTable, ActiveViewType } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import type { QueryExecutionManager } from "./query-execution.svelte.js";
import type { ProviderRegistry } from "$lib/providers";

// Dialect-specific placeholder syntax and CAST-to-string type for filter
// conditions. MySQL/MariaDB reject `$N` placeholders and `CAST(... AS TEXT)`;
// SQL Server disallows TEXT as a CAST target ("Explicit conversion ... not allowed").
function filterDialect(dbType: string, paramIndex: number): { paramRef: string; textType: string } {
  if (dbType === "mssql") return { paramRef: `@p${paramIndex}`, textType: "NVARCHAR(MAX)" };
  if (dbType === "mysql" || dbType === "mariadb") return { paramRef: "?", textType: "CHAR" };
  return { paramRef: `$${paramIndex}`, textType: "TEXT" };
}

/**
 * Manages data viewer tabs: add, remove, set active.
 * Handles query building with filters, sorting, and pagination.
 */
export class DataTabManager extends BaseTabManager<DataTab> {
  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
    private queryExecution: QueryExecutionManager,
    private providers?: ProviderRegistry,
  ) {
    super(state, tabOrdering, schedulePersistence, setActiveView);
  }

  protected get accessors(): TabStateAccessors<DataTab> {
    return {
      getTabs: () => this.state.dataTabsByProject,
      setTabs: (r) => (this.state.dataTabsByProject = r),
      getActiveId: () => this.state.activeDataTabIdByProject,
      setActiveId: (r) => (this.state.activeDataTabIdByProject = r),
    };
  }

  /**
   * Open a data viewer for a table. Auto-executes the initial query.
   * If initialFilter is provided, the tab opens pre-filtered to that value.
   */
  add(table: SchemaTable, initialFilter?: { column: string; value: string }): string | null {
    const tabId = this.addWithoutRefresh(table, initialFilter);
    if (tabId) void this.refresh(tabId);
    return tabId;
  }

  addWithoutRefresh(
    table: SchemaTable,
    initialFilter?: { column: string; value: string },
  ): string | null {
    if (!this.state.activeProjectId || !this.state.activeConnectionId) return null;

    // Check if already open for this table on the active connection
    const existing = this.getProjectTabs().find(
      (t) =>
        t.connectionId === this.state.activeConnectionId &&
        t.tableName === table.name &&
        t.schemaName === table.schema,
    );
    if (existing) {
      this.setActive(existing.id);
      if (initialFilter) {
        this.setFilters(existing.id, [
          {
            id: crypto.randomUUID(),
            column: initialFilter.column,
            operator: "=",
            value: initialFilter.value,
            enabled: true,
          },
        ]);
      }
      return existing.id;
    }

    const filters: DataFilter[] = initialFilter
      ? [
          {
            id: crypto.randomUUID(),
            column: initialFilter.column,
            operator: "=",
            value: initialFilter.value,
            enabled: true,
          },
        ]
      : [];

    const tab: DataTab = {
      id: `data-${crypto.randomUUID()}`,
      connectionId: this.state.activeConnectionId,
      tableName: table.name,
      schemaName: table.schema,
      filters,
      filterLogic: "AND",
      sortColumns: [],
      page: 1,
      pageSize: 100,
      isLoading: false,
      pendingNewRows: [],
    };

    return this.appendTab(tab);
  }

  /**
   * Re-execute the query with current filters/sort/page.
   */
  async refresh(tabId: string): Promise<void> {
    const tab = this.getProjectTabs().find((t) => t.id === tabId);
    if (!tab) return;

    const connection = this.state.connections.find((c) => c.id === tab.connectionId);
    if (!connection?.providerConnectionId) return;

    this.updateTab(tabId, (t) => ({ ...t, isLoading: true }));

    try {
      const { sql, params } = this.buildQuery(tab, connection.type);
      const { sql: countSql, params: countParams } = this.buildCountQuery(tab, connection.type);

      if (!this.providers) return;
      const provider = await this.providers.getForType(connection.type);

      // Execute count query for total rows
      let totalRows = 0;
      try {
        const countResult = await provider.select<Record<string, number>>(
          connection.providerConnectionId,
          countSql,
          countParams,
        );
        totalRows = Number(Object.values(countResult[0] ?? {})[0] ?? 0);
      } catch {
        // Count query failed, proceed without total
      }

      // Execute data query
      const rowObjects = await provider.select<Record<string, unknown>>(
        connection.providerConnectionId,
        sql,
        params,
      );

      const columns =
        rowObjects.length > 0 ? Object.keys(rowObjects[0]) : this.getTableColumnNames(tab);
      const primaryKeys = this.getTablePrimaryKeys(tab);

      // Convert provider's row-object shape into the columnar `unknown[][]`
      // that `StatementResult.rows` now stores. Data-tab pages are always
      // paginated (≤ pageSize rows), so this is cheap.
      const columnarRows: unknown[][] = rowObjects.map((r) => columns.map((c) => r[c]));

      this.updateTab(tabId, (t) => ({
        ...t,
        isLoading: false,
        totalRows,
        results: {
          columns,
          rows: columnarRows,
          rowCount: columnarRows.length,
          totalRows,
          page: t.page,
          pageSize: t.pageSize,
          totalPages: Math.max(1, Math.ceil(totalRows / t.pageSize)),
          executionTime: 0,
          queryType: "select" as const,
          sourceTable:
            primaryKeys.length > 0
              ? { schema: t.schemaName, name: t.tableName, primaryKeys }
              : undefined,
          statementIndex: 0,
          statementSql: sql,
          isError: false,
        },
      }));
    } catch (error) {
      this.updateTab(tabId, (t) => ({
        ...t,
        isLoading: false,
        results: {
          columns: [],
          rows: [],
          rowCount: 0,
          totalRows: 0,
          page: t.page,
          pageSize: t.pageSize,
          totalPages: 1,
          executionTime: 0,
          queryType: "select" as const,
          statementIndex: 0,
          statementSql: "",
          isError: true,
          error: error instanceof Error ? error.message : String(error),
        },
      }));
    }
  }

  /**
   * Refresh all open data tabs for a given connection.
   */
  async refreshAllForConnection(connectionId: string): Promise<void> {
    const tabs = this.getProjectTabs().filter((t) => t.connectionId === connectionId);
    await Promise.all(tabs.map((tab) => this.refresh(tab.id)));
  }

  /**
   * Update filters and re-execute.
   */
  setFilters(tabId: string, filters: DataFilter[], logic?: "AND" | "OR"): void {
    this.updateTab(tabId, (t) => ({
      ...t,
      filters,
      filterLogic: logic ?? t.filterLogic,
      page: 1,
    }));
    void this.refresh(tabId);
  }

  /**
   * Update sorting and re-execute.
   */
  setSorting(tabId: string, sorts: DataSort[]): void {
    this.updateTab(tabId, (t) => ({ ...t, sortColumns: sorts }));
    void this.refresh(tabId);
  }

  /**
   * Toggle sort on a column (none → ASC → DESC → none).
   */
  toggleSort(tabId: string, column: string): void {
    const tab = this.getProjectTabs().find((t) => t.id === tabId);
    if (!tab) return;

    const existing = tab.sortColumns.find((s) => s.column === column);
    let newSorts: DataSort[];

    if (!existing) {
      newSorts = [{ column, direction: "ASC" }];
    } else if (existing.direction === "ASC") {
      newSorts = [{ column, direction: "DESC" }];
    } else {
      newSorts = [];
    }

    this.setSorting(tabId, newSorts);
  }

  /**
   * Navigate to a specific page.
   */
  setPage(tabId: string, page: number): void {
    this.updateTab(tabId, (t) => ({ ...t, page }));
    void this.refresh(tabId);
  }

  /**
   * Change page size and reset to page 1.
   */
  setPageSize(tabId: string, pageSize: number): void {
    this.updateTab(tabId, (t) => ({ ...t, pageSize, page: 1 }));
    void this.refresh(tabId);
  }

  /**
   * Add an empty pending row for inline editing.
   */
  addNewRow(tabId: string): void {
    this.updateTab(tabId, (t) => ({
      ...t,
      pendingNewRows: [...t.pendingNewRows, {}],
    }));
  }

  /**
   * Save a pending new row via INSERT.
   */
  async saveNewRow(
    tabId: string,
    rowIndex: number,
    values: Record<string, unknown>,
  ): Promise<boolean> {
    const tab = this.getProjectTabs().find((t) => t.id === tabId);
    if (!tab) return false;

    const result = await this.queryExecution.insertRow(
      { schema: tab.schemaName, name: tab.tableName },
      values,
    );

    if (result.success) {
      if (!result.queued) {
        await this.refresh(tabId);
      }
      this.updateTab(tabId, (t) => ({
        ...t,
        pendingNewRows: t.pendingNewRows.filter((_, i) => i !== rowIndex),
      }));
      return true;
    }

    return false;
  }

  /**
   * Cancel a pending new row.
   */
  cancelNewRow(tabId: string, rowIndex: number): void {
    this.updateTab(tabId, (t) => ({
      ...t,
      pendingNewRows: t.pendingNewRows.filter((_, i) => i !== rowIndex),
    }));
  }

  /**
   * Build a SELECT query from the tab's current state.
   */
  buildQuery(tab: DataTab, dbType: string): { sql: string; params: unknown[] } {
    const q = this.quoteIdentifier(dbType);
    const selectClause = this.buildSelectClause(tab, dbType, q);
    const base = `SELECT ${selectClause} FROM ${q(tab.schemaName)}.${q(tab.tableName)}`;
    const params: unknown[] = [];

    // WHERE clause from enabled filters
    const activeFilters = tab.filters.filter((f) => f.enabled && f.column);
    let where = "";
    if (activeFilters.length > 0) {
      const conditions = activeFilters.map((f) => {
        const col = q(f.column);
        if (f.operator === "IS NULL") return `${col} IS NULL`;
        if (f.operator === "IS NOT NULL") return `${col} IS NOT NULL`;
        params.push(f.value);
        const { paramRef, textType } = filterDialect(dbType, params.length);
        return `CAST(${col} AS ${textType}) ${f.operator} ${paramRef}`;
      });
      where = ` WHERE ${conditions.join(` ${tab.filterLogic} `)}`;
    }

    // ORDER BY
    let orderBy = "";
    if (tab.sortColumns.length > 0) {
      const sorts = tab.sortColumns.map((s) => `${q(s.column)} ${s.direction}`);
      orderBy = ` ORDER BY ${sorts.join(", ")}`;
    }

    // PAGINATION
    const offset = (tab.page - 1) * tab.pageSize;
    let pagination: string;
    if (dbType === "mssql") {
      if (!orderBy) orderBy = " ORDER BY (SELECT NULL)";
      pagination = ` OFFSET ${offset} ROWS FETCH NEXT ${tab.pageSize} ROWS ONLY`;
    } else {
      pagination = ` LIMIT ${tab.pageSize} OFFSET ${offset}`;
    }

    return { sql: `${base}${where}${orderBy}${pagination}`, params };
  }

  /**
   * Build a COUNT query for the current filters.
   */
  private buildCountQuery(tab: DataTab, dbType: string): { sql: string; params: unknown[] } {
    const q = this.quoteIdentifier(dbType);
    const base = `SELECT COUNT(*) FROM ${q(tab.schemaName)}.${q(tab.tableName)}`;
    const params: unknown[] = [];

    const activeFilters = tab.filters.filter((f) => f.enabled && f.column);
    if (activeFilters.length === 0) return { sql: base, params };

    const conditions = activeFilters.map((f) => {
      const col = q(f.column);
      if (f.operator === "IS NULL") return `${col} IS NULL`;
      if (f.operator === "IS NOT NULL") return `${col} IS NOT NULL`;
      params.push(f.value);
      const { paramRef, textType } = filterDialect(dbType, params.length);
      return `CAST(${col} AS ${textType}) ${f.operator} ${paramRef}`;
    });

    return { sql: `${base} WHERE ${conditions.join(` ${tab.filterLogic} `)}`, params };
  }

  /**
   * Build the SELECT column clause, handling MSSQL types that hang tiberius.
   *
   * tiberius 0.12.3 panics via `todo!()` when it encounters SQL_VARIANT or UDT
   * columns (token_col_metadata.rs:174/204). The panic aborts the async task
   * and the query future never resolves, so the UI spinner never clears.
   * Work around it by explicitly listing columns and CASTing the problematic
   * ones to NVARCHAR(MAX).
   */
  private buildSelectClause(tab: DataTab, dbType: string, q: (name: string) => string): string {
    if (dbType !== "mssql") return "*";
    const columns = this.getTableColumns(tab);
    if (columns.length === 0) return "*";
    const isTiberiusHanging = (type: string): boolean =>
      /^(sql_variant|geography|geometry|hierarchyid)$/i.test(type.trim());
    if (!columns.some((c) => isTiberiusHanging(c.type))) return "*";
    return columns
      .map((c) =>
        isTiberiusHanging(c.type)
          ? `CAST(${q(c.name)} AS NVARCHAR(MAX)) AS ${q(c.name)}`
          : q(c.name),
      )
      .join(", ");
  }

  /**
   * Get all columns (with type info) for a table from the schema cache.
   */
  private getTableColumns(tab: DataTab): Array<{ name: string; type: string }> {
    const schemas = this.state.schemas[tab.connectionId] ?? [];
    const table = schemas.find((t) => t.name === tab.tableName && t.schema === tab.schemaName);
    return table?.columns.map((c) => ({ name: c.name, type: c.type })) ?? [];
  }

  /**
   * Get all column names for a table from the schema cache.
   */
  private getTableColumnNames(tab: DataTab): string[] {
    return this.getTableColumns(tab).map((c) => c.name);
  }

  /**
   * Get primary key column names for a table from the schema cache.
   */
  private getTablePrimaryKeys(tab: DataTab): string[] {
    const schemas = this.state.schemas[tab.connectionId] ?? [];
    const table = schemas.find((t) => t.name === tab.tableName && t.schema === tab.schemaName);
    return table?.columns.filter((c) => c.isPrimaryKey).map((c) => c.name) ?? [];
  }

  /**
   * Returns a quoting function for the given database type.
   */
  private quoteIdentifier(dbType: string): (name: string) => string {
    if (dbType === "mysql" || dbType === "mariadb") {
      return (name: string) => `\`${name}\``;
    }
    if (dbType === "mssql") {
      return (name: string) => `[${name}]`;
    }
    return (name: string) => `"${name}"`;
  }
}
