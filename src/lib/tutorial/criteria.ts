// src/lib/tutorial/criteria.ts
import type {
	QueryBuilderSnapshot,
	ChallengeCriterion,
	SubqueryRole,
	AggregateFunction,
	CanvasSubquery
} from '$lib/types';

type CriterionCheck = (state: QueryBuilderSnapshot, sql: string) => boolean;

/**
 * Check if a specific table is on the canvas.
 */
export function hasTable(tableName: string): CriterionCheck {
	return (state) => state.tables.some((t) => t.tableName === tableName);
}

/**
 * Check if a specific column is selected in a table.
 */
export function hasColumn(tableName: string, columnName: string): CriterionCheck {
	return (state) => {
		const table = state.tables.find((t) => t.tableName === tableName);
		return table ? table.selectedColumns.has(columnName) : false;
	};
}

/**
 * Check if any columns are selected.
 */
export function hasAnyColumns(): CriterionCheck {
	return (state) => state.tables.some((t) => t.selectedColumns.size > 0);
}

/**
 * Check if at least N columns are selected total.
 */
export function hasAtLeastColumns(count: number): CriterionCheck {
	return (state) => {
		const total = state.tables.reduce((sum, t) => sum + t.selectedColumns.size, 0);
		return total >= count;
	};
}

/**
 * Check if a JOIN exists between two tables.
 */
export function hasJoin(table1: string, table2: string): CriterionCheck {
	return (state) =>
		state.joins.some(
			(j) =>
				(j.sourceTable === table1 && j.targetTable === table2) ||
				(j.sourceTable === table2 && j.targetTable === table1)
		);
}

/**
 * Check if a JOIN exists between two tables on specific columns.
 * Validates the exact join condition (e.g., products.category_id = categories.id).
 */
export function hasJoinOn(
	table1: string,
	column1: string,
	table2: string,
	column2: string
): CriterionCheck {
	return (state) =>
		state.joins.some(
			(j) =>
				(j.sourceTable === table1 &&
					j.sourceColumn === column1 &&
					j.targetTable === table2 &&
					j.targetColumn === column2) ||
				(j.sourceTable === table2 &&
					j.sourceColumn === column2 &&
					j.targetTable === table1 &&
					j.targetColumn === column1)
		);
}

/**
 * Check if a specific join type is used.
 */
export function hasJoinType(joinType: 'INNER' | 'LEFT' | 'RIGHT' | 'FULL'): CriterionCheck {
	return (state) => state.joins.some((j) => j.joinType === joinType);
}

/**
 * Check if a WHERE filter exists on a column with a specific operator.
 */
export function hasFilter(
	column: string,
	operator?: string,
	value?: string | number
): CriterionCheck {
	return (state) =>
		state.filters.some((f) => {
			if (f.column !== column) return false;
			if (operator && f.operator !== operator) return false;
			if (value !== undefined && f.value !== String(value)) return false;
			return true;
		});
}

/**
 * Check if any WHERE filter exists.
 */
export function hasAnyFilter(): CriterionCheck {
	return (state) => state.filters.length > 0;
}

/**
 * Check if ORDER BY is applied on a specific column.
 */
export function hasOrderBy(column: string, direction?: 'ASC' | 'DESC'): CriterionCheck {
	return (state) =>
		state.orderBy.some((o) => {
			if (o.column !== column) return false;
			if (direction && o.direction !== direction) return false;
			return true;
		});
}

/**
 * Check if any ORDER BY is applied.
 */
export function hasAnyOrderBy(): CriterionCheck {
	return (state) => state.orderBy.length > 0;
}

/**
 * Check if LIMIT is set to a specific value.
 */
export function hasLimit(value?: number): CriterionCheck {
	return (state) => {
		if (value === undefined) {
			return state.limit !== null;
		}
		return state.limit === value;
	};
}

/**
 * Check if a GROUP BY clause exists on a specific column.
 */
export function hasGroupBy(column: string): CriterionCheck {
	return (state) => state.groupBy.some((g) => g.column === column);
}

/**
 * Check if any GROUP BY clause exists.
 */
export function hasAnyGroupBy(): CriterionCheck {
	return (state) => state.groupBy.length > 0;
}

/**
 * Check if GROUP BY has at least one valid (non-empty) column.
 */
export function hasValidGroupBy(): CriterionCheck {
	return (state) => state.groupBy.some((g) => g.column.trim() !== '');
}

/**
 * Check if the SQL contains a specific keyword (case-insensitive).
 */
export function sqlContains(keyword: string): CriterionCheck {
	return (_, sql) => sql.toUpperCase().includes(keyword.toUpperCase());
}

/**
 * Check if a HAVING condition exists with specific aggregate function and operator.
 * @param aggregateFunction - The aggregate function (COUNT, SUM, AVG, MIN, MAX)
 * @param column - The column inside the aggregate (empty string for COUNT(*))
 * @param operator - The comparison operator
 * @param value - Optional specific value to check
 */
