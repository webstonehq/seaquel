// src/lib/tutorial/sql-parser.ts
import { Parser } from 'node-sql-parser';
import type { FilterOperator, JoinType, AggregateFunction, HavingOperator } from '$lib/types';
import { getTable, getTableNames } from './schema';

const parser = new Parser();

export interface ParsedTable {
	tableName: string;
	alias: string | null;
	selectedColumns: string[];
}

export interface ParsedJoin {
	sourceTable: string;
	sourceColumn: string;
	targetTable: string;
	targetColumn: string;
	joinType: JoinType;
}

export interface ParsedFilter {
	column: string;
	operator: FilterOperator;
	value: string;
	connector: 'AND' | 'OR';
}

export interface ParsedOrderBy {
	column: string;
	direction: 'ASC' | 'DESC';
}

export interface ParsedGroupBy {
	column: string;
}

export interface ParsedHaving {
	aggregateFunction: AggregateFunction;
	column: string;
	operator: HavingOperator;
	value: string;
	connector: 'AND' | 'OR';
}

export interface ParsedSelectAggregate {
	function: AggregateFunction;
	expression: string;
	alias?: string;
}

export interface ParsedColumnAggregate {
	tableName: string;
	column: string;
	function: AggregateFunction;
	alias?: string;
}

export interface ParsedQuery {
	tables: ParsedTable[];
	joins: ParsedJoin[];
	filters: ParsedFilter[];
	groupBy: ParsedGroupBy[];
	having: ParsedHaving[];
	orderBy: ParsedOrderBy[];
	limit: number | null;
	selectAggregates: ParsedSelectAggregate[];
	columnAggregates: ParsedColumnAggregate[];
}

/**
 * Parse SQL and extract query builder components.
 * Returns null if SQL cannot be parsed or is not a SELECT statement.
 */
