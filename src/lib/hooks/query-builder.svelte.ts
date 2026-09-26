import { setContext, getContext } from "svelte";
import { SvelteSet } from "svelte/reactivity";
import type {
  CanvasTable,
  CanvasJoin,
  FilterCondition,
  FilterOperator,
  SortCondition,
  SortDirection,
  GroupByCondition,
  HavingCondition,
  HavingOperator,
  AggregateFunction,
  JoinType,
  QueryBuilderSnapshot,
  SelectAggregate,
  ColumnAggregate,
  DisplayAggregate,
  CanvasSubquery,
  SubqueryRole,
  CanvasCTE,
  QueryBuilderTable,
  DatabaseType,
} from "$lib/types";
import { TUTORIAL_SCHEMA } from "$lib/tutorial/schema";
import { tutorialToQueryBuilder, getQueryBuilderTable } from "$lib/utils/schema-adapter";
import { buildSql as generateSqlFromState } from "./query-builder-sql";
import {
  serializeQueryBuilderState,
  deserializeQueryBuilderState,
  type SerializableQueryBuilderState,
} from "./query-builder-serialization";
import { applyParsedSqlToState } from "./query-builder-parsed-sql";
import type { ParsedQuery } from "$lib/sql";
import {
  cloneSubqueries,
  cloneCtes,
  findSubqueryById as findSubqueryByIdPure,
  findParentSubquery as findParentSubqueryPure,
  updateSubqueryInArray,
  createSubquery,
  createCanvasTableForContainer,
  removeTableFromInnerQuery,
  toggleColumnInTable,
  addSelectAggregateToInnerQuery,
  createCteReferenceTable,
} from "./query-builder-subqueries";
import { createCte, getCteColumns as getCteColumnsPure } from "./query-builder-ctes";
import { computeDisplayAggregates } from "./query-builder-aggregates";
import { computeTableRemovalCleanup } from "./query-builder-tables";
import {
  createFilter,
  createGroupBy,
  createHaving,
  createOrderBy,
  createSelectAggregate,
  createActiveSubquery,
  setColumnAggregateOnTable,
  addItemToArray,
  updateItemInArray,
  removeItemFromArray,
} from "./query-builder-active-context";

/**
 * Generates a unique ID for canvas elements.
 */
function generateId(): string {
  return crypto.randomUUID();
}

/**
 * QueryBuilderState manages the state for the interactive SQL query builder.
 * Uses Svelte 5 runes for reactivity.
 */
export class QueryBuilderState {
  // === STATE PROPERTIES ===

  /**
   * The engine whose dialect the builder's SQL is parsed in: the connection's
   * for the Visual panel, so backtick and bracket names round-trip (fix 9).
   * The tutorial keeps the default, `postgres`.
   */
  engine = $state<DatabaseType>("postgres");

  /** Schema tables available for the query builder. Defaults to tutorial schema. */
  schema = $state<QueryBuilderTable[]>(tutorialToQueryBuilder(TUTORIAL_SCHEMA));

  /** Tables placed on the canvas */
  tables = $state<CanvasTable[]>([]);

  /** Joins between tables */
  joins = $state<CanvasJoin[]>([]);

  /** WHERE clause conditions */
  filters = $state<FilterCondition[]>([]);

  /** GROUP BY columns */
  groupBy = $state<GroupByCondition[]>([]);

  /** HAVING clause conditions */
  having = $state<HavingCondition[]>([]);

  /** ORDER BY clauses */
  orderBy = $state<SortCondition[]>([]);

  /** LIMIT value, or null for no limit. Can be a {{variable}} string. */
  limit = $state<string | number | null>(100);

  /** User's custom SQL text (preserved even if it differs from generated) */
  customSql = $state<string | null>(null);

  /** Standalone aggregates in SELECT clause */
  selectAggregates = $state<SelectAggregate[]>([]);

  /** Subqueries on the canvas */
  subqueries = $state<CanvasSubquery[]>([]);

  /** CTEs (Common Table Expressions) on the canvas */
  ctes = $state<CanvasCTE[]>([]);

  /** Currently selected subquery ID (null = top-level query) */
  selectedSubqueryId = $state<string | null>(null);

  /** Currently selected CTE ID for editing (null = none selected) */
  selectedCteId = $state<string | null>(null);

  // === DERIVED PROPERTIES ===

