/**
 * Pure SQL generation functions for the query builder.
 * Converts builder state into SQL strings.
 */

import type {
	CanvasTable,
	CanvasJoin,
	FilterCondition,
	GroupByCondition,
	HavingCondition,
	SortCondition,
	SelectAggregate,
	CanvasSubquery,
	SubqueryInnerState,
	CanvasCTE
} from '$lib/types';

/**
 * Check if a value is a template variable like {{my_var}}.
 */
function isTemplateVariable(value: string): boolean {
	return /^\{\{.+\}\}$/.test(value.trim());
}

/**
 * Build SQL from the current canvas state.
 * Delegates to buildQuerySql with top-level state.
 */
export function buildSql(
	tables: CanvasTable[],
	joins: CanvasJoin[],
	filters: FilterCondition[],
	groupBy: GroupByCondition[],
	having: HavingCondition[],
	orderBy: SortCondition[],
	limit: string | number | null,
	selectAggregates: SelectAggregate[],
	subqueries: CanvasSubquery[],
	ctes: CanvasCTE[]
): string {
	let sql = '';

	// Build WITH clause if CTEs exist
	if (ctes.length > 0) {
		const cteParts = ctes
			.filter((cte) => cte.name && cte.innerQuery.tables.length > 0)
			.map((cte) => {
				const innerSql = buildSubquerySql(cte.innerQuery);
				// Indent each line of the inner SQL
				const indentedInnerSql = innerSql
					.split('\n')
					.map((line) => '  ' + line)
					.join('\n');
				return `${cte.name} AS (\n${indentedInnerSql}\n)`;
			});

		if (cteParts.length > 0) {
			sql = `WITH ${cteParts.join(',\n')}\n`;
		}
	}

	// Build main query
	sql += buildQuerySql(
		tables,
		joins,
		filters,
		groupBy,
		having,
		orderBy,
		limit,
		selectAggregates,
		subqueries
	);

	return sql;
}

/**
 * Build SQL from query state (recursive for subqueries).
 */