export function parseSql(sql: string): ParsedQuery | null {
	if (!sql.trim()) {
		return { tables: [], joins: [], filters: [], groupBy: [], having: [], orderBy: [], limit: null, selectAggregates: [], columnAggregates: [] };
	}

	try {
		const ast = parser.astify(sql, { database: 'PostgreSQL' });

		// Handle array of statements - take the first SELECT
		const stmt = Array.isArray(ast) ? ast[0] : ast;

		if (!stmt || stmt.type !== 'select') {
			return null;
		}

		const validTableNames = getTableNames();
		const tables: ParsedTable[] = [];
		const joins: ParsedJoin[] = [];
		const selectAggregates: ParsedSelectAggregate[] = [];
		const columnAggregates: ParsedColumnAggregate[] = [];
		const tableAliasMap = new Map<string, string>(); // alias -> tableName

		// Parse FROM clause
		if (stmt.from && Array.isArray(stmt.from)) {
			for (const fromItem of stmt.from) {
				if ('table' in fromItem && fromItem.table) {
					const tableName = fromItem.table;

					// Skip if table doesn't exist in schema
					if (!validTableNames.includes(tableName)) {
						continue;
					}

					const alias = fromItem.as || null;
					if (alias) {
						tableAliasMap.set(alias, tableName);
					}
					tableAliasMap.set(tableName, tableName);

					// Check if this is a JOIN
					if ('join' in fromItem && fromItem.join) {
						const joinType = normalizeJoinType(fromItem.join);

						// Parse the ON condition
						if (fromItem.on) {
							const joinInfo = parseJoinCondition(fromItem.on, tableAliasMap);
							if (joinInfo) {
								joins.push({
									...joinInfo,
									joinType,
									targetTable: tableName
								});
							}
						}

						tables.push({
							tableName,
							alias,
							selectedColumns: []
						});
					} else {
						// Regular FROM table
						tables.push({
							tableName,
							alias,
							selectedColumns: []
						});
					}
				}
			}
		}

		// Parse SELECT columns
		if (stmt.columns && Array.isArray(stmt.columns)) {
			for (const col of stmt.columns) {
				if (col === '*') {
					// SELECT * - select all columns from all tables
					for (const table of tables) {
						const tableSchema = getTable(table.tableName);
						if (tableSchema) {
							table.selectedColumns = tableSchema.columns.map((c) => c.name);
						}
					}
				} else if (col.expr) {
					const colExpr = col.expr;

					if (colExpr.type === 'star' && colExpr.table) {
						// table.* syntax
						const tableName = resolveTableName(colExpr.table, tableAliasMap);
						const table = tables.find((t) => t.tableName === tableName);
						if (table) {
							const tableSchema = getTable(tableName);
							if (tableSchema) {
								table.selectedColumns = tableSchema.columns.map((c) => c.name);
							}
						}
					} else if (colExpr.type === 'column_ref' && colExpr.column === '*') {
						// SELECT * parsed as column_ref with column: "*"
						if (colExpr.table) {
							// table.* syntax
							const tableName = resolveTableName(colExpr.table, tableAliasMap);
							const table = tables.find((t) => t.tableName === tableName);
							if (table) {
								const tableSchema = getTable(tableName);
								if (tableSchema) {
									table.selectedColumns = tableSchema.columns.map((c) => c.name);
								}
							}
						} else {
							// Plain * - select all columns from all tables
							for (const table of tables) {
								const tableSchema = getTable(table.tableName);
								if (tableSchema) {
									table.selectedColumns = tableSchema.columns.map((c) => c.name);
								}
							}
						}
					} else if (colExpr.type === 'column_ref') {
						// Regular column reference
						const columnName =
							typeof colExpr.column === 'string' ? colExpr.column : colExpr.column?.expr?.value;

						if (columnName) {
							let targetTable: ParsedTable | undefined;

							if (colExpr.table) {
								const tableName = resolveTableName(colExpr.table, tableAliasMap);
								targetTable = tables.find((t) => t.tableName === tableName);
							} else {
								// No table specified - find which table has this column
								targetTable = findTableForColumn(tables, columnName);
							}

							if (targetTable && !targetTable.selectedColumns.includes(columnName)) {
								targetTable.selectedColumns.push(columnName);
							}
						}
					} else if (colExpr.type === 'aggr_func' && colExpr.name) {
						// Check if it's an aggregate function
						const funcName = colExpr.name.toUpperCase();
						if (isAggregateFunction(funcName)) {
							const alias = col.as || undefined;
							const args = colExpr.args as { expr?: { type?: string; table?: string; column?: string | { expr?: { value?: string } }; value?: string } };

							if (args?.expr) {
								if (args.expr.type === 'star') {
									// COUNT(*) - standalone aggregate
									selectAggregates.push({
										function: funcName as AggregateFunction,
										expression: '*',
										alias
									});
								} else if (args.expr.type === 'column_ref') {
									// Aggregate on a specific column - per-column aggregate
									const columnName = typeof args.expr.column === 'string' ? args.expr.column : args.expr.column?.expr?.value;
									if (columnName) {
										const tableName = args.expr.table
											? resolveTableName(args.expr.table, tableAliasMap)
											: findTableForColumn(tables, columnName)?.tableName;

										if (tableName) {
											columnAggregates.push({
												tableName,
												column: columnName,
												function: funcName as AggregateFunction,
												alias
											});
											// Also select the column
											const table = tables.find((t) => t.tableName === tableName);
											if (table && !table.selectedColumns.includes(columnName)) {
												table.selectedColumns.push(columnName);
											}
										}
									}
								} else {
									// Expression aggregate (e.g., SUM(price * quantity)) - standalone
									selectAggregates.push({
										function: funcName as AggregateFunction,
										expression: '*', // Simplified - complex expressions become '*'
										alias
									});
								}
							}
						}
					}
				}
			}
		}

		// Parse WHERE clause
		const filters = stmt.where ? parseWhereClause(stmt.where, tableAliasMap, tables) : [];

		// Parse GROUP BY
		const groupBy: ParsedGroupBy[] = [];
		const groupByColumns = stmt.groupby?.columns;
		if (groupByColumns && Array.isArray(groupByColumns)) {
			for (const group of groupByColumns) {
				if (group.type === 'column_ref') {
					const columnNameRaw =
						typeof group.column === 'string' ? group.column : group.column?.expr?.value;
					const columnName = typeof columnNameRaw === 'string' ? columnNameRaw : undefined;

					if (columnName) {
						let fullColumn: string;
						if (group.table) {
							const tableName = resolveTableName(group.table, tableAliasMap);
							fullColumn = `${tableName}.${columnName}`;
						} else {
							const table = findTableForColumn(tables, columnName);
							fullColumn = table ? `${table.tableName}.${columnName}` : columnName;
						}

						groupBy.push({ column: fullColumn });
					}
				}
			}
		}

		// Parse HAVING
		const having: ParsedHaving[] = [];
		if (stmt.having) {
			parseHavingClause(stmt.having, having, 'AND');
		}

		// Parse ORDER BY
		const orderBy: ParsedOrderBy[] = [];
		if (stmt.orderby && Array.isArray(stmt.orderby)) {
			for (const order of stmt.orderby) {
				if (order.expr?.type === 'column_ref') {
					const columnName =
						typeof order.expr.column === 'string'
							? order.expr.column
							: order.expr.column?.expr?.value;

					if (columnName) {
						let fullColumn: string;
						if (order.expr.table) {
							const tableName = resolveTableName(order.expr.table, tableAliasMap);
							fullColumn = `${tableName}.${columnName}`;
						} else {
							const table = findTableForColumn(tables, columnName);
							fullColumn = table ? `${table.tableName}.${columnName}` : columnName;
						}

						orderBy.push({
							column: fullColumn,
							direction: (order.type?.toUpperCase() as 'ASC' | 'DESC') || 'ASC'
						});
					}
				}
			}
		}

		// Parse LIMIT
		let limit: number | null = null;
		if (stmt.limit?.value && Array.isArray(stmt.limit.value) && stmt.limit.value.length > 0) {
			const limitVal = stmt.limit.value[0];
			if (limitVal.type === 'number' && typeof limitVal.value === 'number') {
				limit = limitVal.value;
			}
		}

		return { tables, joins, filters, groupBy, having, orderBy, limit, selectAggregates, columnAggregates };
	} catch {
		// Parse error - return null to keep last valid state
		return null;
	}
}