  /**
   * The currently selected subquery, or null if top-level query is selected.
   * Uses recursive search to support nested subqueries.
   */
  selectedSubquery = $derived.by(() => {
    if (!this.selectedSubqueryId) return null;
    return this.findSubqueryById(this.selectedSubqueryId) ?? null;
  });

  /**
   * The currently selected CTE, or null if none selected.
   */
  selectedCte = $derived.by(() => {
    if (!this.selectedCteId) return null;
    return this.ctes.find((c) => c.id === this.selectedCteId) ?? null;
  });

  /**
   * Active filters - from selected CTE, subquery, or top-level.
   */
  activeFilters = $derived.by(() => {
    if (this.selectedCte) return this.selectedCte.innerQuery.filters;
    return this.selectedSubquery?.innerQuery.filters ?? this.filters;
  });

  /**
   * Active groupBy - from selected CTE, subquery, or top-level.
   */
  activeGroupBy = $derived.by(() => {
    if (this.selectedCte) return this.selectedCte.innerQuery.groupBy;
    return this.selectedSubquery?.innerQuery.groupBy ?? this.groupBy;
  });

  /**
   * Active having - from selected CTE, subquery, or top-level.
   */
  activeHaving = $derived.by(() => {
    if (this.selectedCte) return this.selectedCte.innerQuery.having;
    return this.selectedSubquery?.innerQuery.having ?? this.having;
  });

  /**
   * Active orderBy - from selected CTE, subquery, or top-level.
   */
  activeOrderBy = $derived.by(() => {
    if (this.selectedCte) return this.selectedCte.innerQuery.orderBy;
    return this.selectedSubquery?.innerQuery.orderBy ?? this.orderBy;
  });

  /**
   * Active limit - from selected CTE, subquery, or top-level.
   */
  activeLimit = $derived.by(() => {
    if (this.selectedCte) return this.selectedCte.innerQuery.limit;
    return this.selectedSubquery?.innerQuery.limit ?? this.limit;
  });

  /**
   * Active select aggregates - from selected CTE, subquery, or top-level.
   */
  activeSelectAggregates = $derived.by(() => {
    if (this.selectedCte) return this.selectedCte.innerQuery.selectAggregates;
    return this.selectedSubquery?.innerQuery.selectAggregates ?? this.selectAggregates;
  });

  /**
   * Active tables - from selected CTE, subquery, or top-level.
   */
  activeTables = $derived.by(() => {
    if (this.selectedCte) return this.selectedCte.innerQuery.tables;
    return this.selectedSubquery?.innerQuery.tables ?? this.tables;
  });

  /**
   * Active display aggregates - unified format from active context.
   */
  activeDisplayAggregates = $derived.by((): DisplayAggregate[] =>
    computeDisplayAggregates(this.activeTables, this.activeSelectAggregates),
  );

  /**
   * Generated SQL query from the current canvas state.
   */
  generatedSql = $derived.by(() => {
    return this.buildSql();
  });

  /**
   * Get a snapshot of the current query builder state.
   * Useful for challenge validation.
   */
  get snapshot(): QueryBuilderSnapshot {
    return {
      tables: this.tables.map((t) => ({
        ...t,
        selectedColumns: new Set(t.selectedColumns),
        columnAggregates: new Map(t.columnAggregates),
      })),
      joins: [...this.joins],
      filters: [...this.filters],
      groupBy: [...this.groupBy],
      having: [...this.having],
      orderBy: [...this.orderBy],
      limit: this.limit,
      selectAggregates: [...this.selectAggregates],
      subqueries: cloneSubqueries(this.subqueries),
      ctes: cloneCtes(this.ctes),
    };
  }

  // Clone helpers delegated to query-builder-subqueries.ts

  // === TABLE MANAGEMENT ===

  /**
   * Add a table to the canvas.
   * @param tableName - Name of the table from the schema
   * @param position - Position on the canvas
   * @returns The created canvas table, or undefined if table not found in schema
   */
  addTable(tableName: string, position: { x: number; y: number }): CanvasTable | undefined {
    const tableSchema = this.getSchemaTable(tableName);
    if (!tableSchema) {
      return undefined;
    }

    const canvasTable: CanvasTable = {
      id: generateId(),
      tableName,
      position,
      selectedColumns: new SvelteSet<string>(),
      columnAggregates: new Map<string, ColumnAggregate>(),
    };

    this.tables = [...this.tables, canvasTable];
    this.customSql = null; // Clear custom SQL so editor syncs with visual state
    return canvasTable;
  }

