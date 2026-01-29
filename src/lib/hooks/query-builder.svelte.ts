import { setContext, getContext } from 'svelte';
import { SvelteSet } from 'svelte/reactivity';
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
	SubqueryInnerState,
	CanvasCTE,
	QueryBuilderTable
} from '$lib/types';
import { TUTORIAL_SCHEMA } from '$lib/tutorial/schema';
import { tutorialToQueryBuilder, getQueryBuilderTable } from '$lib/utils/schema-adapter';
import { buildSql as generateSqlFromState } from './query-builder-sql';
import {
	serializeQueryBuilderState,
	deserializeQueryBuilderState,
	type SerializableQueryBuilderState
} from './query-builder-serialization';

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
	activeDisplayAggregates = $derived.by((): DisplayAggregate[] => {
		const result: DisplayAggregate[] = [];
		const tables = this.activeTables;
		const selectAggs = this.activeSelectAggregates;

		// Collect column aggregates from active tables
		for (const table of tables) {
			for (const [columnName, agg] of table.columnAggregates) {
				result.push({
					id: `col-${table.id}-${columnName}`,
					function: agg.function,
					expression: `${table.tableName}.${columnName}`,
					alias: agg.alias,
					source: 'column',
					tableId: table.id,
					columnName
				});
			}
		}

		// Add standalone select aggregates
		for (const agg of selectAggs) {
			result.push({
				id: agg.id,
				function: agg.function,
				expression: agg.expression,
				alias: agg.alias,
				source: 'select'
			});
		}

		return result;
	});

	/**
	 * Generated SQL query from the current canvas state.
	 */
	generatedSql = $derived.by(() => {
		return this.buildSql();
	});

	/**
	 * All aggregates (column + select) in a unified display format.
	 * Used by the filter panel to show all aggregates in one place.
	 */
	allDisplayAggregates = $derived.by((): DisplayAggregate[] => {
		const result: DisplayAggregate[] = [];

		// Collect column aggregates from all tables
		for (const table of this.tables) {
			for (const [columnName, agg] of table.columnAggregates) {
				result.push({
					id: `col-${table.id}-${columnName}`,
					function: agg.function,
					expression: `${table.tableName}.${columnName}`,
					alias: agg.alias,
					source: 'column',
					tableId: table.id,
					columnName
				});
			}
		}

		// Add standalone select aggregates
		for (const agg of this.selectAggregates) {
			result.push({
				id: agg.id,
				function: agg.function,
				expression: agg.expression,
				alias: agg.alias,
				source: 'select'
			});
		}

		return result;
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
				columnAggregates: new Map(t.columnAggregates)
			})),
			joins: [...this.joins],
			filters: [...this.filters],
			groupBy: [...this.groupBy],
			having: [...this.having],
			orderBy: [...this.orderBy],
			limit: this.limit,
			selectAggregates: [...this.selectAggregates],
			subqueries: this.cloneSubqueries(this.subqueries),
			ctes: this.cloneCtes(this.ctes)
		};
	}

	/**
	 * Deep clone subqueries array for snapshot.
	 */
	private cloneSubqueries(subqueries: CanvasSubquery[]): CanvasSubquery[] {
		return subqueries.map((sq) => ({
			...sq,
			position: { ...sq.position },
			size: { ...sq.size },
			innerQuery: this.cloneInnerQuery(sq.innerQuery)
		}));
	}

	/**
	 * Deep clone CTEs array for snapshot.
	 */
	private cloneCtes(ctes: CanvasCTE[]): CanvasCTE[] {
		return ctes.map((cte) => ({
			...cte,
			position: { ...cte.position },
			size: { ...cte.size },
			innerQuery: this.cloneInnerQuery(cte.innerQuery)
		}));
	}

	/**
	 * Deep clone inner query state.
	 */
	private cloneInnerQuery(inner: SubqueryInnerState): SubqueryInnerState {
		return {
			tables: inner.tables.map((t) => ({
				...t,
				selectedColumns: new Set(t.selectedColumns),
				columnAggregates: new Map(t.columnAggregates)
			})),
			joins: [...inner.joins],
			filters: [...inner.filters],
			groupBy: [...inner.groupBy],
			having: [...inner.having],
			orderBy: [...inner.orderBy],
			limit: inner.limit,
			selectAggregates: [...inner.selectAggregates],
			subqueries: this.cloneSubqueries(inner.subqueries)
		};
	}

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
			columnAggregates: new Map<string, ColumnAggregate>()
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

		const tableName = table.tableName;

		// Remove associated joins
		this.joins = this.joins.filter(
			(j) => j.sourceTable !== tableName && j.targetTable !== tableName
		);

		// Remove associated filters (column format is "table.column")
		this.filters = this.filters.filter((f) => !f.column.startsWith(`${tableName}.`));

		// Remove associated order by clauses
		this.orderBy = this.orderBy.filter((o) => !o.column.startsWith(`${tableName}.`));

		// Remove the table
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
		joinType: JoinType = 'INNER'
	): CanvasJoin {
		const join: CanvasJoin = {
			id: generateId(),
			sourceTable,
			sourceColumn,
			targetTable,
			targetColumn,
			joinType
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

	/**
	 * Remove a join.
	 * @param joinId - ID of the join to remove
	 */
	removeJoin(joinId: string): void {
		this.joins = this.joins.filter((j) => j.id !== joinId);
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	// === FILTER MANAGEMENT ===

	/**
	 * Add a filter condition.
	 * @param column - Column to filter on (format: "table.column")
	 * @param operator - Comparison operator
	 * @param value - Value to compare against
	 * @param connector - Logical connector to next condition
	 * @returns The created filter
	 */
	addFilter(
		column: string,
		operator: FilterOperator,
		value: string,
		connector: 'AND' | 'OR' = 'AND'
	): FilterCondition {
		const filter: FilterCondition = {
			id: generateId(),
			column,
			operator,
			value,
			connector
		};

		this.filters = [...this.filters, filter];
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
		return filter;
	}

	/**
	 * Update an existing filter.
	 * @param filterId - ID of the filter
	 * @param updates - Partial filter updates
	 */
	updateFilter(filterId: string, updates: Partial<Omit<FilterCondition, 'id'>>): void {
		this.filters = this.filters.map((f) => (f.id === filterId ? { ...f, ...updates } : f));
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	/**
	 * Remove a filter.
	 * @param filterId - ID of the filter to remove
	 */
	removeFilter(filterId: string): void {
		this.filters = this.filters.filter((f) => f.id !== filterId);
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	// === GROUP BY MANAGEMENT ===

	/**
	 * Add a GROUP BY column.
	 * @param column - Column to group by (format: "table.column")
	 * @returns The created group by condition
	 */
	addGroupBy(column: string): GroupByCondition {
		const groupByCondition: GroupByCondition = {
			id: generateId(),
			column
		};

		this.groupBy = [...this.groupBy, groupByCondition];
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
		return groupByCondition;
	}

	/**
	 * Update the column of a GROUP BY clause.
	 * @param groupById - ID of the group by condition
	 * @param column - New column name
	 */
	updateGroupBy(groupById: string, column: string): void {
		this.groupBy = this.groupBy.map((g) => (g.id === groupById ? { ...g, column } : g));
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	/**
	 * Remove a GROUP BY clause.
	 * @param groupById - ID of the group by condition to remove
	 */
	removeGroupBy(groupById: string): void {
		this.groupBy = this.groupBy.filter((g) => g.id !== groupById);
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	// === HAVING MANAGEMENT ===

	/**
	 * Add a HAVING condition.
	 * @param aggregateFunction - Aggregate function (COUNT, SUM, AVG, MIN, MAX)
	 * @param column - Column for the aggregate (empty string = * for COUNT)
	 * @param operator - Comparison operator
	 * @param value - Value to compare against
	 * @param connector - Logical connector to next condition
	 * @returns The created having condition
	 */
	addHaving(
		aggregateFunction: AggregateFunction,
		column: string,
		operator: HavingOperator,
		value: string,
		connector: 'AND' | 'OR' = 'AND'
	): HavingCondition {
		const havingCondition: HavingCondition = {
			id: generateId(),
			aggregateFunction,
			column,
			operator,
			value,
			connector
		};

		this.having = [...this.having, havingCondition];
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
		return havingCondition;
	}

	/**
	 * Update an existing HAVING condition.
	 * @param havingId - ID of the having condition
	 * @param updates - Partial having condition updates
	 */
	updateHaving(havingId: string, updates: Partial<Omit<HavingCondition, 'id'>>): void {
		this.having = this.having.map((h) => (h.id === havingId ? { ...h, ...updates } : h));
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	/**
	 * Remove a HAVING condition.
	 * @param havingId - ID of the having condition to remove
	 */
	removeHaving(havingId: string): void {
		this.having = this.having.filter((h) => h.id !== havingId);
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	// === ORDER BY MANAGEMENT ===

	/**
	 * Add an ORDER BY clause.
	 * @param column - Column to sort by (format: "table.column")
	 * @param direction - Sort direction
	 * @returns The created sort condition
	 */
	addOrderBy(column: string, direction: SortDirection = 'ASC'): SortCondition {
		const sortCondition: SortCondition = {
			id: generateId(),
			column,
			direction
		};

		this.orderBy = [...this.orderBy, sortCondition];
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
		return sortCondition;
	}

	/**
	 * Update the direction of an ORDER BY clause.
	 * @param orderId - ID of the sort condition
	 * @param direction - New sort direction
	 */
	updateOrderBy(orderId: string, direction: SortDirection): void {
		this.orderBy = this.orderBy.map((o) => (o.id === orderId ? { ...o, direction } : o));
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	/**
	 * Update the column of an ORDER BY clause.
	 * @param orderId - ID of the sort condition
	 * @param column - New column name
	 */
	updateOrderByColumn(orderId: string, column: string): void {
		this.orderBy = this.orderBy.map((o) => (o.id === orderId ? { ...o, column } : o));
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	/**
	 * Remove an ORDER BY clause.
	 * @param orderId - ID of the sort condition to remove
	 */
	removeOrderBy(orderId: string): void {
		this.orderBy = this.orderBy.filter((o) => o.id !== orderId);
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
	}

	/**
	 * Reorder ORDER BY clauses.
	 * @param fromIndex - Current index
	 * @param toIndex - Target index
	 */
	reorderOrderBy(fromIndex: number, toIndex: number): void {
		if (fromIndex < 0 || fromIndex >= this.orderBy.length) return;
		if (toIndex < 0 || toIndex >= this.orderBy.length) return;

		const newOrderBy = [...this.orderBy];
		const [removed] = newOrderBy.splice(fromIndex, 1);
		newOrderBy.splice(toIndex, 0, removed);
		this.orderBy = newOrderBy;
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
		alias?: string
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

	// === SELECT AGGREGATE MANAGEMENT ===

	/**
	 * Add a standalone aggregate to the SELECT clause.
	 * @param func - Aggregate function
	 * @param expression - Expression inside the aggregate (*, column, or expression)
	 * @param alias - Optional alias for AS clause
	 * @returns The created aggregate's ID
	 */
	addSelectAggregate(func: AggregateFunction, expression: string, alias?: string): string {
		const aggregate: SelectAggregate = {
			id: generateId(),
			function: func,
			expression,
			alias
		};

		this.selectAggregates = [...this.selectAggregates, aggregate];
		this.customSql = null;
		return aggregate.id;
	}

	/**
	 * Update a standalone aggregate.
	 * @param id - ID of the aggregate
	 * @param updates - Partial updates
	 */
	updateSelectAggregate(id: string, updates: Partial<Omit<SelectAggregate, 'id'>>): void {
		this.selectAggregates = this.selectAggregates.map((a) =>
			a.id === id ? { ...a, ...updates } : a
		);
		this.customSql = null;
	}

	/**
	 * Remove a standalone aggregate.
	 * @param id - ID of the aggregate to remove
	 */
	removeSelectAggregate(id: string): void {
		this.selectAggregates = this.selectAggregates.filter((a) => a.id !== id);
		this.customSql = null;
	}

	// === SUBQUERY MANAGEMENT ===

	/**
	 * Create an empty inner query state.
	 */
	private createEmptyInnerQuery(): SubqueryInnerState {
		return {
			tables: [],
			joins: [],
			filters: [],
			groupBy: [],
			having: [],
			orderBy: [],
			limit: null,
			selectAggregates: [],
			subqueries: []
		};
	}

	/**
	 * Recursively find a subquery by ID in the subquery tree.
	 * @param subqueryId - ID of the subquery to find
	 * @param subqueries - Array of subqueries to search (defaults to top-level)
	 * @returns The subquery or undefined
	 */
	findSubqueryById(
		subqueryId: string,
		subqueries: CanvasSubquery[] = this.subqueries
	): CanvasSubquery | undefined {
		for (const sq of subqueries) {
			if (sq.id === subqueryId) return sq;
			const nested = this.findSubqueryById(subqueryId, sq.innerQuery.subqueries);
			if (nested) return nested;
		}
		return undefined;
	}

	/**
	 * Find the parent subquery that contains a given subquery ID.
	 * @param childId - ID of the child subquery
	 * @param subqueries - Array to search (defaults to top-level)
	 * @param parent - Current parent (used in recursion)
	 * @returns The parent subquery or undefined if childId is at top level
	 */
	private findParentSubquery(
		childId: string,
		subqueries: CanvasSubquery[] = this.subqueries,
		parent?: CanvasSubquery
	): CanvasSubquery | undefined {
		for (const sq of subqueries) {
			if (sq.id === childId) return parent;
			const found = this.findParentSubquery(childId, sq.innerQuery.subqueries, sq);
			if (found !== undefined) return found;
		}
		return undefined;
	}

	/**
	 * Recursively update a subquery in the tree.
	 * @param subqueryId - ID of the subquery to update
	 * @param updater - Function that returns updated subquery properties
	 */
	private updateSubqueryRecursive(
		subqueryId: string,
		updater: (sq: CanvasSubquery) => Partial<CanvasSubquery>
	): void {
		const updateInArray = (subqueries: CanvasSubquery[]): boolean => {
			for (let i = 0; i < subqueries.length; i++) {
				if (subqueries[i].id === subqueryId) {
					subqueries[i] = { ...subqueries[i], ...updater(subqueries[i]) };
					return true;
				}
				if (updateInArray(subqueries[i].innerQuery.subqueries)) {
					return true;
				}
			}
			return false;
		};

		updateInArray(this.subqueries);
		this.subqueries = [...this.subqueries]; // Trigger reactivity
	}

	/**
	 * Add a subquery to the canvas.
	 * @param role - The role of the subquery (where, from, select)
	 * @param position - Position on the canvas
	 * @param linkedFilterId - Optional filter ID for WHERE subqueries
	 * @returns The created subquery
	 */
	addSubquery(
		role: SubqueryRole,
		position: { x: number; y: number },
		linkedFilterId?: string
	): CanvasSubquery {
		const subquery: CanvasSubquery = {
			id: generateId(),
			position,
			size: { width: 300, height: 200 },
			role,
			linkedFilterId,
			innerQuery: this.createEmptyInnerQuery()
		};

		this.subqueries = [...this.subqueries, subquery];
		this.customSql = null;
		return subquery;
	}

	/**
	 * Remove a subquery from the canvas (supports nested subqueries).
	 * Also cleans up any filters linked to this subquery.
	 * @param subqueryId - ID of the subquery to remove
	 */
	removeSubquery(subqueryId: string): void {
		const subquery = this.findSubqueryById(subqueryId);
		if (!subquery) return;

		// Clean up linked filter's subquery reference
		if (subquery.linkedFilterId) {
			const filter = this.filters.find((f) => f.id === subquery.linkedFilterId);
			if (filter) {
				this.updateFilter(filter.id, { subqueryId: undefined });
			}
		}

		// Check if it's a nested subquery
		const parent = this.findParentSubquery(subqueryId);
		if (parent) {
			parent.innerQuery.subqueries = parent.innerQuery.subqueries.filter(
				(s) => s.id !== subqueryId
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.subqueries = this.subqueries.filter((s) => s.id !== subqueryId);
		}
		this.customSql = null;
	}

	/**
	 * Update a subquery's position on the canvas (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @param position - New position
	 */
	updateSubqueryPosition(subqueryId: string, position: { x: number; y: number }): void {
		this.updateSubqueryRecursive(subqueryId, () => ({ position }));
	}

	/**
	 * Update a subquery's size (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @param size - New size
	 */
	updateSubquerySize(subqueryId: string, size: { width: number; height: number }): void {
		this.updateSubqueryRecursive(subqueryId, () => ({ size }));
		this.customSql = null;
	}

	/**
	 * Update a subquery's role (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @param role - New role
	 */
	updateSubqueryRole(subqueryId: string, role: SubqueryRole): void {
		this.updateSubqueryRecursive(subqueryId, () => ({ role }));
		this.customSql = null;
	}

	/**
	 * Update a subquery's alias (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @param alias - New alias
	 */
	updateSubqueryAlias(subqueryId: string, alias: string): void {
		this.updateSubqueryRecursive(subqueryId, () => ({ alias: alias || undefined }));
		this.customSql = null;
	}

	/**
	 * Link a subquery to a filter for WHERE subqueries (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @param filterId - ID of the filter to link to
	 */
	linkSubqueryToFilter(subqueryId: string, filterId: string): void {
		this.updateSubqueryRecursive(subqueryId, () => ({ linkedFilterId: filterId }));
		this.customSql = null;
	}

	/**
	 * Get a subquery by ID (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @returns The subquery or undefined
	 */
	getSubquery(subqueryId: string): CanvasSubquery | undefined {
		return this.findSubqueryById(subqueryId);
	}

	/**
	 * Add a table to a subquery's inner query.
	 * Auto-resizes the subquery container if the table doesn't fit.
	 * @param subqueryId - ID of the subquery
	 * @param tableName - Name of the table
	 * @param position - Position relative to subquery
	 * @returns The created table or undefined
	 */
	addTableToSubquery(
		subqueryId: string,
		tableName: string,
		position: { x: number; y: number }
	): CanvasTable | undefined {
		const tableSchema = this.getSchemaTable(tableName);
		if (!tableSchema) return undefined;

		// Use recursive finder to support nested subqueries
		const subquery = this.findSubqueryById(subqueryId);
		if (!subquery) return undefined;

		const canvasTable: CanvasTable = {
			id: generateId(),
			tableName,
			position,
			selectedColumns: new SvelteSet<string>(),
			columnAggregates: new Map<string, ColumnAggregate>()
		};

		// Estimated table node dimensions (from table-node.svelte styling)
		const TABLE_WIDTH = 220;
		const TABLE_HEIGHT = 280; // scroll area (240px) + header (~40px)
		const PADDING = 20;

		// Calculate the space needed for the new table
		const requiredWidth = position.x + TABLE_WIDTH + PADDING;
		const requiredHeight = position.y + TABLE_HEIGHT + PADDING;

		// Expand subquery size if needed
		let newWidth = subquery.size.width;
		let newHeight = subquery.size.height;

		if (requiredWidth > subquery.size.width) {
			newWidth = requiredWidth;
		}
		if (requiredHeight > subquery.size.height) {
			newHeight = requiredHeight;
		}

		// Update size if changed
		if (newWidth !== subquery.size.width || newHeight !== subquery.size.height) {
			subquery.size = { width: newWidth, height: newHeight };
		}

		subquery.innerQuery.tables = [...subquery.innerQuery.tables, canvasTable];
		this.subqueries = [...this.subqueries]; // Trigger reactivity
		this.customSql = null;
		return canvasTable;
	}

	/**
	 * Remove a table from a subquery's inner query (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @param tableId - ID of the table to remove
	 */
	removeTableFromSubquery(subqueryId: string, tableId: string): void {
		// Use recursive finder to support nested subqueries
		const subquery = this.findSubqueryById(subqueryId);
		if (!subquery) return;

		const table = subquery.innerQuery.tables.find((t) => t.id === tableId);
		if (!table) return;

		const tableName = table.tableName;

		// Remove associated joins
		subquery.innerQuery.joins = subquery.innerQuery.joins.filter(
			(j) => j.sourceTable !== tableName && j.targetTable !== tableName
		);

		// Remove associated filters
		subquery.innerQuery.filters = subquery.innerQuery.filters.filter(
			(f) => !f.column.startsWith(`${tableName}.`)
		);

		// Remove associated order by
		subquery.innerQuery.orderBy = subquery.innerQuery.orderBy.filter(
			(o) => !o.column.startsWith(`${tableName}.`)
		);

		// Remove the table
		subquery.innerQuery.tables = subquery.innerQuery.tables.filter((t) => t.id !== tableId);
		this.subqueries = [...this.subqueries];
		this.customSql = null;
	}

	/**
	 * Toggle a column selection in a subquery table (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @param tableId - ID of the table
	 * @param columnName - Column to toggle
	 */
	toggleSubqueryColumn(subqueryId: string, tableId: string, columnName: string): void {
		const subquery = this.findSubqueryById(subqueryId);
		if (!subquery) return;

		const table = subquery.innerQuery.tables.find((t) => t.id === tableId);
		if (!table) return;

		if (table.selectedColumns.has(columnName)) {
			table.selectedColumns.delete(columnName);
			table.columnAggregates.delete(columnName);
		} else {
			table.selectedColumns.add(columnName);
		}
		this.subqueries = [...this.subqueries];
		this.customSql = null;
	}

	/**
	 * Add a select aggregate to a subquery (supports nested).
	 * @param subqueryId - ID of the subquery
	 * @param func - Aggregate function
	 * @param expression - Expression
	 * @param alias - Optional alias
	 * @returns The aggregate ID or undefined
	 */
	addSubquerySelectAggregate(
		subqueryId: string,
		func: AggregateFunction,
		expression: string,
		alias?: string
	): string | undefined {
		const subquery = this.findSubqueryById(subqueryId);
		if (!subquery) return undefined;

		const aggregate: SelectAggregate = {
			id: generateId(),
			function: func,
			expression,
			alias
		};

		subquery.innerQuery.selectAggregates = [...subquery.innerQuery.selectAggregates, aggregate];
		this.subqueries = [...this.subqueries];
		this.customSql = null;
		return aggregate.id;
	}

	// === ACTIVE CONTEXT METHODS ===
	// These methods operate on either the top-level query or the selected subquery

	/**
	 * Add a filter to the active context (CTE, subquery, or top-level).
	 */
	addActiveFilter(
		column: string,
		operator: FilterOperator,
		value: string,
		connector: 'AND' | 'OR' = 'AND'
	): FilterCondition {
		const filter: FilterCondition = {
			id: generateId(),
			column,
			operator,
			value,
			connector
		};

		if (this.selectedCte) {
			this.selectedCte.innerQuery.filters = [...this.selectedCte.innerQuery.filters, filter];
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.filters = [...this.selectedSubquery.innerQuery.filters, filter];
			this.subqueries = [...this.subqueries];
		} else {
			this.filters = [...this.filters, filter];
		}
		this.customSql = null;
		return filter;
	}

	/**
	 * Update a filter in the active context.
	 */
	updateActiveFilter(filterId: string, updates: Partial<Omit<FilterCondition, 'id'>>): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.filters = this.selectedCte.innerQuery.filters.map((f) =>
				f.id === filterId ? { ...f, ...updates } : f
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.filters = this.selectedSubquery.innerQuery.filters.map((f) =>
				f.id === filterId ? { ...f, ...updates } : f
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.filters = this.filters.map((f) => (f.id === filterId ? { ...f, ...updates } : f));
		}
		this.customSql = null;
	}

	/**
	 * Remove a filter from the active context.
	 */
	removeActiveFilter(filterId: string): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.filters = this.selectedCte.innerQuery.filters.filter(
				(f) => f.id !== filterId
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.filters = this.selectedSubquery.innerQuery.filters.filter(
				(f) => f.id !== filterId
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.filters = this.filters.filter((f) => f.id !== filterId);
		}
		this.customSql = null;
	}

	/**
	 * Add a GROUP BY to the active context.
	 */
	addActiveGroupBy(column: string): GroupByCondition {
		const groupBy: GroupByCondition = {
			id: generateId(),
			column
		};

		if (this.selectedCte) {
			this.selectedCte.innerQuery.groupBy = [...this.selectedCte.innerQuery.groupBy, groupBy];
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.groupBy = [...this.selectedSubquery.innerQuery.groupBy, groupBy];
			this.subqueries = [...this.subqueries];
		} else {
			this.groupBy = [...this.groupBy, groupBy];
		}
		this.customSql = null;
		return groupBy;
	}

	/**
	 * Update a GROUP BY in the active context.
	 */
	updateActiveGroupBy(groupById: string, column: string): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.groupBy = this.selectedCte.innerQuery.groupBy.map((g) =>
				g.id === groupById ? { ...g, column } : g
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.groupBy = this.selectedSubquery.innerQuery.groupBy.map((g) =>
				g.id === groupById ? { ...g, column } : g
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.groupBy = this.groupBy.map((g) => (g.id === groupById ? { ...g, column } : g));
		}
		this.customSql = null;
	}

	/**
	 * Remove a GROUP BY from the active context.
	 */
	removeActiveGroupBy(groupById: string): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.groupBy = this.selectedCte.innerQuery.groupBy.filter(
				(g) => g.id !== groupById
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.groupBy = this.selectedSubquery.innerQuery.groupBy.filter(
				(g) => g.id !== groupById
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.groupBy = this.groupBy.filter((g) => g.id !== groupById);
		}
		this.customSql = null;
	}

	/**
	 * Add a HAVING to the active context.
	 */
	addActiveHaving(
		aggregateFunction: AggregateFunction,
		column: string,
		operator: HavingOperator,
		value: string,
		connector: 'AND' | 'OR' = 'AND'
	): HavingCondition {
		const having: HavingCondition = {
			id: generateId(),
			aggregateFunction,
			column,
			operator,
			value,
			connector
		};

		if (this.selectedCte) {
			this.selectedCte.innerQuery.having = [...this.selectedCte.innerQuery.having, having];
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.having = [...this.selectedSubquery.innerQuery.having, having];
			this.subqueries = [...this.subqueries];
		} else {
			this.having = [...this.having, having];
		}
		this.customSql = null;
		return having;
	}

	/**
	 * Update a HAVING in the active context.
	 */
	updateActiveHaving(havingId: string, updates: Partial<Omit<HavingCondition, 'id'>>): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.having = this.selectedCte.innerQuery.having.map((h) =>
				h.id === havingId ? { ...h, ...updates } : h
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.having = this.selectedSubquery.innerQuery.having.map((h) =>
				h.id === havingId ? { ...h, ...updates } : h
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.having = this.having.map((h) => (h.id === havingId ? { ...h, ...updates } : h));
		}
		this.customSql = null;
	}

	/**
	 * Remove a HAVING from the active context.
	 */
	removeActiveHaving(havingId: string): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.having = this.selectedCte.innerQuery.having.filter(
				(h) => h.id !== havingId
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.having = this.selectedSubquery.innerQuery.having.filter(
				(h) => h.id !== havingId
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.having = this.having.filter((h) => h.id !== havingId);
		}
		this.customSql = null;
	}

	/**
	 * Add an ORDER BY to the active context.
	 */
	addActiveOrderBy(column: string, direction: SortDirection = 'ASC'): SortCondition {
		const orderBy: SortCondition = {
			id: generateId(),
			column,
			direction
		};

		if (this.selectedCte) {
			this.selectedCte.innerQuery.orderBy = [...this.selectedCte.innerQuery.orderBy, orderBy];
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.orderBy = [...this.selectedSubquery.innerQuery.orderBy, orderBy];
			this.subqueries = [...this.subqueries];
		} else {
			this.orderBy = [...this.orderBy, orderBy];
		}
		this.customSql = null;
		return orderBy;
	}

	/**
	 * Update an ORDER BY direction in the active context.
	 */
	updateActiveOrderBy(orderId: string, direction: SortDirection): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.orderBy = this.selectedCte.innerQuery.orderBy.map((o) =>
				o.id === orderId ? { ...o, direction } : o
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.orderBy = this.selectedSubquery.innerQuery.orderBy.map((o) =>
				o.id === orderId ? { ...o, direction } : o
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.orderBy = this.orderBy.map((o) => (o.id === orderId ? { ...o, direction } : o));
		}
		this.customSql = null;
	}

	/**
	 * Update an ORDER BY column in the active context.
	 */
	updateActiveOrderByColumn(orderId: string, column: string): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.orderBy = this.selectedCte.innerQuery.orderBy.map((o) =>
				o.id === orderId ? { ...o, column } : o
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.orderBy = this.selectedSubquery.innerQuery.orderBy.map((o) =>
				o.id === orderId ? { ...o, column } : o
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.orderBy = this.orderBy.map((o) => (o.id === orderId ? { ...o, column } : o));
		}
		this.customSql = null;
	}

	/**
	 * Remove an ORDER BY from the active context.
	 */
	removeActiveOrderBy(orderId: string): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.orderBy = this.selectedCte.innerQuery.orderBy.filter(
				(o) => o.id !== orderId
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.orderBy = this.selectedSubquery.innerQuery.orderBy.filter(
				(o) => o.id !== orderId
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.orderBy = this.orderBy.filter((o) => o.id !== orderId);
		}
		this.customSql = null;
	}

	/**
	 * Set the LIMIT in the active context. Can be a number, null, or a {{variable}} string.
	 */
	setActiveLimit(limit: string | number | null): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.limit = limit;
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.limit = limit;
			this.subqueries = [...this.subqueries];
		} else {
			this.limit = limit;
		}
		this.customSql = null;
	}

	/**
	 * Add a select aggregate to the active context.
	 */
	addActiveSelectAggregate(func: AggregateFunction, expression: string, alias?: string): string {
		const aggregate: SelectAggregate = {
			id: generateId(),
			function: func,
			expression,
			alias
		};

		if (this.selectedCte) {
			this.selectedCte.innerQuery.selectAggregates = [
				...this.selectedCte.innerQuery.selectAggregates,
				aggregate
			];
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.selectAggregates = [
				...this.selectedSubquery.innerQuery.selectAggregates,
				aggregate
			];
			this.subqueries = [...this.subqueries];
		} else {
			this.selectAggregates = [...this.selectAggregates, aggregate];
		}
		this.customSql = null;
		return aggregate.id;
	}

	/**
	 * Update a select aggregate in the active context.
	 */
	updateActiveSelectAggregate(id: string, updates: Partial<Omit<SelectAggregate, 'id'>>): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.selectAggregates = this.selectedCte.innerQuery.selectAggregates.map(
				(a) => (a.id === id ? { ...a, ...updates } : a)
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.selectAggregates = this.selectedSubquery.innerQuery.selectAggregates.map(
				(a) => (a.id === id ? { ...a, ...updates } : a)
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.selectAggregates = this.selectAggregates.map((a) => (a.id === id ? { ...a, ...updates } : a));
		}
		this.customSql = null;
	}

	/**
	 * Remove a select aggregate from the active context.
	 */
	removeActiveSelectAggregate(id: string): void {
		if (this.selectedCte) {
			this.selectedCte.innerQuery.selectAggregates = this.selectedCte.innerQuery.selectAggregates.filter(
				(a) => a.id !== id
			);
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.selectAggregates = this.selectedSubquery.innerQuery.selectAggregates.filter(
				(a) => a.id !== id
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.selectAggregates = this.selectAggregates.filter((a) => a.id !== id);
		}
		this.customSql = null;
	}

	/**
	 * Set a column aggregate in the active context.
	 */
	setActiveColumnAggregate(
		tableId: string,
		column: string,
		func: AggregateFunction | null,
		alias?: string
	): void {
		const tables = this.activeTables;
		const table = tables.find((t) => t.id === tableId);
		if (!table) return;

		if (func === null) {
			table.columnAggregates.delete(column);
		} else {
			table.columnAggregates.set(column, { function: func, alias });
		}

		// Trigger reactivity
		if (this.selectedCte) {
			this.ctes = [...this.ctes];
		} else if (this.selectedSubquery) {
			this.subqueries = [...this.subqueries];
		} else {
			this.tables = [...this.tables];
		}
		this.customSql = null;
	}

	/**
	 * Clear a column aggregate in the active context.
	 */
	clearActiveColumnAggregate(tableId: string, column: string): void {
		this.setActiveColumnAggregate(tableId, column, null);
	}

	/**
	 * Add a subquery to the active context (top-level or nested in selected subquery).
	 */
	addActiveSubquery(
		role: SubqueryRole,
		position: { x: number; y: number },
		linkedFilterId?: string
	): CanvasSubquery {
		const subquery: CanvasSubquery = {
			id: generateId(),
			position,
			size: { width: 300, height: 200 },
			role,
			linkedFilterId,
			innerQuery: this.createEmptyInnerQuery()
		};

		if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.subqueries = [
				...this.selectedSubquery.innerQuery.subqueries,
				subquery
			];
			this.subqueries = [...this.subqueries];
		} else {
			this.subqueries = [...this.subqueries, subquery];
		}
		this.customSql = null;
		return subquery;
	}

	/**
	 * Get subqueries from the active context.
	 */
	get activeSubqueries(): CanvasSubquery[] {
		return this.selectedSubquery?.innerQuery.subqueries ?? this.subqueries;
	}

	/**
	 * Remove a subquery from the active context.
	 */
	removeActiveSubquery(subqueryId: string): void {
		if (this.selectedSubquery) {
			const subquery = this.selectedSubquery.innerQuery.subqueries.find(s => s.id === subqueryId);
			if (subquery?.linkedFilterId) {
				// Clean up linked filter
				const filter = this.selectedSubquery.innerQuery.filters.find(f => f.id === subquery.linkedFilterId);
				if (filter) {
					this.updateActiveFilter(filter.id, { subqueryId: undefined });
				}
			}
			this.selectedSubquery.innerQuery.subqueries = this.selectedSubquery.innerQuery.subqueries.filter(
				s => s.id !== subqueryId
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.removeSubquery(subqueryId);
		}
		this.customSql = null;
	}

	/**
	 * Link a subquery to a filter in the active context.
	 */
	linkActiveSubqueryToFilter(subqueryId: string, filterId: string): void {
		if (this.selectedSubquery) {
			this.selectedSubquery.innerQuery.subqueries = this.selectedSubquery.innerQuery.subqueries.map(s =>
				s.id === subqueryId ? { ...s, linkedFilterId: filterId } : s
			);
			this.subqueries = [...this.subqueries];
		} else {
			this.linkSubqueryToFilter(subqueryId, filterId);
		}
		this.customSql = null;
	}

	// === CTE MANAGEMENT ===

	/**
	 * Add a CTE to the canvas.
	 * @param name - CTE name (used in WITH clause)
	 * @param position - Position on the canvas
	 * @returns The created CTE
	 */
	addCte(name: string, position: { x: number; y: number }): CanvasCTE {
		const cte: CanvasCTE = {
			id: generateId(),
			name,
			position,
			size: { width: 300, height: 200 },
			innerQuery: this.createEmptyInnerQuery()
		};

		this.ctes = [...this.ctes, cte];
		this.customSql = null;
		return cte;
	}

	/**
	 * Remove a CTE from the canvas.
	 * Also removes any tables that reference this CTE.
	 * @param cteId - ID of the CTE to remove
	 */
	removeCte(cteId: string): void {
		const cte = this.ctes.find((c) => c.id === cteId);
		if (!cte) return;

		// Remove any tables that reference this CTE
		this.tables = this.tables.filter((t) => t.cteId !== cteId);

		// Remove the CTE
		this.ctes = this.ctes.filter((c) => c.id !== cteId);
		this.customSql = null;
	}

	/**
	 * Update a CTE's name.
	 * @param cteId - ID of the CTE
	 * @param name - New name
	 */
	updateCteName(cteId: string, name: string): void {
		this.ctes = this.ctes.map((c) => (c.id === cteId ? { ...c, name } : c));
		this.customSql = null;
	}

	/**
	 * Update a CTE's position on the canvas.
	 * @param cteId - ID of the CTE
	 * @param position - New position
	 */
	updateCtePosition(cteId: string, position: { x: number; y: number }): void {
		this.ctes = this.ctes.map((c) => (c.id === cteId ? { ...c, position } : c));
	}

	/**
	 * Update a CTE's size.
	 * @param cteId - ID of the CTE
	 * @param size - New size
	 */
	updateCteSize(cteId: string, size: { width: number; height: number }): void {
		this.ctes = this.ctes.map((c) => (c.id === cteId ? { ...c, size } : c));
		this.customSql = null;
	}

	/**
	 * Get a CTE by ID.
	 * @param cteId - ID of the CTE
	 * @returns The CTE or undefined
	 */
	getCte(cteId: string): CanvasCTE | undefined {
		return this.ctes.find((c) => c.id === cteId);
	}

	/**
	 * Add a table to a CTE's inner query.
	 * Auto-resizes the CTE container if the table doesn't fit.
	 * @param cteId - ID of the CTE
	 * @param tableName - Name of the table
	 * @param position - Position relative to CTE
	 * @returns The created table or undefined
	 */
	addTableToCte(
		cteId: string,
		tableName: string,
		position: { x: number; y: number }
	): CanvasTable | undefined {
		const tableSchema = this.getSchemaTable(tableName);
		if (!tableSchema) return undefined;

		const cte = this.ctes.find((c) => c.id === cteId);
		if (!cte) return undefined;

		const canvasTable: CanvasTable = {
			id: generateId(),
			tableName,
			position,
			selectedColumns: new SvelteSet<string>(),
			columnAggregates: new Map<string, ColumnAggregate>()
		};

		// Estimated table node dimensions
		const TABLE_WIDTH = 220;
		const TABLE_HEIGHT = 280;
		const PADDING = 20;

		// Calculate the space needed for the new table
		const requiredWidth = position.x + TABLE_WIDTH + PADDING;
		const requiredHeight = position.y + TABLE_HEIGHT + PADDING;

		// Expand CTE size if needed
		let newWidth = cte.size.width;
		let newHeight = cte.size.height;

		if (requiredWidth > cte.size.width) {
			newWidth = requiredWidth;
		}
		if (requiredHeight > cte.size.height) {
			newHeight = requiredHeight;
		}

		// Update size if changed
		if (newWidth !== cte.size.width || newHeight !== cte.size.height) {
			cte.size = { width: newWidth, height: newHeight };
		}

		cte.innerQuery.tables = [...cte.innerQuery.tables, canvasTable];
		this.ctes = [...this.ctes]; // Trigger reactivity
		this.customSql = null;
		return canvasTable;
	}

	/**
	 * Remove a table from a CTE's inner query.
	 * @param cteId - ID of the CTE
	 * @param tableId - ID of the table to remove
	 */
	removeTableFromCte(cteId: string, tableId: string): void {
		const cte = this.ctes.find((c) => c.id === cteId);
		if (!cte) return;

		const table = cte.innerQuery.tables.find((t) => t.id === tableId);
		if (!table) return;

		const tableName = table.tableName;

		// Remove associated joins
		cte.innerQuery.joins = cte.innerQuery.joins.filter(
			(j) => j.sourceTable !== tableName && j.targetTable !== tableName
		);

		// Remove associated filters
		cte.innerQuery.filters = cte.innerQuery.filters.filter(
			(f) => !f.column.startsWith(`${tableName}.`)
		);

		// Remove associated order by
		cte.innerQuery.orderBy = cte.innerQuery.orderBy.filter(
			(o) => !o.column.startsWith(`${tableName}.`)
		);

		// Remove the table
		cte.innerQuery.tables = cte.innerQuery.tables.filter((t) => t.id !== tableId);
		this.ctes = [...this.ctes];
		this.customSql = null;
	}

	/**
	 * Toggle a column selection in a CTE table.
	 * @param cteId - ID of the CTE
	 * @param tableId - ID of the table
	 * @param columnName - Column to toggle
	 */
	toggleCteColumn(cteId: string, tableId: string, columnName: string): void {
		const cte = this.ctes.find((c) => c.id === cteId);
		if (!cte) return;

		const table = cte.innerQuery.tables.find((t) => t.id === tableId);
		if (!table) return;

		if (table.selectedColumns.has(columnName)) {
			table.selectedColumns.delete(columnName);
			table.columnAggregates.delete(columnName);
		} else {
			table.selectedColumns.add(columnName);
		}
		this.ctes = [...this.ctes];
		this.customSql = null;
	}

	/**
	 * Get the derived columns from a CTE (columns output by its SELECT clause).
	 * Used when referencing the CTE as a table.
	 * @param cteId - ID of the CTE
	 * @returns Array of column definitions
	 */
	getCteColumns(cteId: string): Array<{ name: string; type: string }> {
		const cte = this.ctes.find((c) => c.id === cteId);
		if (!cte) return [];

		const columns: Array<{ name: string; type: string }> = [];

		// Get columns from selected columns in CTE's tables
		for (const table of cte.innerQuery.tables) {
			const tableSchema = this.getSchemaTable(table.tableName);
			if (!tableSchema) continue;

			for (const colName of table.selectedColumns) {
				const col = tableSchema.columns.find((c) => c.name === colName);
				if (col) {
					const agg = table.columnAggregates.get(colName);
					if (agg) {
						// Aggregated column - use alias or generated name
						columns.push({
							name: agg.alias || `${agg.function.toLowerCase()}_${colName}`,
							type: 'numeric'
						});
					} else {
						columns.push({ name: col.name, type: col.type });
					}
				}
			}
		}

		// Add standalone aggregates from the CTE
		for (const agg of cte.innerQuery.selectAggregates) {
			columns.push({
				name: agg.alias || `${agg.function.toLowerCase()}_${agg.expression.replace(/[^a-zA-Z0-9]/g, '_')}`,
				type: 'numeric'
			});
		}

		// If no columns selected, treat it as SELECT * (all columns from first table)
		if (columns.length === 0 && cte.innerQuery.tables.length > 0) {
			const firstTable = cte.innerQuery.tables[0];
			const tableSchema = this.getSchemaTable(firstTable.tableName);
			if (tableSchema) {
				for (const col of tableSchema.columns) {
					columns.push({ name: col.name, type: col.type });
				}
			}
		}

		return columns;
	}

	/**
	 * Add a CTE reference table to the main query canvas.
	 * This creates a table node that references the CTE by its ID.
	 * @param cteId - ID of the CTE to reference
	 * @param position - Position on the canvas
	 * @returns The created table or undefined
	 */
	addCteReference(cteId: string, position: { x: number; y: number }): CanvasTable | undefined {
		const cte = this.ctes.find((c) => c.id === cteId);
		if (!cte || !cte.name) return undefined;

		const canvasTable: CanvasTable = {
			id: generateId(),
			tableName: cte.name, // Use CTE name as table name
			position,
			selectedColumns: new SvelteSet<string>(),
			columnAggregates: new Map<string, ColumnAggregate>(),
			cteId // Mark this as a CTE reference
		};

		this.tables = [...this.tables, canvasTable];
		this.customSql = null;
		return canvasTable;
	}

	/**
	 * Add a reference to an existing CTE inside a subquery.
	 * CTEs defined at the top level are accessible within subqueries in SQL.
	 * @param subqueryId - ID of the subquery to add the reference to
	 * @param cteId - ID of the CTE to reference
	 * @param position - Position relative to the subquery
	 * @returns The created table or undefined
	 */
	addCteReferenceToSubquery(
		subqueryId: string,
		cteId: string,
		position: { x: number; y: number }
	): CanvasTable | undefined {
		const cte = this.ctes.find((c) => c.id === cteId);
		if (!cte || !cte.name) return undefined;

		const subquery = this.findSubqueryById(subqueryId);
		if (!subquery) return undefined;

		const canvasTable: CanvasTable = {
			id: generateId(),
			tableName: cte.name, // Use CTE name as table name
			position,
			selectedColumns: new SvelteSet<string>(),
			columnAggregates: new Map<string, ColumnAggregate>(),
			cteId // Mark this as a CTE reference
		};

		// Auto-resize subquery if needed
		const TABLE_WIDTH = 220;
		const TABLE_HEIGHT = 280;
		const PADDING = 20;
		const requiredWidth = position.x + TABLE_WIDTH + PADDING;
		const requiredHeight = position.y + TABLE_HEIGHT + PADDING;

		if (requiredWidth > subquery.size.width || requiredHeight > subquery.size.height) {
			subquery.size = {
				width: Math.max(subquery.size.width, requiredWidth),
				height: Math.max(subquery.size.height, requiredHeight)
			};
		}

		subquery.innerQuery.tables = [...subquery.innerQuery.tables, canvasTable];
		this.subqueries = [...this.subqueries]; // Trigger reactivity
		this.customSql = null;
		return canvasTable;
	}

	// === LIMIT ===

	/**
	 * Set the LIMIT value.
	 * @param limit - Limit value, or null for no limit
	 */
	setLimit(limit: number | null): void {
		this.limit = limit;
		this.customSql = null; // Clear custom SQL so editor syncs with visual state
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
			this.ctes
		);
	}

	// === APPLY FROM PARSED SQL ===

	/**
	 * Expand selectedColumns, converting '*' to all column names from the schema.
	 * @param tableName - The name of the table to look up in the schema
	 * @param selectedColumns - The columns from parsed SQL (may contain '*')
	 * @returns Expanded column names, or original if no '*' or table not found
	 */
	private expandSelectedColumns(tableName: string, selectedColumns: string[]): string[] {
		// Check if selectedColumns contains '*' (SELECT * or table.*)
		if (selectedColumns.includes('*')) {
			const schemaTable = this.schema.find((t) => t.name === tableName);
			if (schemaTable) {
				// Return all column names from the schema
				return schemaTable.columns.map((c) => c.name);
			}
		}
		return selectedColumns;
	}

	/**
	 * Type for parsed subquery inner query (recursive).
	 */
	// eslint-disable-next-line @typescript-eslint/no-explicit-any
	private buildSubqueriesFromParsed(
		parsedSubqueries: Array<{
			id: string;
			role: 'where' | 'from' | 'select';
			linkedFilterIndex?: number;
			innerQuery: {
				tables: Array<{ tableName: string; selectedColumns: string[]; isCteReference?: boolean }>;
				joins: Array<{
					sourceTable: string;
					sourceColumn: string;
					targetTable: string;
					targetColumn: string;
					joinType: 'INNER' | 'LEFT' | 'RIGHT' | 'FULL';
				}>;
				filters: Array<{
					column: string;
					operator: FilterOperator;
					value: string;
					connector: 'AND' | 'OR';
					subqueryIndex?: number;
				}>;
				groupBy: Array<{ column: string }>;
				having: Array<{
					aggregateFunction: AggregateFunction;
					column: string;
					operator: HavingOperator;
					value: string;
					connector: 'AND' | 'OR';
				}>;
				orderBy: Array<{ column: string; direction: 'ASC' | 'DESC' }>;
				limit: number | null;
				selectAggregates: Array<{
					function: AggregateFunction;
					expression: string;
					alias?: string;
				}>;
				columnAggregates?: Array<{
					tableName: string;
					column: string;
					function: AggregateFunction;
					alias?: string;
				}>;
				// eslint-disable-next-line @typescript-eslint/no-explicit-any
				subqueries?: any[];
			};
		}>,
		existingSubqueries: CanvasSubquery[],
		basePosition: { x: number; y: number } = { x: 50, y: 50 },
		cteNameToId: Map<string, string> = new Map()
	): { subqueries: CanvasSubquery[]; subqueryIdMap: Map<number, string> } {
		const result: CanvasSubquery[] = [];
		const subqueryIdMap = new Map<number, string>();

		for (let i = 0; i < parsedSubqueries.length; i++) {
			const ps = parsedSubqueries[i];
			const existingSubquery = existingSubqueries[i];

			const position = existingSubquery?.position ?? {
				x: basePosition.x + i * 350,
				y: basePosition.y
			};

			const subqueryId = existingSubquery?.id ?? generateId();
			subqueryIdMap.set(i, subqueryId);

			// Build inner query tables
			const innerTables: CanvasTable[] = [];
			for (let j = 0; j < ps.innerQuery.tables.length; j++) {
				const pt = ps.innerQuery.tables[j];

				const columnAggsForTable = new Map<string, ColumnAggregate>();
				if (ps.innerQuery.columnAggregates) {
					for (const ca of ps.innerQuery.columnAggregates) {
						if (ca.tableName === pt.tableName) {
							columnAggsForTable.set(ca.column, {
								function: ca.function,
								alias: ca.alias
							});
						}
					}
				}

				// Check if this table references a CTE
				const cteId = cteNameToId.get(pt.tableName);

				innerTables.push({
					id: generateId(),
					tableName: pt.tableName,
					position: { x: 20 + j * 240, y: 50 },
					selectedColumns: new SvelteSet(this.expandSelectedColumns(pt.tableName, pt.selectedColumns)),
					columnAggregates: columnAggsForTable,
					cteId // Will be undefined for regular tables, set for CTE references
				});
			}

			// Build inner query joins
			const innerJoins: CanvasJoin[] = ps.innerQuery.joins.map((pj) => ({
				id: generateId(),
				sourceTable: pj.sourceTable,
				sourceColumn: pj.sourceColumn,
				targetTable: pj.targetTable,
				targetColumn: pj.targetColumn,
				joinType: pj.joinType
			}));

			// Recursively build nested subqueries
			const nestedResult = ps.innerQuery.subqueries && ps.innerQuery.subqueries.length > 0
				? this.buildSubqueriesFromParsed(
						ps.innerQuery.subqueries,
						existingSubquery?.innerQuery.subqueries ?? [],
						{ x: 50, y: 50 },
						cteNameToId
				  )
				: { subqueries: [], subqueryIdMap: new Map<number, string>() };

			// Build inner filters with nested subquery links
			const innerFilters: FilterCondition[] = ps.innerQuery.filters.map((f, idx) => {
				const filter: FilterCondition = {
					id: generateId(),
					column: f.column,
					operator: f.operator,
					value: f.value,
					connector: f.connector
				};
				if (f.subqueryIndex !== undefined) {
					const nestedSubqueryId = nestedResult.subqueryIdMap.get(f.subqueryIndex);
					if (nestedSubqueryId) {
						filter.subqueryId = nestedSubqueryId;
						const nestedSubquery = nestedResult.subqueries.find(s => s.id === nestedSubqueryId);
						if (nestedSubquery) {
							nestedSubquery.linkedFilterId = filter.id;
						}
					}
				}
				return filter;
			});

			const subqueryWidth = Math.max(300, 20 + ps.innerQuery.tables.length * 240 + 20);
			const subqueryHeight = Math.max(200, 350);

			result.push({
				id: subqueryId,
				position,
				size: existingSubquery?.size ?? { width: subqueryWidth, height: subqueryHeight },
				role: ps.role,
				innerQuery: {
					tables: innerTables,
					joins: innerJoins,
					filters: innerFilters,
					groupBy: ps.innerQuery.groupBy.map((g) => ({
						id: generateId(),
						column: g.column
					})),
					having: ps.innerQuery.having.map((h) => ({
						id: generateId(),
						aggregateFunction: h.aggregateFunction,
						column: h.column,
						operator: h.operator,
						value: h.value,
						connector: h.connector
					})),
					orderBy: ps.innerQuery.orderBy.map((o) => ({
						id: generateId(),
						column: o.column,
						direction: o.direction
					})),
					limit: ps.innerQuery.limit,
					selectAggregates: ps.innerQuery.selectAggregates.map((a) => ({
						id: generateId(),
						function: a.function,
						expression: a.expression,
						alias: a.alias
					})),
					subqueries: nestedResult.subqueries
				}
			});
		}

		return { subqueries: result, subqueryIdMap };
	}

	/**
	 * Apply parsed SQL to the visual state.
	 * Used for two-way sync between SQL editor and canvas.
	 * @param parsed - Parsed query from sql-parser
	 */
	applyFromParsedSql(parsed: {
		tables: Array<{ tableName: string; selectedColumns: string[] }>;
		joins: Array<{
			sourceTable: string;
			sourceColumn: string;
			targetTable: string;
			targetColumn: string;
			joinType: 'INNER' | 'LEFT' | 'RIGHT' | 'FULL';
		}>;
		filters: Array<{
			column: string;
			operator: FilterOperator;
			value: string;
			connector: 'AND' | 'OR';
			subqueryIndex?: number;
		}>;
		groupBy: Array<{ column: string }>;
		having: Array<{
			aggregateFunction: AggregateFunction;
			column: string;
			operator: HavingOperator;
			value: string;
			connector: 'AND' | 'OR';
		}>;
		orderBy: Array<{ column: string; direction: 'ASC' | 'DESC' }>;
		limit: number | null;
		selectAggregates: Array<{
			function: AggregateFunction;
			expression: string;
			alias?: string;
		}>;
		columnAggregates: Array<{
			tableName: string;
			column: string;
			function: AggregateFunction;
			alias?: string;
		}>;
		subqueries?: Array<{
			id: string;
			role: 'where' | 'from' | 'select';
			linkedFilterIndex?: number;
			innerQuery: {
				tables: Array<{ tableName: string; selectedColumns: string[] }>;
				joins: Array<{
					sourceTable: string;
					sourceColumn: string;
					targetTable: string;
					targetColumn: string;
					joinType: 'INNER' | 'LEFT' | 'RIGHT' | 'FULL';
				}>;
				filters: Array<{
					column: string;
					operator: FilterOperator;
					value: string;
					connector: 'AND' | 'OR';
					subqueryIndex?: number;
				}>;
				groupBy: Array<{ column: string }>;
				having: Array<{
					aggregateFunction: AggregateFunction;
					column: string;
					operator: HavingOperator;
					value: string;
					connector: 'AND' | 'OR';
				}>;
				orderBy: Array<{ column: string; direction: 'ASC' | 'DESC' }>;
				limit: number | null;
				selectAggregates: Array<{
					function: AggregateFunction;
					expression: string;
					alias?: string;
				}>;
				columnAggregates?: Array<{
					tableName: string;
					column: string;
					function: AggregateFunction;
					alias?: string;
				}>;
				subqueries?: unknown[];
			};
		}>;
		ctes?: Array<{
			id: string;
			name: string;
			innerQuery: {
				tables: Array<{ tableName: string; selectedColumns: string[] }>;
				joins: Array<{
					sourceTable: string;
					sourceColumn: string;
					targetTable: string;
					targetColumn: string;
					joinType: 'INNER' | 'LEFT' | 'RIGHT' | 'FULL';
				}>;
				filters: Array<{
					column: string;
					operator: FilterOperator;
					value: string;
					connector: 'AND' | 'OR';
				}>;
				groupBy: Array<{ column: string }>;
				having: Array<{
					aggregateFunction: AggregateFunction;
					column: string;
					operator: HavingOperator;
					value: string;
					connector: 'AND' | 'OR';
				}>;
				orderBy: Array<{ column: string; direction: 'ASC' | 'DESC' }>;
				limit: number | null;
				selectAggregates: Array<{
					function: AggregateFunction;
					expression: string;
					alias?: string;
				}>;
				columnAggregates?: Array<{
					tableName: string;
					column: string;
					function: AggregateFunction;
					alias?: string;
				}>;
			};
		}>;
	}): void {
		// Build CTEs first (so we can identify CTE reference tables)
		const newCtes: CanvasCTE[] = [];
		const cteNameToId = new Map<string, string>(); // Map CTE name to its ID

		if (parsed.ctes && parsed.ctes.length > 0) {
			const existingCteMap = new Map(this.ctes.map((c) => [c.name, c]));

			for (let i = 0; i < parsed.ctes.length; i++) {
				const pc = parsed.ctes[i];
				const existing = existingCteMap.get(pc.name);

				// Build inner tables for the CTE
				const innerTables: CanvasTable[] = pc.innerQuery.tables.map((pt, idx) => ({
					id: generateId(),
					tableName: pt.tableName,
					position: { x: 20 + idx * 240, y: 50 },
					selectedColumns: new SvelteSet(this.expandSelectedColumns(pt.tableName, pt.selectedColumns)),
					columnAggregates: new Map(
						(pc.innerQuery.columnAggregates || [])
							.filter((ca) => ca.tableName === pt.tableName)
							.map((ca) => [ca.column, { function: ca.function, alias: ca.alias }])
					)
				}));

				// Build inner joins for the CTE
				const innerJoins: CanvasJoin[] = pc.innerQuery.joins.map((pj) => ({
					id: generateId(),
					sourceTable: pj.sourceTable,
					sourceColumn: pj.sourceColumn,
					targetTable: pj.targetTable,
					targetColumn: pj.targetColumn,
					joinType: pj.joinType
				}));

				// Build inner filters for the CTE
				const innerFilters: FilterCondition[] = pc.innerQuery.filters.map((pf) => ({
					id: generateId(),
					column: pf.column,
					operator: pf.operator,
					value: pf.value,
					connector: pf.connector
				}));

				// Build inner group by for the CTE
				const innerGroupBy: GroupByCondition[] = pc.innerQuery.groupBy.map((pg) => ({
					id: generateId(),
					column: pg.column
				}));

				// Build inner having for the CTE
				const innerHaving: HavingCondition[] = pc.innerQuery.having.map((ph) => ({
					id: generateId(),
					aggregateFunction: ph.aggregateFunction,
					column: ph.column,
					operator: ph.operator,
					value: ph.value,
					connector: ph.connector
				}));

				// Build inner order by for the CTE
				const innerOrderBy: SortCondition[] = pc.innerQuery.orderBy.map((po) => ({
					id: generateId(),
					column: po.column,
					direction: po.direction
				}));

				// Build inner select aggregates for the CTE
				const innerSelectAggregates: SelectAggregate[] = pc.innerQuery.selectAggregates.map((pa) => ({
					id: generateId(),
					function: pa.function,
					expression: pa.expression,
					alias: pa.alias
				}));

				const cteWidth = Math.max(300, 20 + pc.innerQuery.tables.length * 240 + 20);
				const cteId = existing?.id ?? generateId();

				newCtes.push({
					id: cteId,
					name: pc.name,
					position: existing?.position ?? { x: 50 + i * 350, y: 50 },
					size: existing?.size ?? { width: cteWidth, height: 350 },
					innerQuery: {
						tables: innerTables,
						joins: innerJoins,
						filters: innerFilters,
						groupBy: innerGroupBy,
						having: innerHaving,
						orderBy: innerOrderBy,
						limit: pc.innerQuery.limit,
						selectAggregates: innerSelectAggregates,
						subqueries: []
					}
				});

				cteNameToId.set(pc.name, cteId);
			}
		}

		// Build new tables with positions
		const newTables: CanvasTable[] = [];
		const existingTableMap = new Map(this.tables.map((t) => [t.tableName, t]));

		for (let i = 0; i < parsed.tables.length; i++) {
			const pt = parsed.tables[i];
			const existing = existingTableMap.get(pt.tableName);

			// Reuse existing position if table was already on canvas, otherwise auto-layout
			const position = existing?.position ?? { x: 50 + i * 280, y: 50 + (i % 2) * 150 };

			// Build columnAggregates map for this table
			const columnAggsForTable = new Map<string, ColumnAggregate>();
			for (const ca of parsed.columnAggregates) {
				if (ca.tableName === pt.tableName) {
					columnAggsForTable.set(ca.column, {
						function: ca.function,
						alias: ca.alias
					});
				}
			}

			// Check if this table references a CTE
			const cteId = cteNameToId.get(pt.tableName);

			newTables.push({
				id: existing?.id ?? generateId(),
				tableName: pt.tableName,
				position,
				selectedColumns: new SvelteSet(this.expandSelectedColumns(pt.tableName, pt.selectedColumns)),
				columnAggregates: columnAggsForTable,
				cteId // Will be undefined for regular tables, set for CTE references
			});
		}

		// Build new joins
		const newJoins: CanvasJoin[] = parsed.joins.map((pj) => ({
			id: generateId(),
			sourceTable: pj.sourceTable,
			sourceColumn: pj.sourceColumn,
			targetTable: pj.targetTable,
			targetColumn: pj.targetColumn,
			joinType: pj.joinType
		}));

		// Build subqueries recursively (supports nested subqueries)
		const subqueryResult = parsed.subqueries && parsed.subqueries.length > 0
			? this.buildSubqueriesFromParsed(parsed.subqueries, this.subqueries, { x: 400, y: 300 }, cteNameToId)
			: { subqueries: [], subqueryIdMap: new Map<number, string>() };
		const newSubqueries = subqueryResult.subqueries;
		const subqueryIdMap = subqueryResult.subqueryIdMap;

		// Build new filters with subquery links
		const newFilters: FilterCondition[] = parsed.filters.map((pf, index) => {
			const filter: FilterCondition = {
				id: generateId(),
				column: pf.column,
				operator: pf.operator,
				value: pf.value,
				connector: pf.connector
			};

			// Link to subquery if this filter uses one
			if (pf.subqueryIndex !== undefined) {
				const subqueryId = subqueryIdMap.get(pf.subqueryIndex);
				if (subqueryId) {
					filter.subqueryId = subqueryId;
					// Also update the subquery's linkedFilterId
					const subquery = newSubqueries.find((s) => s.id === subqueryId);
					if (subquery) {
						subquery.linkedFilterId = filter.id;
					}
				}
			}

			return filter;
		});

		// Build new group by
		const newGroupBy: GroupByCondition[] = parsed.groupBy.map((pg) => ({
			id: generateId(),
			column: pg.column
		}));

		// Build new having
		const newHaving: HavingCondition[] = parsed.having.map((ph) => ({
			id: generateId(),
			aggregateFunction: ph.aggregateFunction,
			column: ph.column,
			operator: ph.operator,
			value: ph.value,
			connector: ph.connector
		}));

		// Build new order by
		const newOrderBy: SortCondition[] = parsed.orderBy.map((po) => ({
			id: generateId(),
			column: po.column,
			direction: po.direction
		}));

		// Build new select aggregates
		const newSelectAggregates: SelectAggregate[] = parsed.selectAggregates.map((pa) => ({
			id: generateId(),
			function: pa.function,
			expression: pa.expression,
			alias: pa.alias
		}));

		// Apply all at once
		this.tables = newTables;
		this.joins = newJoins;
		this.filters = newFilters;
		this.groupBy = newGroupBy;
		this.having = newHaving;
		this.orderBy = newOrderBy;
		this.limit = parsed.limit;
		this.selectAggregates = newSelectAggregates;
		this.subqueries = newSubqueries;
		this.ctes = newCtes;
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

const QUERY_BUILDER_CONTEXT_KEY = 'query-builder';

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
	SerializableQueryBuilderState
} from './query-builder-serialization';
