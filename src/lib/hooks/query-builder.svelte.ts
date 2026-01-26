import { setContext, getContext } from 'svelte';
import { SvelteSet } from 'svelte/reactivity';
import type {
	CanvasTable,
	CanvasJoin,
	FilterCondition,
	FilterOperator,
	SortCondition,
	SortDirection,
	JoinType,
	QueryBuilderSnapshot
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

	/** ORDER BY clauses */
	orderBy = $state<SortCondition[]>([]);

	/** LIMIT value, or null for no limit */
	limit = $state<number | null>(100);

	/** SQL override when user manually edits the generated SQL */
	private _sqlOverride = $state<string | null>(null);

	/** Whether the builder is in visual mode (vs SQL editing mode) */
	private _isVisualMode = $state<boolean>(true);

	// === DERIVED PROPERTIES ===

	/**
	 * Generated SQL query from the current canvas state.
	 * Returns the override if set, otherwise builds from visual state.
	 */
	generatedSql = $derived.by(() => {
		if (this._sqlOverride !== null) {
			return this._sqlOverride;
		}
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
				selectedColumns: new Set(t.selectedColumns)
			})),
			joins: [...this.joins],
			filters: [...this.filters],
			orderBy: [...this.orderBy],
			limit: this.limit
		};
	}

	/**
	 * Whether visual mode is active.
	 */
	get isVisualMode(): boolean {
		return this._isVisualMode;
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
			selectedColumns: new SvelteSet<string>()
		};

		this.tables = [...this.tables, canvasTable];
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
		} else {
			table.selectedColumns.add(columnName);
		}
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
	}

	/**
	 * Clear all selected columns in a table.
	 * @param tableId - ID of the canvas table
	 */
	clearColumns(tableId: string): void {
		const table = this.tables.find((t) => t.id === tableId);
		if (!table) return;

		table.selectedColumns.clear();
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
		return join;
	}

	/**
	 * Update the type of an existing join.
	 * @param joinId - ID of the join
	 * @param joinType - New join type
	 */
	updateJoinType(joinId: string, joinType: JoinType): void {
		this.joins = this.joins.map((j) => (j.id === joinId ? { ...j, joinType } : j));
	}

	/**
	 * Remove a join.
	 * @param joinId - ID of the join to remove
	 */
	removeJoin(joinId: string): void {
		this.joins = this.joins.filter((j) => j.id !== joinId);
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
		return filter;
	}

	/**
	 * Update an existing filter.
	 * @param filterId - ID of the filter
	 * @param updates - Partial filter updates
	 */
	updateFilter(filterId: string, updates: Partial<Omit<FilterCondition, 'id'>>): void {
		this.filters = this.filters.map((f) => (f.id === filterId ? { ...f, ...updates } : f));
	}

	/**
	 * Remove a filter.
	 * @param filterId - ID of the filter to remove
	 */
	removeFilter(filterId: string): void {
		this.filters = this.filters.filter((f) => f.id !== filterId);
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
		return sortCondition;
	}

	/**
	 * Update the direction of an ORDER BY clause.
	 * @param orderId - ID of the sort condition
	 * @param direction - New sort direction
	 */
	updateOrderBy(orderId: string, direction: SortDirection): void {
		this.orderBy = this.orderBy.map((o) => (o.id === orderId ? { ...o, direction } : o));
	}

	/**
	 * Remove an ORDER BY clause.
	 * @param orderId - ID of the sort condition to remove
	 */
	removeOrderBy(orderId: string): void {
		this.orderBy = this.orderBy.filter((o) => o.id !== orderId);
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
	}

	// === LIMIT ===

	/**
	 * Set the LIMIT value.
	 * @param limit - Limit value, or null for no limit
	 */
	setLimit(limit: number | null): void {
		this.limit = limit;
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

		// Collect all selected columns
		const selectColumns: string[] = [];
		for (const table of this.tables) {
			for (const column of table.selectedColumns) {
				selectColumns.push(`${table.tableName}.${column}`);
			}
		}

		// If no columns selected, use * from first table
		const selectClause =
			selectColumns.length > 0 ? selectColumns.join(', ') : `${this.tables[0].tableName}.*`;

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

		return `SELECT ${selectClause}\nFROM ${fromClause}${whereClause}${orderByClause}${limitClause}`;
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

	// === SQL SYNC ===

	/**
	 * Set or clear the SQL override.
	 * When set, generatedSql returns this instead of building from visual state.
	 * @param sql - SQL to use, or null to clear override
	 */
	setSqlOverride(sql: string | null): void {
		this._sqlOverride = sql;
	}

	/**
	 * Set visual mode state.
	 * @param isVisual - Whether to enable visual mode
	 */
	setVisualMode(isVisual: boolean): void {
		this._isVisualMode = isVisual;
		if (isVisual) {
			// Clear override when switching back to visual mode
			this._sqlOverride = null;
		}
	}

	// === RESET ===

	/**
	 * Reset all state to defaults.
	 */
	reset(): void {
		this.tables = [];
		this.joins = [];
		this.filters = [];
		this.orderBy = [];
		this.limit = 100;
		this._sqlOverride = null;
		this._isVisualMode = true;
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