function normalizeJoinType(joinStr: string): JoinType {
	const upper = joinStr.toUpperCase();
	if (upper.includes('LEFT')) return 'LEFT';
	if (upper.includes('RIGHT')) return 'RIGHT';
	if (upper.includes('FULL')) return 'FULL';
	return 'INNER';
}

function resolveTableName(nameOrAlias: string, aliasMap: Map<string, string>): string {
	return aliasMap.get(nameOrAlias) || nameOrAlias;
}

function findTableForColumn(tables: ParsedTable[], columnName: string): ParsedTable | undefined {
	for (const table of tables) {
		const tableSchema = getTable(table.tableName);
		if (tableSchema?.columns.some((c) => c.name === columnName)) {
			return table;
		}
	}
	return tables[0]; // Fallback to first table
}

interface JoinConditionInfo {
	sourceTable: string;
	sourceColumn: string;
	targetColumn: string;
}

function parseJoinCondition(
	condition: unknown,
	aliasMap: Map<string, string>
): JoinConditionInfo | null {
	const cond = condition as {
		type?: string;
		operator?: string;
		left?: { type?: string; table?: string; column?: string | { expr?: { value?: string } } };
		right?: { type?: string; table?: string; column?: string | { expr?: { value?: string } } };
	};

	if (cond.type !== 'binary_expr' || cond.operator !== '=') {
		return null;
	}

	const left = cond.left;
	const right = cond.right;

	if (left?.type !== 'column_ref' || right?.type !== 'column_ref') {
		return null;
	}

	const leftTable = left.table ? resolveTableName(left.table, aliasMap) : null;
	const rightTable = right.table ? resolveTableName(right.table, aliasMap) : null;
	const leftColumn = typeof left.column === 'string' ? left.column : left.column?.expr?.value;
	const rightColumn = typeof right.column === 'string' ? right.column : right.column?.expr?.value;

	if (!leftTable || !rightTable || !leftColumn || !rightColumn) {
		return null;
	}

	// The source is the table that was already in the FROM, target is the JOINed table
	return {
		sourceTable: leftTable,
		sourceColumn: leftColumn,
		targetColumn: rightColumn
	};
}

function parseWhereClause(
	where: unknown,
	aliasMap: Map<string, string>,
	tables: ParsedTable[]
): ParsedFilter[] {
	const filters: ParsedFilter[] = [];
	parseWhereRecursive(where, aliasMap, tables, filters, 'AND');
	return filters;
}