  /**
   * Remove a table from the canvas.
   * Also removes associated joins, filters, and order by clauses.
   * @param tableId - ID of the canvas table to remove
   */
  removeTable(tableId: string): void {
    const table = this.tables.find((t) => t.id === tableId);
    if (!table) return;

    const cleanup = computeTableRemovalCleanup(
      table.tableName,
      this.joins,
      this.filters,
      this.orderBy,
    );
    this.joins = cleanup.joins;
    this.filters = cleanup.filters;
    this.orderBy = cleanup.orderBy;

    this.tables = this.tables.filter((t) => t.id !== tableId);
    this.customSql = null; // Clear custom SQL so editor syncs with visual state
  }

  /**
   * Update a table's position on the canvas.
   * @param tableId - ID of the canvas table
   * @param position - New position
   */
  updateTablePosition(tableId: string, position: { x: number; y: number }): void {
    this.tables = this.tables.map((t) => (t.id === tableId ? { ...t, position } : t));
  }

  /**
   * Toggle a column's selection state.
   * @param tableId - ID of the canvas table
   * @param columnName - Name of the column to toggle
   */
  toggleColumn(tableId: string, columnName: string): void {
    const table = this.tables.find((t) => t.id === tableId);
    if (!table) return;

    if (table.selectedColumns.has(columnName)) {
      table.selectedColumns.delete(columnName);
      // Clear any aggregate on this column when deselected
      table.columnAggregates.delete(columnName);
    } else {
      table.selectedColumns.add(columnName);
    }
    // Trigger reactivity by reassigning the tables array
    this.tables = [...this.tables];
    this.customSql = null; // Clear custom SQL so editor syncs with visual state
  }

  /**
   * Select all columns in a table.
   * @param tableId - ID of the canvas table
   */
  selectAllColumns(tableId: string): void {
    const table = this.tables.find((t) => t.id === tableId);
    if (!table) return;

    const tableSchema = this.getSchemaTable(table.tableName);
    if (!tableSchema) return;

    for (const column of tableSchema.columns) {
      table.selectedColumns.add(column.name);
    }
    // Trigger reactivity by reassigning the tables array
    this.tables = [...this.tables];
    this.customSql = null; // Clear custom SQL so editor syncs with visual state
  }

  /**
   * Clear all selected columns in a table.
   * @param tableId - ID of the canvas table
   */
  clearColumns(tableId: string): void {
    const table = this.tables.find((t) => t.id === tableId);
    if (!table) return;

    table.selectedColumns.clear();
    // Trigger reactivity by reassigning the tables array
    this.tables = [...this.tables];
    this.customSql = null; // Clear custom SQL so editor syncs with visual state
  }

  // === JOIN MANAGEMENT ===

  /**
   * Add a join between two tables.
   * @param sourceTable - Name of the source table
   * @param sourceColumn - Column in the source table
   * @param targetTable - Name of the target table
   * @param targetColumn - Column in the target table
   * @param joinType - Type of join (INNER, LEFT, RIGHT, FULL)
   * @returns The created join
   */
  addJoin(
    sourceTable: string,
    sourceColumn: string,
    targetTable: string,
    targetColumn: string,
    joinType: JoinType = "INNER",
  ): CanvasJoin {
    const join: CanvasJoin = {
      id: generateId(),
      sourceTable,
      sourceColumn,
      targetTable,
      targetColumn,
      joinType,
    };

    this.joins = [...this.joins, join];
    this.customSql = null; // Clear custom SQL so editor syncs with visual state
    return join;
  }

  /**
   * Update the type of an existing join.
   * @param joinId - ID of the join
   * @param joinType - New join type
   */
  updateJoinType(joinId: string, joinType: JoinType): void {
    this.joins = this.joins.map((j) => (j.id === joinId ? { ...j, joinType } : j));
    this.customSql = null; // Clear custom SQL so editor syncs with visual state
  }

  updateJoinColumns(
    joinId: string,
    sourceTable: string,
    sourceColumn: string,
    targetTable: string,
    targetColumn: string,
  ): void {
    this.joins = this.joins.map((j) =>
      j.id === joinId ? { ...j, sourceTable, sourceColumn, targetTable, targetColumn } : j,
    );
    this.customSql = null;
  }

