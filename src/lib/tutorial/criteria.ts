// src/lib/tutorial/criteria.ts
import type { QueryBuilderSnapshot, ChallengeCriterion } from '$lib/types';

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
 * Check if the SQL contains a specific keyword (case-insensitive).
 */
export function sqlContains(keyword: string): CriterionCheck {
	return (_, sql) => sql.toUpperCase().includes(keyword.toUpperCase());
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