function parseWhereRecursive(
	expr: unknown,
	aliasMap: Map<string, string>,
	tables: ParsedTable[],
	filters: ParsedFilter[],
	connector: 'AND' | 'OR'
): void {
	const e = expr as {
		type?: string;
		operator?: string;
		left?: unknown;
		right?: unknown;
		table?: string;
		column?: string | { expr?: { value?: string } };
		value?: unknown;
	};

	if (!e || typeof e !== 'object') return;

	// Handle AND/OR
	if (e.type === 'binary_expr' && (e.operator === 'AND' || e.operator === 'OR')) {
		parseWhereRecursive(e.left, aliasMap, tables, filters, connector);
		parseWhereRecursive(e.right, aliasMap, tables, filters, e.operator as 'AND' | 'OR');
		return;
	}

	// Handle comparison operators
	if (e.type === 'binary_expr') {
		const left = e.left as {
			type?: string;
			table?: string;
			column?: string | { expr?: { value?: string } };
		};
		const right = e.right as { type?: string; value?: unknown };

		if (left?.type === 'column_ref') {
			const columnName =
				typeof left.column === 'string' ? left.column : left.column?.expr?.value;

			if (columnName) {
				let fullColumn: string;
				if (left.table) {
					const tableName = resolveTableName(left.table, aliasMap);
					fullColumn = `${tableName}.${columnName}`;
				} else {
					const table = findTableForColumn(tables, columnName);
					fullColumn = table ? `${table.tableName}.${columnName}` : columnName;
				}

				const operator = mapOperator(e.operator);
				const value = extractValue(right);

				if (operator && value !== null) {
					filters.push({
						column: fullColumn,
						operator,
						value,
						connector
					});
				}
			}
		}
	}

	// Handle IS NULL / IS NOT NULL
	if (e.type === 'unary_expr' || (e.type === 'binary_expr' && e.operator === 'IS')) {
		// Handle later if needed
	}
}

function mapOperator(op: string | undefined): FilterOperator | null {
	if (!op) return null;
	const opMap: Record<string, FilterOperator> = {
		'=': '=',
		'!=': '!=',
		'<>': '!=',
		'>': '>',
		'<': '<',
		'>=': '>=',
		'<=': '<=',
		LIKE: 'LIKE',
		'NOT LIKE': 'NOT LIKE',
		IN: 'IN',
		BETWEEN: 'BETWEEN'
	};
	return opMap[op.toUpperCase()] || null;
}

function extractValue(expr: { type?: string; value?: unknown } | undefined): string | null {
	if (!expr) return null;

	if (
		expr.type === 'number' ||
		expr.type === 'string' ||
		expr.type === 'single_quote_string' ||
		expr.type === 'double_quote_string'
	) {
		return String(expr.value);
	}

	return null;
}

/**
 * Parse HAVING clause and extract aggregate conditions.
 */
function parseHavingClause(
	expr: unknown,
	having: ParsedHaving[],
	connector: 'AND' | 'OR'
): void {
	const e = expr as {
		type?: string;
		operator?: string;
		left?: unknown;
		right?: unknown;
	};

	if (!e || typeof e !== 'object') return;

	// Handle AND/OR
	if (e.type === 'binary_expr' && (e.operator === 'AND' || e.operator === 'OR')) {
		parseHavingClause(e.left, having, connector);
		parseHavingClause(e.right, having, e.operator as 'AND' | 'OR');
		return;
	}

	// Handle comparison with aggregate function
	if (e.type === 'binary_expr') {
		const left = e.left as {
			type?: string;
			name?: string;
			args?: { expr?: { type?: string; column?: string | { expr?: { value?: string } } } };
		};
		const right = e.right as { type?: string; value?: unknown };

		// Check if left side is an aggregate function
		if (left?.type === 'aggr_func' && left.name) {
			const funcName = left.name.toUpperCase();
			if (isAggregateFunction(funcName)) {
				const operator = mapHavingOperator(e.operator);
				const value = extractValue(right);

				if (operator && value !== null) {
					// Extract column from aggregate function args
					let column = '';
					const args = left.args as {
						expr?: {
							type?: string;
							column?: string | { expr?: { value?: string } };
							value?: string;
						};
					};

					if (args?.expr) {
						if (args.expr.type === 'star') {
							column = ''; // COUNT(*)
						} else if (args.expr.type === 'column_ref') {
							column =
								typeof args.expr.column === 'string'
									? args.expr.column
									: args.expr.column?.expr?.value || '';
						}
					}

					having.push({
						aggregateFunction: funcName as AggregateFunction,
						column,
						operator,
						value,
						connector
					});
				}
			}
		}
	}
}

function isAggregateFunction(name: string): name is AggregateFunction {
	return ['COUNT', 'SUM', 'AVG', 'MIN', 'MAX'].includes(name);
}

function mapHavingOperator(op: string | undefined): HavingOperator | null {
	if (!op) return null;
	const opMap: Record<string, HavingOperator> = {
		'=': '=',
		'!=': '!=',
		'<>': '!=',
		'>': '>',
		'<': '<',
		'>=': '>=',
		'<=': '<='
	};
	return opMap[op.toUpperCase()] || null;
}