  /**
   * Remove a join.
   * @param joinId - ID of the join to remove
   */
  removeJoin(joinId: string): void {
    this.joins = this.joins.filter((j) => j.id !== joinId);
    this.customSql = null; // Clear custom SQL so editor syncs with visual state
  }

  // === COLUMN AGGREGATE MANAGEMENT ===

  /**
   * Set an aggregate function on a selected column.
   * @param tableId - ID of the canvas table
   * @param column - Column name
   * @param func - Aggregate function, or null to clear
   * @param alias - Optional alias for AS clause
   */
  setColumnAggregate(
    tableId: string,
    column: string,
    func: AggregateFunction | null,
    alias?: string,
  ): void {
    const table = this.tables.find((t) => t.id === tableId);
    if (!table) return;

    if (func === null) {
      table.columnAggregates.delete(column);
    } else {
      table.columnAggregates.set(column, { function: func, alias });
    }
    // Trigger reactivity
    this.tables = [...this.tables];
    this.customSql = null;
  }

  /**
   * Clear aggregate from a column.
   * @param tableId - ID of the canvas table
   * @param column - Column name
   */
  clearColumnAggregate(tableId: string, column: string): void {
    this.setColumnAggregate(tableId, column, null);
  }

  // === SUBQUERY MANAGEMENT ===
  // Core logic delegated to query-builder-subqueries.ts

  /**
   * Recursively find a subquery by ID in the subquery tree.
   */
  findSubqueryById(
    subqueryId: string,
    subqueries: CanvasSubquery[] = this.subqueries,
  ): CanvasSubquery | undefined {
    return findSubqueryByIdPure(subqueryId, subqueries);
  }

  /**
   * Recursively update a subquery in the tree.
   */
  private updateSubqueryRecursive(
    subqueryId: string,
    updater: (sq: CanvasSubquery) => Partial<CanvasSubquery>,
  ): void {
    updateSubqueryInArray(this.subqueries, subqueryId, updater);
    this.subqueries = [...this.subqueries];
  }

  /**
   * Add a subquery to the canvas.
   */
  addSubquery(
    role: SubqueryRole,
    position: { x: number; y: number },
    linkedFilterId?: string,
  ): CanvasSubquery {
    const subquery = createSubquery(role, position, linkedFilterId);
    this.subqueries = [...this.subqueries, subquery];
    this.customSql = null;
    return subquery;
  }

  /**
   * Remove a subquery from the canvas (supports nested subqueries).
   * Also cleans up any filters linked to this subquery.
   */
  removeSubquery(subqueryId: string): void {
    const subquery = this.findSubqueryById(subqueryId);
    if (!subquery) return;

    if (subquery.linkedFilterId) {
      this.filters = this.filters.map((f) =>
        f.id === subquery.linkedFilterId ? { ...f, subqueryId: undefined } : f,
      );
    }

    const parent = findParentSubqueryPure(subqueryId, this.subqueries);
    if (parent) {
      parent.innerQuery.subqueries = parent.innerQuery.subqueries.filter(
        (s) => s.id !== subqueryId,
      );
      this.subqueries = [...this.subqueries];
    } else {
      this.subqueries = this.subqueries.filter((s) => s.id !== subqueryId);
    }
    this.customSql = null;
  }

  updateSubqueryPosition(subqueryId: string, position: { x: number; y: number }): void {
    this.updateSubqueryRecursive(subqueryId, () => ({ position }));
  }

  updateSubquerySize(subqueryId: string, size: { width: number; height: number }): void {
    this.updateSubqueryRecursive(subqueryId, () => ({ size }));
    this.customSql = null;
  }

  updateSubqueryRole(subqueryId: string, role: SubqueryRole): void {
    this.updateSubqueryRecursive(subqueryId, () => ({ role }));
    this.customSql = null;
  }

  updateSubqueryAlias(subqueryId: string, alias: string): void {
    this.updateSubqueryRecursive(subqueryId, () => ({ alias: alias || undefined }));
    this.customSql = null;
  }

  linkSubqueryToFilter(subqueryId: string, filterId: string): void {
    this.updateSubqueryRecursive(subqueryId, () => ({ linkedFilterId: filterId }));
    this.customSql = null;
  }

  getSubquery(subqueryId: string): CanvasSubquery | undefined {
    return this.findSubqueryById(subqueryId);
  }