export function buildQuerySql(
	tables: CanvasTable[],
	joins: CanvasJoin[],
	filters: FilterCondition[],
	groupBy: GroupByCondition[],
	having: HavingCondition[],
	orderBy: SortCondition[],
	limit: string | number | null,
	selectAggregates: SelectAggregate[],
	subqueries: CanvasSubquery[]
): string {
	// No tables and no FROM subqueries = empty query
	const fromSubqueries = subqueries.filter((s) => s.role === 'from');
	if (tables.length === 0 && fromSubqueries.length === 0) {
		return '';
	}

	// Collect all selected columns and aggregates
	const selectParts: string[] = [];

	for (const table of tables) {
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
	for (const agg of selectAggregates) {
		const expr = `${agg.function}(${agg.expression})`;
		selectParts.push(agg.alias ? `${expr} AS ${agg.alias}` : expr);
	}

	// Add SELECT subqueries (scalar subqueries)
	const selectSubqueries = subqueries.filter((s) => s.role === 'select');
	for (const sq of selectSubqueries) {
		const subquerySql = buildSubquerySql(sq.innerQuery);
		if (subquerySql) {
			const expr = `(${subquerySql})`;
			selectParts.push(sq.alias ? `${expr} AS ${sq.alias}` : expr);
		}
	}

	// If no columns selected, use * from first table (or first FROM subquery alias)
	let selectClause: string;
	if (selectParts.length > 0) {
		selectClause = selectParts.join(', ');
	} else if (tables.length > 0) {
		selectClause = `${tables[0].tableName}.*`;
	} else if (fromSubqueries.length > 0 && fromSubqueries[0].alias) {
		selectClause = `${fromSubqueries[0].alias}.*`;
	} else {
		selectClause = '*';
	}

	// Build FROM clause - start with first table or FROM subquery
	let fromClause: string;
	if (tables.length > 0) {
		fromClause = tables[0].tableName;
	} else if (fromSubqueries.length > 0) {
		const firstFromSq = fromSubqueries[0];
		const subquerySql = buildSubquerySql(firstFromSq.innerQuery);
		fromClause = `(${subquerySql}) AS ${firstFromSq.alias || 'subquery'}`;
	} else {
		fromClause = '';
	}

	// Add JOINs
	for (const join of joins) {
		fromClause += `\n  ${join.joinType} JOIN ${join.targetTable} ON ${join.sourceTable}.${join.sourceColumn} = ${join.targetTable}.${join.targetColumn}`;
	}

	// Add additional FROM subqueries (after the first one)
	for (let i = tables.length > 0 ? 0 : 1; i < fromSubqueries.length; i++) {
		const sq = fromSubqueries[i];
		const subquerySql = buildSubquerySql(sq.innerQuery);
		if (subquerySql) {
			// For simplicity, add as CROSS JOIN or the user can manually adjust
			fromClause += `,\n  (${subquerySql}) AS ${sq.alias || `subquery_${i}`}`;
		}
	}

	// Build WHERE clause with subquery support
	let whereClause = '';
	if (filters.length > 0) {
		const filterConditions = filters.map((f, index) => {
			const condition = buildFilterCondition(f, subqueries);
			// Don't add connector before the first condition
			if (index === 0) {
				return condition;
			}
			// Use the previous filter's connector
			const prevConnector = filters[index - 1].connector;
			return `${prevConnector} ${condition}`;
		});
		whereClause = `\nWHERE ${filterConditions.join('\n  ')}`;
	}

	// Build GROUP BY clause
	let groupByClause = '';
	if (groupBy.length > 0) {
		const groupByColumns = groupBy.map((g) => g.column);
		groupByClause = `\nGROUP BY ${groupByColumns.join(', ')}`;
	}

	// Build HAVING clause
	let havingClause = '';
	if (having.length > 0) {
		const havingConditions = having.map((h, index) => {
			const condition = buildHavingCondition(h);
			// Don't add connector before the first condition
			if (index === 0) {
				return condition;
			}
			// Use the previous having's connector
			const prevConnector = having[index - 1].connector;
			return `${prevConnector} ${condition}`;
		});
		havingClause = `\nHAVING ${havingConditions.join('\n  ')}`;
	}

	// Build ORDER BY clause
	let orderByClause = '';
	if (orderBy.length > 0) {
		const orderConditions = orderBy.map((o) => `${o.column} ${o.direction}`);
		orderByClause = `\nORDER BY ${orderConditions.join(', ')}`;
	}

	// Build LIMIT clause
	let limitClause = '';
	if (limit !== null) {
		limitClause = `\nLIMIT ${limit}`;
	}

	return `SELECT ${selectClause}\nFROM ${fromClause}${whereClause}${groupByClause}${havingClause}${orderByClause}${limitClause}`;
}

/**
 * Build SQL for a subquery's inner state (recursive).
 */
export function buildSubquerySql(inner: SubqueryInnerState): string {
	return buildQuerySql(
		inner.tables,
		inner.joins,
		inner.filters,
		inner.groupBy,
		inner.having,
		inner.orderBy,
		inner.limit,
		inner.selectAggregates,
		inner.subqueries
	);
}

/**
 * Build a single filter condition string.
 * Supports subquery values for WHERE subqueries.
 */
export function buildFilterCondition(
	filter: FilterCondition,
	subqueries: CanvasSubquery[] = []
): string {
	const { column, operator, value, subqueryId } = filter;

	// If filter is linked to a subquery, use subquery SQL as value
	if (subqueryId) {
		const subquery = subqueries.find((s) => s.id === subqueryId);
		if (subquery && subquery.innerQuery.tables.length > 0) {
			const subquerySql = buildSubquerySql(subquery.innerQuery);
			if (operator === 'IN') {
				return `${column} IN (${subquerySql})`;
			} else if (operator === 'NOT IN') {
				return `${column} NOT IN (${subquerySql})`;
			} else {
				// Scalar subquery comparison
				return `${column} ${operator} (${subquerySql})`;
			}
		}
	}

	switch (operator) {
		case 'IS NULL':
			return `${column} IS NULL`;
		case 'IS NOT NULL':
			return `${column} IS NOT NULL`;
		case 'IS TRUE':
			return `${column} IS TRUE`;
		case 'IS FALSE':
			return `${column} IS FALSE`;
		case 'IS NOT TRUE':
			return `${column} IS NOT TRUE`;
		case 'IS NOT FALSE':
			return `${column} IS NOT FALSE`;
		case 'IN':
			// Assume value is comma-separated list
			return `${column} IN (${value})`;
		case 'NOT IN':
			// Assume value is comma-separated list
			return `${column} NOT IN (${value})`;
		case 'BETWEEN':
			// Assume value is "low AND high"
			return `${column} BETWEEN ${value}`;
		case 'LIKE':
		case 'NOT LIKE':
			// Template variables pass through unquoted
			if (isTemplateVariable(value)) {
				return `${column} ${operator} ${value}`;
			}
			return `${column} ${operator} '${value}'`;
		default: {
			// Template variables pass through unquoted
			if (isTemplateVariable(value)) {
				return `${column} ${operator} ${value}`;
			}
			// Check if value looks like a number
			const isNumeric = !isNaN(Number(value)) && value.trim() !== '';
			const formattedValue = isNumeric ? value : `'${value}'`;
			return `${column} ${operator} ${formattedValue}`;
		}
	}
}

/**
 * Build a single HAVING condition string.
 */
export function buildHavingCondition(having: HavingCondition): string {
	const { aggregateFunction, column, operator, value } = having;
	// Use * for empty column (COUNT(*)), otherwise use the column name
	const columnPart = column === '' ? '*' : column;
	return `${aggregateFunction}(${columnPart}) ${operator} ${value}`;
}
