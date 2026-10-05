import type {
  ActiveViewType,
  DatabaseConnection,
  DataFilter,
  DataSort,
  DataTab,
  SchemaTable,
  StatementResult,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import type { QueryExecutionManager } from "./query-execution.svelte.js";
import type { ProviderRegistry } from "$lib/providers";
import { rowToObject, dedupeColumnNames } from "$lib/utils/row-access";
import { decodeRows } from "$lib/values";
import { CANCELLED } from "$lib/core/client";
import { log } from "$lib/utils/logger";
import { callErrorText, errorText } from "./error-text.js";
import { getEditService, type EditService, type TableQuery } from "./edit-service/index.js";

type EditResult = { success: boolean; error?: string; queued?: boolean };

/** A data tab's refresh in flight: a new one, closing the tab or a project reload cancels it. */
interface Refresh {
  controller: AbortController;
  projectId: string;
}

/**
 * Data viewer tabs: one table's rows, filtered, sorted and paged, with
 * inline edits.
 *
 * A page is `db.tablePage` through the connection's `EditService` (phase 5c):
 * the tab sends its table, enabled filters, logic, sort,
 * page and page size, and Core quotes, binds, casts, pages and counts. A tab
 * has one refresh at a time: a new one cancels the one in flight (so pages
 * clicked quickly can't land out of order), and closing the tab or reloading
 * its project cancels it too.
 */
export class DataTabManager extends BaseTabManager<DataTab> {
  /** Each tab's refresh in flight. */
  private refreshes = new Map<string, Refresh>();

  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
    private queryExecution: QueryExecutionManager,
    private providers?: ProviderRegistry,
    private editServiceFor?: (connection: DatabaseConnection) => Promise<EditService>,
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

  override remove(id: string): void {
    this.cancel(id);
    super.remove(id);
  }

  /** Cancel the tab's refresh in flight, in Core too. */
  cancel(tabId: string): void {
    this.refreshes.get(tabId)?.controller.abort();
    this.refreshes.delete(tabId);
  }

  /** A project's tabs are about to be replaced or were deleted: cancel their refreshes. */
  cancelProject(projectId: string): void {
    for (const [tabId, refresh] of this.refreshes) {
      if (refresh.projectId === projectId) this.cancel(tabId);
    }
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

  /** A tab of `projectId`, whatever project is active now. */
  private tabIn(projectId: string, tabId: string): DataTab | undefined {
    return this.state.dataTabsByProject[projectId]?.find((t) => t.id === tabId);
  }

  /** Update a tab of `projectId`, whatever project is active now. */
  private updateTabIn(projectId: string, tabId: string, updater: (tab: DataTab) => DataTab): void {
    const tabs = this.state.dataTabsByProject[projectId];
    if (!tabs?.some((t) => t.id === tabId)) return;
    this.state.dataTabsByProject = {
      ...this.state.dataTabsByProject,
      [projectId]: tabs.map((t) => (t.id === tabId ? updater(t) : t)),
    };
  }

  private serviceFor(connection: DatabaseConnection): Promise<EditService> {
    if (this.editServiceFor) return this.editServiceFor(connection);
    return getEditService(connection, this.state, this.providers!);
  }

  /** The tab's query as `db.tablePage` takes it: enabled filters with a column only. */
  private tableQuery(tab: DataTab): TableQuery {
    return {
      target: { schema: tab.schemaName, table: tab.tableName },
      filters: tab.filters
        .filter((f) => f.enabled && f.column)
        .map((f) => ({ column: f.column, op: f.operator, value: f.value })),
      logic: tab.filterLogic,
      sort: tab.sortColumns.map((s) => ({ column: s.column, direction: s.direction })),
    };
  }

  /**
   * Load the tab's page with its filters, sort and page, on the tab's own
   * connection. Cancels the tab's refresh in flight; only the latest one
   * updates the tab.
   */
  async refresh(tabId: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    const tab = projectId ? this.tabIn(projectId, tabId) : undefined;
    if (!projectId || !tab) return;

    this.refreshes.get(tabId)?.controller.abort();
    const op: Refresh = { controller: new AbortController(), projectId };
    this.refreshes.set(tabId, op);
    const current = () => this.refreshes.get(tabId) === op && !op.controller.signal.aborted;

    const lookUp = () => this.state.connections.find((c) => c.id === tab.connectionId);
    const connection = lookUp();
    if (!connection?.providerConnectionId || !(this.providers || this.editServiceFor)) {
      this.refreshes.delete(tabId);
      if (tab.isLoading) this.updateTabIn(projectId, tabId, (t) => ({ ...t, isLoading: false }));
      return;
    }

    this.updateTabIn(projectId, tabId, (t) => ({ ...t, isLoading: true }));
    const query = this.tableQuery(tab);
    const { page, pageSize } = tab;

    let columns: string[] | null = null;
    let rows: unknown[][] = [];
    let statementSql = "";
    let totalRows = 0;
    let totalPages = 1;
    let countEstimated = false;
    let executionTime = 0;
    let error: string | null = null;
    let finished = false;
    try {
      const service = await this.serviceFor(connection);
      // After the await: a reconnect gives a new Core id, a disconnect none.
      const now = lookUp();
      if (!now?.providerConnectionId || now.type !== connection.type) {
        throw new Error("No connection established");
      }
      const events = service.tablePage(
        {
          connectionId: now.providerConnectionId,
          streamId: crypto.randomUUID(),
          query,
          page,
          pageSize,
        },
        op.controller.signal,
      );
      for await (const event of events) {
        if (!current()) continue;
        switch (event.type) {
          case "statementStart":
            statementSql = event.source.sql;
            break;
          case "batch":
            if (event.columns) columns = dedupeColumnNames(event.columns);
            rows = rows.concat(decodeRows(event.rows));
            break;
          case "statementDone":
            totalRows = event.totalRows;
            totalPages = event.totalPages;
            countEstimated = event.countEstimated;
            executionTime = event.elapsedMs;
            finished = true;
            break;
          case "statementError":
            error = errorText(event.code, event.message);
            break;
          case "error":
            if (event.code !== CANCELLED) error = errorText(event.code, event.message);
            break;
          default:
            break;
        }
      }
    } catch (e) {
      error = callErrorText(e);
    } finally {
      if (this.refreshes.get(tabId) === op) this.refreshes.delete(tabId);
    }
    if (op.controller.signal.aborted) return;
    if (!error && !finished) error = "The page ended without an answer";
    if (error) void log.warn(`Data tab page on ${tab.connectionId} failed`);

    const latest = this.tabIn(projectId, tabId);
    if (!latest) return;
    const shownColumns = columns ?? this.getTableColumnNames(latest);
    const primaryKeys = this.getTablePrimaryKeys(latest);
    const results: StatementResult = error
      ? {
          columns: [],
          rows: [],
          rowCount: 0,
          totalRows: 0,
          page,
          pageSize,
          totalPages: 1,
          executionTime: 0,
          queryType: "select",
          statementIndex: 0,
          statementSql,
          connectionId: latest.connectionId,
          isError: true,
          error,
        }
      : {
          columns: shownColumns,
          rows,
          rowCount: rows.length,
          totalRows,
          page,
          pageSize,
          totalPages,
          ...(countEstimated ? { countEstimated: true } : {}),
          executionTime,
          queryType: "select",
          sourceTable:
            primaryKeys.length > 0
              ? { schema: latest.schemaName, name: latest.tableName, primaryKeys }
              : undefined,
          statementIndex: 0,
          statementSql,
          connectionId: latest.connectionId,
          isError: false,
        };
    this.updateTabIn(projectId, tabId, (t) => ({
      ...t,
      isLoading: false,
      totalRows: results.totalRows,
      results,
    }));
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
   * The row at `rowIndex` of the tab's page and the table it came from, for
   * an edit. Edits always go to the tab's own connection, whatever is active
   * in the sidebar.
   */
  private editTarget(tabId: string, rowIndex: number) {
    const tab = this.getProjectTabs().find((t) => t.id === tabId);
    const results = tab?.results;
    const row = results?.rows[rowIndex];
    if (!tab || !results?.sourceTable || !row) return null;
    return {
      tab,
      sourceTable: results.sourceTable,
      row: rowToObject(row, results.columns),
    };
  }

  /** After an edit that ran (not queued), reload the page to show it. */
  private refreshAfter(tabId: string, result: EditResult): EditResult {
    if (result.success && !result.queued) void this.refresh(tabId);
    return result;
  }

  /** Set one cell of the tab's page, on the tab's connection. */
  async updateCell(
    tabId: string,
    rowIndex: number,
    column: string,
    newValue: unknown,
  ): Promise<EditResult> {
    const target = this.editTarget(tabId, rowIndex);
    if (!target) return { success: false, error: "Row not found" };
    const result = await this.queryExecution.updateCellDirect(
      target.tab.connectionId,
      target.sourceTable,
      target.row,
      column,
      newValue,
    );
    return this.refreshAfter(tabId, result);
  }

  /** Set one cell of the tab's page to its column's default, on the tab's connection. */
  async setCellDefault(tabId: string, rowIndex: number, column: string): Promise<EditResult> {
    const target = this.editTarget(tabId, rowIndex);
    if (!target) return { success: false, error: "Row not found" };
    const result = await this.queryExecution.setCellDefaultDirect(
      target.tab.connectionId,
      target.sourceTable,
      target.row,
      column,
    );
    return this.refreshAfter(tabId, result);
  }

  /** Delete a row of the tab's page (columnar, as the grid holds it), on the tab's connection. */
  async deleteRow(tabId: string, row: unknown[]): Promise<EditResult> {
    const tab = this.getProjectTabs().find((t) => t.id === tabId);
    if (!tab?.results?.sourceTable) return { success: false, error: "Row not found" };
    const result = await this.queryExecution.deleteRow(
      tab.connectionId,
      tab.results.sourceTable,
      rowToObject(row, tab.results.columns),
    );
    return this.refreshAfter(tabId, result);
  }

  /**
   * Save a pending new row via INSERT, on the tab's connection.
   */
  async saveNewRow(
    tabId: string,
    rowIndex: number,
    values: Record<string, unknown>,
  ): Promise<boolean> {
    const tab = this.getProjectTabs().find((t) => t.id === tabId);
    if (!tab) return false;

    const result = await this.queryExecution.insertRow(
      tab.connectionId,
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
   * Get all columns (with type info) for a table from the schema cache.
   */
  private getTableColumns(tab: DataTab): Array<{ name: string; type: string }> {
    const schemas = this.state.schemas[tab.connectionId] ?? [];
    const table = schemas.find((t) => t.name === tab.tableName && t.schema === tab.schemaName);
    return table?.columns.map((c) => ({ name: c.name, type: c.type })) ?? [];
  }

  /**
   * Get all column names for a table from the schema cache (an empty page
   * that brought none).
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
}