  /**
   * Add a table to a subquery's inner query.
   * Auto-resizes the subquery container if the table doesn't fit.
   */
  addTableToSubquery(
    subqueryId: string,
    tableName: string,
    position: { x: number; y: number },
  ): CanvasTable | undefined {
    if (!this.getSchemaTable(tableName)) return undefined;
    const subquery = this.findSubqueryById(subqueryId);
    if (!subquery) return undefined;

    const { table, newSize } = createCanvasTableForContainer(tableName, position, subquery.size);
    if (newSize) subquery.size = newSize;
    subquery.innerQuery.tables = [...subquery.innerQuery.tables, table];
    this.subqueries = [...this.subqueries];
    this.customSql = null;
    return table;
  }

  /**
   * Remove a table from a subquery's inner query (supports nested).
   */
  removeTableFromSubquery(subqueryId: string, tableId: string): void {
    const subquery = this.findSubqueryById(subqueryId);
    if (!subquery) return;

    removeTableFromInnerQuery(subquery.innerQuery, tableId);
    this.subqueries = [...this.subqueries];
    this.customSql = null;
  }

  /**
   * Toggle a column selection in a subquery table (supports nested).
   */
  toggleSubqueryColumn(subqueryId: string, tableId: string, columnName: string): void {
    const subquery = this.findSubqueryById(subqueryId);
    if (!subquery) return;

    if (toggleColumnInTable(subquery.innerQuery, tableId, columnName)) {
      this.subqueries = [...this.subqueries];
      this.customSql = null;
    }
  }

  /**
   * Add a select aggregate to a subquery (supports nested).
   */
  addSubquerySelectAggregate(
    subqueryId: string,
    func: AggregateFunction,
    expression: string,
    alias?: string,
  ): string | undefined {
    const subquery = this.findSubqueryById(subqueryId);
    if (!subquery) return undefined;

    const id = addSelectAggregateToInnerQuery(subquery.innerQuery, func, expression, alias);
    this.subqueries = [...this.subqueries];
    this.customSql = null;
    return id;
  }

  // === ACTIVE CONTEXT METHODS ===
  // These methods operate on CTE, subquery, or top-level query.
  // Core operations delegated to query-builder-active-context.ts.

  /**
   * Apply an operation to the active clause array (CTE → subquery → top-level).
   * Returns the clause key name for the inner query, enabling generic routing.
   */
  private applyToActiveClause<K extends keyof import("$lib/types").SubqueryInnerState>(
    clause: K,
    updater: (
      arr: import("$lib/types").SubqueryInnerState[K],
    ) => import("$lib/types").SubqueryInnerState[K],
  ): void {
    if (this.selectedCte) {
      this.selectedCte.innerQuery[clause] = updater(this.selectedCte.innerQuery[clause]);
      this.ctes = [...this.ctes];
    } else if (this.selectedSubquery) {
      this.selectedSubquery.innerQuery[clause] = updater(this.selectedSubquery.innerQuery[clause]);
      this.subqueries = [...this.subqueries];
    } else {
      // Top-level: cast is safe because clause keys match class property names
      (this as any)[clause] = updater((this as any)[clause]);
    }
    this.customSql = null;
  }

  addActiveFilter(
    column: string,
    operator: FilterOperator,
    value: string,
    connector: "AND" | "OR" = "AND",
  ): FilterCondition {
    const filter = createFilter(column, operator, value, connector);
    this.applyToActiveClause("filters", (arr) => addItemToArray(arr, filter));
    return filter;
  }

  updateActiveFilter(filterId: string, updates: Partial<Omit<FilterCondition, "id">>): void {
    this.applyToActiveClause("filters", (arr) => updateItemInArray(arr, filterId, updates));
  }

  removeActiveFilter(filterId: string): void {
    this.applyToActiveClause("filters", (arr) => removeItemFromArray(arr, filterId));
  }

  addActiveGroupBy(column: string): GroupByCondition {
    const groupBy = createGroupBy(column);
    this.applyToActiveClause("groupBy", (arr) => addItemToArray(arr, groupBy));
    return groupBy;
  }

  updateActiveGroupBy(groupById: string, column: string): void {
    this.applyToActiveClause("groupBy", (arr) => updateItemInArray(arr, groupById, { column }));
  }

  removeActiveGroupBy(groupById: string): void {
    this.applyToActiveClause("groupBy", (arr) => removeItemFromArray(arr, groupById));
  }