export function hasHaving(
	aggregateFunction: 'COUNT' | 'SUM' | 'AVG' | 'MIN' | 'MAX',
	column: string,
	operator?: '=' | '!=' | '>' | '<' | '>=' | '<=',
	value?: string | number
): CriterionCheck {
	return (state) =>
		state.having.some((h) => {
			if (h.aggregateFunction !== aggregateFunction) return false;
			// For column, check if it ends with the specified column (handles table.column format)
			if (column === '' || column === '*') {
				if (h.column !== '' && h.column !== '*') return false;
			} else {
				if (!h.column.endsWith(column) && h.column !== column) return false;
			}
			if (operator && h.operator !== operator) return false;
			if (value !== undefined && h.value !== String(value)) return false;
			return true;
		});
}

/**
 * Check if any HAVING condition exists.
 */
export function hasAnyHaving(): CriterionCheck {
	return (state) => state.having.length > 0;
}

/**
 * Check if a HAVING condition exists with a specific comparison (operator + value).
 * More flexible than hasHaving - checks aggregate function and that the comparison makes sense.
 */
export function hasHavingComparison(
	aggregateFunction: 'COUNT' | 'SUM' | 'AVG' | 'MIN' | 'MAX',
	operator: '=' | '!=' | '>' | '<' | '>=' | '<=',
	value: string | number
): CriterionCheck {
	return (state) =>
		state.having.some((h) => {
			if (h.aggregateFunction !== aggregateFunction) return false;
			if (h.operator !== operator) return false;
			if (h.value !== String(value)) return false;
			return true;
		});
}

/**
 * Create a ChallengeCriterion from a description and check function.
 */
export function criterion(
	id: string,
	description: string,
	check: CriterionCheck
): Omit<ChallengeCriterion, 'satisfied'> {
	return { id, description, check };
}

// === SUBQUERY CRITERIA ===

/**
 * Check if a subquery with a specific role exists.
 * @param role - The role of the subquery ('where', 'from', or 'select')
 */
export function hasSubquery(role: SubqueryRole): CriterionCheck {
	return (state) => state.subqueries.some((s) => s.role === role);
}

/**
 * Check if any subquery exists.
 */
export function hasAnySubquery(): CriterionCheck {
	return (state) => state.subqueries.length > 0;
}

/**
 * Check if a subquery contains a specific table.
 * @param tableName - The name of the table to check for
 */
export function hasSubqueryWithTable(tableName: string): CriterionCheck {
	return (state) =>
		state.subqueries.some((s) => s.innerQuery.tables.some((t) => t.tableName === tableName));
}

/**
 * Check if a subquery contains a specific aggregate function.
 * @param func - The aggregate function to check for
 */
export function hasSubqueryAggregate(func: AggregateFunction): CriterionCheck {
	return (state) =>
		state.subqueries.some(
			(s) =>
				s.innerQuery.selectAggregates.some((a) => a.function === func) ||
				s.innerQuery.tables.some((t) =>
					Array.from(t.columnAggregates.values()).some((ca) => ca.function === func)
				)
		);
}

/**
 * Check if a subquery with a specific role contains a table.
 * @param role - The role of the subquery
 * @param tableName - The table name
 */
export function hasSubqueryRoleWithTable(role: SubqueryRole, tableName: string): CriterionCheck {
	return (state) =>
		state.subqueries.some(
			(s) => s.role === role && s.innerQuery.tables.some((t) => t.tableName === tableName)
		);
}

/**
 * Check if a subquery has selected columns.
 */
export function hasSubqueryWithSelectedColumns(): CriterionCheck {
	return (state) =>
		state.subqueries.some((s) => s.innerQuery.tables.some((t) => t.selectedColumns.size > 0));
}

/**
 * Check if a WHERE filter is linked to a subquery.
 */
export function hasFilterWithSubquery(): CriterionCheck {
	return (state) => state.filters.some((f) => f.subqueryId !== undefined);
}

/**
 * Recursively check all subqueries (including nested) for a condition.
 * @param predicate - Function to check on each subquery
 */
export function hasSubqueryMatching(
	predicate: (subquery: CanvasSubquery) => boolean
): CriterionCheck {
	const checkRecursive = (subqueries: CanvasSubquery[]): boolean => {
		for (const sq of subqueries) {
			if (predicate(sq)) return true;
			if (checkRecursive(sq.innerQuery.subqueries)) return true;
		}
		return false;
	};

	return (state) => checkRecursive(state.subqueries);
}

/**
 * Check if nested subqueries exist (subquery inside subquery).
 */
export function hasNestedSubquery(): CriterionCheck {
	return (state) => state.subqueries.some((s) => s.innerQuery.subqueries.length > 0);
}
