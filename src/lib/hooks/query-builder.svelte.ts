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
	ColumnAggregate
} from '$lib/types';
import { getTable } from '$lib/tutorial/schema';

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

	/** LIMIT value, or null for no limit */
	limit = $state<number | null>(100);

	/** User's custom SQL text (preserved even if it differs from generated) */
	customSql = $state<string | null>(null);

	/** Standalone aggregates in SELECT clause */
	selectAggregates = $state<SelectAggregate[]>([]);

	// === DERIVED PROPERTIES ===

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
				columnAggregates: new Map(t.columnAggregates)
			})),
			joins: [...this.joins],
			filters: [...this.filters],
			groupBy: [...this.groupBy],
			having: [...this.having],
			orderBy: [...this.orderBy],
			limit: this.limit,
			selectAggregates: [...this.selectAggregates]
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
		const tableSchema = getTable(tableName);
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

		const tableSchema = getTable(table.tableName);
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
	 */
	private buildSql(): string {
		// No tables = empty query
		if (this.tables.length === 0) {
			return '';
		}

		// Collect all selected columns and aggregates
		const selectParts: string[] = [];

		for (const table of this.tables) {
			for (const column of table.selectedColumns) {
				const agg = table.columnAggregates.get(column);
				if (agg) {
					// Column with aggregate
					const expr = `${agg.function}(${table.tableName}.${column})`;
					selectParts.push(agg.alias ? `${expr} AS ${agg.alias}` : expr);
				} else {
					// Regular column
					selectParts.push(`${table.tableName}.${column}`);
				}
			}
		}

		// Add standalone aggregates
		for (const agg of this.selectAggregates) {
			const expr = `${agg.function}(${agg.expression})`;
			selectParts.push(agg.alias ? `${expr} AS ${agg.alias}` : expr);
		}

		// If no columns selected, use * from first table
		const selectClause =
			selectParts.length > 0 ? selectParts.join(', ') : `${this.tables[0].tableName}.*`;

		// Build FROM clause - start with first table
		const fromTable = this.tables[0].tableName;
		let fromClause = fromTable;

		// Add JOINs
		for (const join of this.joins) {
			fromClause += `\n  ${join.joinType} JOIN ${join.targetTable} ON ${join.sourceTable}.${join.sourceColumn} = ${join.targetTable}.${join.targetColumn}`;
		}

		// Build WHERE clause
		let whereClause = '';
		if (this.filters.length > 0) {
			const filterConditions = this.filters.map((f, index) => {
				const condition = this.buildFilterCondition(f);
				// Don't add connector before the first condition
				if (index === 0) {
					return condition;
				}
				// Use the previous filter's connector
				const prevConnector = this.filters[index - 1].connector;
				return `${prevConnector} ${condition}`;
			});
			whereClause = `\nWHERE ${filterConditions.join('\n  ')}`;
		}

		// Build GROUP BY clause
		let groupByClause = '';
		if (this.groupBy.length > 0) {
			const groupByColumns = this.groupBy.map((g) => g.column);
			groupByClause = `\nGROUP BY ${groupByColumns.join(', ')}`;
		}

		// Build HAVING clause
		let havingClause = '';
		if (this.having.length > 0) {
			const havingConditions = this.having.map((h, index) => {
				const condition = this.buildHavingCondition(h);
				// Don't add connector before the first condition
				if (index === 0) {
					return condition;
				}
				// Use the previous having's connector
				const prevConnector = this.having[index - 1].connector;
				return `${prevConnector} ${condition}`;
			});
			havingClause = `\nHAVING ${havingConditions.join('\n  ')}`;
		}

		// Build ORDER BY clause
		let orderByClause = '';
		if (this.orderBy.length > 0) {
			const orderConditions = this.orderBy.map((o) => `${o.column} ${o.direction}`);
			orderByClause = `\nORDER BY ${orderConditions.join(', ')}`;
		}

		// Build LIMIT clause
		let limitClause = '';
		if (this.limit !== null) {
			limitClause = `\nLIMIT ${this.limit}`;
		}

		return `SELECT ${selectClause}\nFROM ${fromClause}${whereClause}${groupByClause}${havingClause}${orderByClause}${limitClause}`;
	}

	/**
	 * Build a single filter condition string.
	 */
	private buildFilterCondition(filter: FilterCondition): string {
		const { column, operator, value } = filter;

		switch (operator) {
			case 'IS NULL':
				return `${column} IS NULL`;
			case 'IS NOT NULL':
				return `${column} IS NOT NULL`;
			case 'IN':
				// Assume value is comma-separated list
				return `${column} IN (${value})`;
			case 'BETWEEN':
				// Assume value is "low AND high"
				return `${column} BETWEEN ${value}`;
			case 'LIKE':
			case 'NOT LIKE':
				return `${column} ${operator} '${value}'`;
			default:
				// Check if value looks like a number
				const isNumeric = !isNaN(Number(value)) && value.trim() !== '';
				const formattedValue = isNumeric ? value : `'${value}'`;
				return `${column} ${operator} ${formattedValue}`;
		}
	}

	/**
	 * Build a single HAVING condition string.
	 */
	private buildHavingCondition(having: HavingCondition): string {
		const { aggregateFunction, column, operator, value } = having;
		// Use * for empty column (COUNT(*)), otherwise use the column name
		const columnPart = column === '' ? '*' : column;
		return `${aggregateFunction}(${columnPart}) ${operator} ${value}`;
	}

	// === APPLY FROM PARSED SQL ===

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
	}): void {
		// Build new tables with positions
		const newTables: CanvasTable[] = [];
		const existingTableMap = new Map(this.tables.map((t) => [t.tableName, t]));

		for (let i = 0; i < parsed.tables.length; i++) {
			const pt = parsed.tables[i];
			const existing = existingTableMap.get(pt.tableName);

			// Reuse existing position if table was already on canvas, otherwise auto-layout
			const position = existing?.position ?? { x: 50 + i * 280, y: 50 + (i % 2) * 150 };

			newTables.push({
				id: existing?.id ?? generateId(),
				tableName: pt.tableName,
				position,
				selectedColumns: new SvelteSet(pt.selectedColumns),
				columnAggregates: existing?.columnAggregates ?? new Map<string, ColumnAggregate>()
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

		// Build new filters
		const newFilters: FilterCondition[] = parsed.filters.map((pf) => ({
			id: generateId(),
			column: pf.column,
			operator: pf.operator,
			value: pf.value,
			connector: pf.connector
		}));

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

		// Apply all at once
		this.tables = newTables;
		this.joins = newJoins;
		this.filters = newFilters;
		this.groupBy = newGroupBy;
		this.having = newHaving;
		this.orderBy = newOrderBy;
		this.limit = parsed.limit;
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
	}

	// === SERIALIZATION ===

	/**
	 * Get a serializable version of the state (for persistence).
	 * Converts Sets to arrays.
	 */
	toSerializable(): SerializableQueryBuilderState {
		return {
			tables: this.tables.map((t) => ({
				id: t.id,
				tableName: t.tableName,
				position: t.position,
				selectedColumns: Array.from(t.selectedColumns),
				columnAggregates: Object.fromEntries(t.columnAggregates)
			})),
			joins: [...this.joins],
			filters: [...this.filters],
			groupBy: [...this.groupBy],
			having: [...this.having],
			orderBy: [...this.orderBy],
			limit: this.limit,
			customSql: this.customSql,
			selectAggregates: [...this.selectAggregates]
		};
	}

	/**
	 * Restore state from a serialized snapshot.
	 */
	fromSerializable(state: SerializableQueryBuilderState): void {
		this.tables = state.tables.map((t) => ({
			id: t.id,
			tableName: t.tableName,
			position: t.position,
			selectedColumns: new SvelteSet(t.selectedColumns),
			columnAggregates: new Map(
				Object.entries(t.columnAggregates ?? {}) as [string, ColumnAggregate][]
			)
		}));
		this.joins = state.joins.map((j) => ({ ...j }));
		this.filters = state.filters.map((f) => ({ ...f }));
		this.groupBy = (state.groupBy ?? []).map((g) => ({ ...g }));
		this.having = (state.having ?? []).map((h) => ({ ...h }));
		this.orderBy = state.orderBy.map((o) => ({ ...o }));
		this.limit = state.limit;
		this.customSql = state.customSql ?? null;
		this.selectAggregates = (state.selectAggregates ?? []).map((a) => ({ ...a }));
	}
}

/**
 * Serializable version of query builder state for persistence.
 */
export interface SerializableQueryBuilderState {
	tables: Array<{
		id: string;
		tableName: string;
		position: { x: number; y: number };
		selectedColumns: string[];
		columnAggregates?: Record<string, ColumnAggregate>;
	}>;
	joins: CanvasJoin[];
	filters: FilterCondition[];
	groupBy?: GroupByCondition[];
	having?: HavingCondition[];
	orderBy: SortCondition[];
	limit: number | null;
	/** User's custom SQL text (preserved even if it differs from generated) */
	customSql?: string | null;
	/** Standalone aggregates in SELECT clause */
	selectAggregates?: SelectAggregate[];
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