  addActiveHaving(
    aggregateFunction: AggregateFunction,
    column: string,
    operator: HavingOperator,
    value: string,
    connector: "AND" | "OR" = "AND",
  ): HavingCondition {
    const having = createHaving(aggregateFunction, column, operator, value, connector);
    this.applyToActiveClause("having", (arr) => addItemToArray(arr, having));
    return having;
  }

  updateActiveHaving(havingId: string, updates: Partial<Omit<HavingCondition, "id">>): void {
    this.applyToActiveClause("having", (arr) => updateItemInArray(arr, havingId, updates));
  }

  removeActiveHaving(havingId: string): void {
    this.applyToActiveClause("having", (arr) => removeItemFromArray(arr, havingId));
  }

  addActiveOrderBy(column: string, direction: SortDirection = "ASC"): SortCondition {
    const orderBy = createOrderBy(column, direction);
    this.applyToActiveClause("orderBy", (arr) => addItemToArray(arr, orderBy));
    return orderBy;
  }

  updateActiveOrderBy(orderId: string, direction: SortDirection): void {
    this.applyToActiveClause("orderBy", (arr) => updateItemInArray(arr, orderId, { direction }));
  }

  updateActiveOrderByColumn(orderId: string, column: string): void {
    this.applyToActiveClause("orderBy", (arr) => updateItemInArray(arr, orderId, { column }));
  }

  removeActiveOrderBy(orderId: string): void {
    this.applyToActiveClause("orderBy", (arr) => removeItemFromArray(arr, orderId));
  }

  setActiveLimit(limit: string | number | null): void {
    this.applyToActiveClause("limit", () => limit);
  }

  addActiveSelectAggregate(func: AggregateFunction, expression: string, alias?: string): string {
    const aggregate = createSelectAggregate(func, expression, alias);
    this.applyToActiveClause("selectAggregates", (arr) => addItemToArray(arr, aggregate));
    return aggregate.id;
  }

  updateActiveSelectAggregate(id: string, updates: Partial<Omit<SelectAggregate, "id">>): void {
    this.applyToActiveClause("selectAggregates", (arr) => updateItemInArray(arr, id, updates));
  }

  removeActiveSelectAggregate(id: string): void {
    this.applyToActiveClause("selectAggregates", (arr) => removeItemFromArray(arr, id));
  }

  setActiveColumnAggregate(
    tableId: string,
    column: string,
    func: AggregateFunction | null,
    alias?: string,
  ): void {
    if (!setColumnAggregateOnTable(this.activeTables, tableId, column, func, alias)) return;

    if (this.selectedCte) {
      this.ctes = [...this.ctes];
    } else if (this.selectedSubquery) {
      this.subqueries = [...this.subqueries];
    } else {
      this.tables = [...this.tables];
    }
    this.customSql = null;
  }

  clearActiveColumnAggregate(tableId: string, column: string): void {
    this.setActiveColumnAggregate(tableId, column, null);
  }

  addActiveSubquery(
    role: SubqueryRole,
    position: { x: number; y: number },
    linkedFilterId?: string,
  ): CanvasSubquery {
    const subquery = createActiveSubquery(role, position, linkedFilterId);

    if (this.selectedSubquery) {
      this.selectedSubquery.innerQuery.subqueries = [
        ...this.selectedSubquery.innerQuery.subqueries,
        subquery,
      ];
      this.subqueries = [...this.subqueries];
    } else {
      this.subqueries = [...this.subqueries, subquery];
    }
    this.customSql = null;
    return subquery;
  }

  get activeSubqueries(): CanvasSubquery[] {
    return this.selectedSubquery?.innerQuery.subqueries ?? this.subqueries;
  }

  removeActiveSubquery(subqueryId: string): void {
    if (this.selectedSubquery) {
      const subquery = this.selectedSubquery.innerQuery.subqueries.find((s) => s.id === subqueryId);
      if (subquery?.linkedFilterId) {
        const filter = this.selectedSubquery.innerQuery.filters.find(
          (f) => f.id === subquery.linkedFilterId,
        );
        if (filter) {
          this.updateActiveFilter(filter.id, { subqueryId: undefined });
        }
      }
      this.selectedSubquery.innerQuery.subqueries =
        this.selectedSubquery.innerQuery.subqueries.filter((s) => s.id !== subqueryId);
      this.subqueries = [...this.subqueries];
    } else {
      this.removeSubquery(subqueryId);
    }
    this.customSql = null;
  }

  linkActiveSubqueryToFilter(subqueryId: string, filterId: string): void {
    if (this.selectedSubquery) {
      this.selectedSubquery.innerQuery.subqueries = this.selectedSubquery.innerQuery.subqueries.map(
        (s) => (s.id === subqueryId ? { ...s, linkedFilterId: filterId } : s),
      );
      this.subqueries = [...this.subqueries];
    } else {
      this.linkSubqueryToFilter(subqueryId, filterId);
    }
    this.customSql = null;
  }

  // === CTE MANAGEMENT ===
  // Core logic delegated to query-builder-ctes.ts

  addCte(name: string, position: { x: number; y: number }): CanvasCTE {
    const cte = createCte(name, position);
    this.ctes = [...this.ctes, cte];
    this.customSql = null;
    return cte;
  }

  removeCte(cteId: string): void {
    const cte = this.ctes.find((c) => c.id === cteId);
    if (!cte) return;

    this.tables = this.tables.filter((t) => t.cteId !== cteId);
    this.ctes = this.ctes.filter((c) => c.id !== cteId);
    this.customSql = null;
  }

  updateCteName(cteId: string, name: string): void {
    this.ctes = this.ctes.map((c) => (c.id === cteId ? { ...c, name } : c));
    this.customSql = null;
  }

  updateCtePosition(cteId: string, position: { x: number; y: number }): void {
    this.ctes = this.ctes.map((c) => (c.id === cteId ? { ...c, position } : c));
  }

  updateCteSize(cteId: string, size: { width: number; height: number }): void {
    this.ctes = this.ctes.map((c) => (c.id === cteId ? { ...c, size } : c));
    this.customSql = null;
  }

  getCte(cteId: string): CanvasCTE | undefined {
    return this.ctes.find((c) => c.id === cteId);
  }

  addTableToCte(
    cteId: string,
    tableName: string,
    position: { x: number; y: number },
  ): CanvasTable | undefined {
    if (!this.getSchemaTable(tableName)) return undefined;
    const cte = this.ctes.find((c) => c.id === cteId);
    if (!cte) return undefined;

    const { table, newSize } = createCanvasTableForContainer(tableName, position, cte.size);
    if (newSize) cte.size = newSize;
    cte.innerQuery.tables = [...cte.innerQuery.tables, table];
    this.ctes = [...this.ctes];
    this.customSql = null;
    return table;
  }

  removeTableFromCte(cteId: string, tableId: string): void {
    const cte = this.ctes.find((c) => c.id === cteId);
    if (!cte) return;

    removeTableFromInnerQuery(cte.innerQuery, tableId);
    this.ctes = [...this.ctes];
    this.customSql = null;
  }

  toggleCteColumn(cteId: string, tableId: string, columnName: string): void {
    const cte = this.ctes.find((c) => c.id === cteId);
    if (!cte) return;

    if (toggleColumnInTable(cte.innerQuery, tableId, columnName)) {
      this.ctes = [...this.ctes];
      this.customSql = null;
    }
  }

  getCteColumns(cteId: string): Array<{ name: string; type: string }> {
    const cte = this.ctes.find((c) => c.id === cteId);
    if (!cte) return [];
    return getCteColumnsPure(cte, this.schema);
  }

  addCteReference(cteId: string, position: { x: number; y: number }): CanvasTable | undefined {
    const cte = this.ctes.find((c) => c.id === cteId);
    if (!cte || !cte.name) return undefined;

    const { table } = createCteReferenceTable(cte.name, cteId, position, { width: 0, height: 0 });
    this.tables = [...this.tables, table];
    this.customSql = null;
    return table;
  }

  addCteReferenceToSubquery(
    subqueryId: string,
    cteId: string,
    position: { x: number; y: number },
  ): CanvasTable | undefined {
    const cte = this.ctes.find((c) => c.id === cteId);
    if (!cte || !cte.name) return undefined;

    const subquery = this.findSubqueryById(subqueryId);
    if (!subquery) return undefined;

    const { table, newSize } = createCteReferenceTable(cte.name, cteId, position, subquery.size);
    if (newSize) subquery.size = newSize;
    subquery.innerQuery.tables = [...subquery.innerQuery.tables, table];
    this.subqueries = [...this.subqueries];
    this.customSql = null;
    return table;
  }

  // === SQL GENERATION ===

  /**
   * Build SQL from the current canvas state.
   * Delegates to pure SQL generation functions in query-builder-sql.ts.
   */
  private buildSql(): string {
    return generateSqlFromState(
      this.tables,
      this.joins,
      this.filters,
      this.groupBy,
      this.having,
      this.orderBy,
      this.limit,
      this.selectAggregates,
      this.subqueries,
      this.ctes,
      (name) => this.getSchemaTable(name)?.sqlName ?? name,
    );
  }

  // === APPLY FROM PARSED SQL ===

  /**
   * Apply parsed SQL to the visual state.
   * Used for two-way sync between SQL editor and canvas.
   * Delegates to pure functions in query-builder-parsed-sql.ts.
   */
  applyFromParsedSql(parsed: ParsedQuery): void {
    const result = applyParsedSqlToState(
      this.schema,
      parsed,
      this.tables,
      this.ctes,
      this.subqueries,
    );

    this.tables = result.tables;
    this.joins = result.joins;
    this.filters = result.filters;
    this.groupBy = result.groupBy;
    this.having = result.having;
    this.orderBy = result.orderBy;
    this.limit = result.limit;
    this.selectAggregates = result.selectAggregates;
    this.subqueries = result.subqueries;
    this.ctes = result.ctes;
  }

  // === RESET ===

  /**
   * Reset all state to defaults.
   */
  reset(): void {
    this.tables = [];
    this.joins = [];
    this.filters = [];
    this.groupBy = [];
    this.having = [];
    this.orderBy = [];
    this.limit = 100;
    this.customSql = null;
    this.selectAggregates = [];
    this.subqueries = [];
    this.ctes = [];
    this.selectedCteId = null;
  }

  // === SCHEMA MANAGEMENT ===

  /**
   * Set the schema for the query builder.
   * This allows the query builder to work with real database schemas from the Manage section.
   * @param tables - Array of QueryBuilderTable to use as the schema
   */
  setSchema(tables: QueryBuilderTable[]): void {
    this.schema = tables;
  }

  /**
   * Get a table from the current schema by name.
   * @param name - Table name to find
   * @returns The table or undefined if not found
   */
  getSchemaTable(name: string): QueryBuilderTable | undefined {
    return getQueryBuilderTable(this.schema, name);
  }

  // === SERIALIZATION ===

  /**
   * Get a serializable version of the state (for persistence).
   * Converts Sets to arrays.
   */
  toSerializable(): SerializableQueryBuilderState {
    return serializeQueryBuilderState(this);
  }

  /**
   * Restore state from a serialized snapshot.
   */
  fromSerializable(state: SerializableQueryBuilderState): void {
    const deserialized = deserializeQueryBuilderState(state);
    this.tables = deserialized.tables;
    this.joins = deserialized.joins;
    this.filters = deserialized.filters;
    this.groupBy = deserialized.groupBy;
    this.having = deserialized.having;
    this.orderBy = deserialized.orderBy;
    this.limit = deserialized.limit;
    this.customSql = deserialized.customSql;
    this.selectAggregates = deserialized.selectAggregates;
    this.subqueries = deserialized.subqueries;
    this.ctes = deserialized.ctes;
  }
}

// === CONTEXT FUNCTIONS ===

const QUERY_BUILDER_CONTEXT_KEY = "query-builder";

/**
 * Set the query builder context.
 * Call this in a parent component to make the state available to children.
 * @param state - Optional existing state instance. Creates new if not provided.
 * @returns The query builder state instance
 */
export function setQueryBuilder(state?: QueryBuilderState): QueryBuilderState {
  const instance = state ?? new QueryBuilderState();
  setContext(QUERY_BUILDER_CONTEXT_KEY, instance);
  return instance;
}

/**
 * Get the query builder from context.
 * Must be called from a component that has a parent with setQueryBuilder.
 * @returns The query builder state instance
 */
export function useQueryBuilder(): QueryBuilderState {
  return getContext<QueryBuilderState>(QUERY_BUILDER_CONTEXT_KEY);
}

// Re-export serialization types for backward compatibility
export type {
  SerializableTable,
  SerializableInnerQuery,
  SerializableSubquery,
  SerializableCTE,
  SerializableQueryBuilderState,
} from "./query-builder-serialization";
