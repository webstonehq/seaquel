// src/lib/tutorial/sql-parser.ts
import { Parser } from 'node-sql-parser';
import type { FilterOperator, JoinType, AggregateFunction, HavingOperator } from '$lib/types';
import { getTable, getTableNames } from './schema';

const parser = new Parser();

export interface ParsedTable {
	tableName: string;
	alias: string | null;
	selectedColumns: string[];
	isCteReference?: boolean;
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
	subqueryIndex?: number; // Index into subqueries array if value is a subquery
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

export interface ParsedSubquery {
	id: string;
	role: 'where' | 'from' | 'select';
	linkedFilterIndex?: number; // Index of filter that uses this subquery
	innerQuery: ParsedQuery;
}

export interface ParsedCTE {
	id: string;
	name: string;
	innerQuery: ParsedQuery;
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
	subqueries: ParsedSubquery[];
	ctes?: ParsedCTE[];
}

/**
 * Options for SQL parsing.
 */
export interface ParseSqlOptions {
	/**
	 * List of valid table names to accept. If not provided, uses tutorial schema.
	 * Pass null to accept all table names (for real database schemas).
	 */
	validTableNames?: string[] | null;
}

/**
 * Parse SQL and extract query builder components.
 * Returns null if SQL cannot be parsed or is not a SELECT statement.
 * @param sql - The SQL string to parse
 * @param options - Optional parsing options
 */
export function parseSql(sql: string, options?: ParseSqlOptions): ParsedQuery | null {
	if (!sql.trim()) {
		return { tables: [], joins: [], filters: [], groupBy: [], having: [], orderBy: [], limit: null, selectAggregates: [], columnAggregates: [], subqueries: [], ctes: [] };
	}

	try {
		const ast = parser.astify(sql, { database: 'PostgreSQL' });

		// Handle array of statements - take the first SELECT
		const stmt = Array.isArray(ast) ? ast[0] : ast;

		if (!stmt || stmt.type !== 'select') {
			return null;
		}

		// If validTableNames is null, accept all tables. If undefined, use tutorial schema.
		const validTableNames = options?.validTableNames === null
			? null
			: (options?.validTableNames ?? getTableNames());
		const tables: ParsedTable[] = [];
		const joins: ParsedJoin[] = [];
		const selectAggregates: ParsedSelectAggregate[] = [];
		const columnAggregates: ParsedColumnAggregate[] = [];
		const tableAliasMap = new Map<string, string>(); // alias -> tableName
		const subqueries: ParsedSubquery[] = []; // Moved early to support FROM subqueries
		const ctes: ParsedCTE[] = [];
		const cteNameSet = new Set<string>(); // Track CTE names for table resolution

		// Parse WITH clause (CTEs)
		// node-sql-parser structure: { with: [{ name: { type, value }, stmt: { type: 'select', ... } }] }
		const stmtWithCte = stmt as { with?: Array<{ name?: { value?: string }; stmt?: unknown }> };
		if (stmtWithCte.with && Array.isArray(stmtWithCte.with)) {
			for (const cteItem of stmtWithCte.with) {
				const cteName = cteItem.name?.value;
				// The stmt is the full SELECT AST directly (not wrapped in { ast: ... })
				if (cteName && cteItem.stmt) {
					const cteQuery = parseSubqueryAst(cteItem.stmt, undefined, validTableNames);
					if (cteQuery) {
						ctes.push({
							id: `cte-${ctes.length}`,
							name: cteName,
							innerQuery: cteQuery
						});
						cteNameSet.add(cteName);
						// Register CTE name as a valid "table" for column resolution
						tableAliasMap.set(cteName, cteName);
					}
				}
			}
		}

		// Parse FROM clause (handles regular tables and derived tables/subqueries)
		if (stmt.from && Array.isArray(stmt.from)) {
			for (const fromItem of stmt.from) {
				// Check for derived table (FROM (SELECT ...) AS alias)
				const fi = fromItem as {
					table?: string;
					as?: string | null;
					join?: string;
					on?: unknown;
					expr?: { ast?: unknown; parentheses?: boolean };
				};

				if (fi.expr?.ast) {
					const derivedTableParsed = parseSubqueryAst(fi.expr.ast, cteNameSet, validTableNames);
					if (derivedTableParsed) {
						const subqueryIndex = subqueries.length;
						subqueries.push({
							id: `subquery-${subqueryIndex}`,
							role: 'from',
							innerQuery: derivedTableParsed
						});
						// Register alias in the alias map for column resolution
						if (fi.as) {
							tableAliasMap.set(fi.as, fi.as);
						}
					}
					continue;
				}

				if ('table' in fromItem && fromItem.table) {
					const tableName = fromItem.table;

					// Skip if table doesn't exist in schema AND is not a CTE reference
					// (unless validTableNames is null, which means accept all tables)
					if (validTableNames !== null && !validTableNames.includes(tableName) && !cteNameSet.has(tableName)) {
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
								} else if (cteNameSet.has(table.tableName)) {
									// For CTE references, mark as "select all" by leaving selectedColumns empty
									// The actual columns will be derived from the CTE when rendering
									table.selectedColumns = ['*'];
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

		// Parse WHERE clause (also extracts subqueries - uses the subqueries array from above)
		const filters = stmt.where ? parseWhereClause(stmt.where, tableAliasMap, tables, subqueries, cteNameSet, validTableNames) : [];

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

		// Parse HAVING (also extracts subqueries from HAVING comparisons)
		const having: ParsedHaving[] = [];
		if (stmt.having) {
			parseHavingClause(stmt.having, having, 'AND', subqueries, cteNameSet, validTableNames);
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

		return { tables, joins, filters, groupBy, having, orderBy, limit, selectAggregates, columnAggregates, subqueries, ctes };
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

function resolveTableName(nameOrAlias: string | { type?: string; value?: string } | undefined, aliasMap: Map<string, string>): string {
	// Handle object form: { type: "default", value: "tablename" }
	if (typeof nameOrAlias === 'object' && nameOrAlias !== null) {
		const resolved = nameOrAlias.value || '';
		return aliasMap.get(resolved) || resolved;
	}
	return aliasMap.get(nameOrAlias || '') || nameOrAlias || '';
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
	tables: ParsedTable[],
	subqueries: ParsedSubquery[],
	cteNameSet?: Set<string>,
	validTableNames?: string[] | null
): ParsedFilter[] {
	const filters: ParsedFilter[] = [];
	parseWhereRecursive(where, aliasMap, tables, filters, subqueries, 'AND', cteNameSet, validTableNames);
	return filters;
}

function parseWhereRecursive(
	expr: unknown,
	aliasMap: Map<string, string>,
	tables: ParsedTable[],
	filters: ParsedFilter[],
	subqueries: ParsedSubquery[],
	connector: 'AND' | 'OR',
	cteNameSet?: Set<string>,
	validTableNames?: string[] | null
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
		parseWhereRecursive(e.left, aliasMap, tables, filters, subqueries, connector, cteNameSet, validTableNames);
		parseWhereRecursive(e.right, aliasMap, tables, filters, subqueries, e.operator as 'AND' | 'OR', cteNameSet, validTableNames);
		return;
	}

	// Handle comparison operators
	if (e.type === 'binary_expr') {
		const left = e.left as {
			type?: string;
			table?: string;
			column?: string | { expr?: { value?: string } };
		};
		const right = e.right as { type?: string; value?: unknown; ast?: unknown };

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

				// Check if right side is a subquery
				// Case 1: Scalar subquery (price > (SELECT ...)) - right.ast contains the subquery
				// Case 2: IN/NOT IN subquery (id IN (SELECT ...)) - right.type === 'expr_list' && right.value[0].ast
				const rightAsExprList = right as { type?: string; value?: Array<{ ast?: unknown }> };
				let subqueryAst: unknown = null;

				if (right?.type === 'select') {
					subqueryAst = right;
				} else if (right?.ast) {
					subqueryAst = right.ast;
				} else if (rightAsExprList?.type === 'expr_list' &&
				           rightAsExprList.value?.[0]?.ast) {
					// IN (SELECT ...) case
					subqueryAst = rightAsExprList.value[0].ast;
				}

				if (subqueryAst) {
					// This is a subquery - parse it recursively
					const subqueryParsed = parseSubqueryAst(subqueryAst, cteNameSet, validTableNames);

					if (subqueryParsed && operator) {
						const subqueryIndex = subqueries.length;
						subqueries.push({
							id: `subquery-${subqueryIndex}`,
							role: 'where',
							linkedFilterIndex: filters.length,
							innerQuery: subqueryParsed
						});

						filters.push({
							column: fullColumn,
							operator,
							value: '', // Value will come from subquery
							connector,
							subqueryIndex
						});
					}
				} else {
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
	}

	// Handle IS NULL / IS NOT NULL
	if (e.type === 'unary_expr' || (e.type === 'binary_expr' && e.operator === 'IS')) {
		// Handle later if needed
	}
}

/**
 * Parse a subquery AST node into a ParsedQuery.
 * @param ast - The AST node to parse
 * @param cteNameSet - Set of CTE names that are accessible in this scope
 * @param validTableNames - List of valid table names, or null to accept all
 */
function parseSubqueryAst(ast: unknown, cteNameSet?: Set<string>, validTableNames?: string[] | null): ParsedQuery | null {
	const stmt = ast as {
		type?: string;
		columns?: unknown[];
		from?: unknown[];
		where?: unknown;
		groupby?: { columns?: unknown[] };
		having?: unknown;
		orderby?: unknown[];
		limit?: { value?: unknown[] };
	};

	if (!stmt || stmt.type !== 'select') {
		return null;
	}

	// If validTableNames is undefined, use tutorial schema. If null, accept all.
	const effectiveValidTableNames = validTableNames === undefined ? getTableNames() : validTableNames;
	const tables: ParsedTable[] = [];
	const joins: ParsedJoin[] = [];
	const selectAggregates: ParsedSelectAggregate[] = [];
	const columnAggregates: ParsedColumnAggregate[] = [];
	const tableAliasMap = new Map<string, string>();
	const subqueries: ParsedSubquery[] = [];

	// Parse FROM clause (handles regular tables and derived tables/subqueries)
	if (stmt.from && Array.isArray(stmt.from)) {
		for (const fromItem of stmt.from) {
			const fi = fromItem as {
				table?: string;
				as?: string | null;
				join?: string;
				on?: unknown;
				expr?: { ast?: unknown; parentheses?: boolean };
				prefix?: unknown;
			};

			// Check for derived table (FROM (SELECT ...) AS alias)
			if (fi.expr?.ast) {
				const derivedTableParsed = parseSubqueryAst(fi.expr.ast, cteNameSet, effectiveValidTableNames);
				if (derivedTableParsed) {
					const subqueryIndex = subqueries.length;
					subqueries.push({
						id: `subquery-${subqueryIndex}`,
						role: 'from',
						innerQuery: derivedTableParsed
					});
					// Register alias in the alias map for column resolution
					if (fi.as) {
						tableAliasMap.set(fi.as, fi.as);
					}
				}
				continue;
			}

			if (fi.table) {
				const tableName = fi.table;

				// Allow table if it's in schema OR if it's a CTE reference
				// (unless effectiveValidTableNames is null, which means accept all tables)
				if (effectiveValidTableNames !== null && !effectiveValidTableNames.includes(tableName) && !cteNameSet?.has(tableName)) {
					continue;
				}

				const alias = fi.as || null;
				if (alias) {
					tableAliasMap.set(alias, tableName);
				}
				tableAliasMap.set(tableName, tableName);

				if (fi.join) {
					const joinType = normalizeJoinType(fi.join);
					if (fi.on) {
						const joinInfo = parseJoinCondition(fi.on, tableAliasMap);
						if (joinInfo) {
							joins.push({
								...joinInfo,
								joinType,
								targetTable: tableName
							});
						}
					}
				}

				tables.push({
					tableName,
					alias,
					selectedColumns: [],
					isCteReference: cteNameSet?.has(tableName)
				});
			}
		}
	}

	// Parse SELECT columns - handle table.*, basic columns, and aggregates
	if (stmt.columns && Array.isArray(stmt.columns)) {
		for (const col of stmt.columns) {
			if (col === '*') {
				for (const table of tables) {
					const tableSchema = getTable(table.tableName);
					if (tableSchema) {
						table.selectedColumns = tableSchema.columns.map((c) => c.name);
					}
				}
			} else {
				const colObj = col as {
					expr?: {
						type?: string;
						table?: string | { type?: string; value?: string };
						column?: string | { expr?: { value?: string } };
						name?: string;
						args?: { expr?: { type?: string; table?: string | { type?: string; value?: string }; column?: string | { expr?: { value?: string } } } };
					};
					as?: string;
				};

				if (colObj.expr?.type === 'column_ref' && colObj.expr.column === '*') {
					if (colObj.expr.table) {
						// table.* syntax
						const tableName = resolveTableName(colObj.expr.table, tableAliasMap);
						const table = tables.find((t) => t.tableName === tableName);
						if (table) {
							const tableSchema = getTable(tableName);
							if (tableSchema) {
								table.selectedColumns = tableSchema.columns.map((c) => c.name);
							}
						}
					} else {
						// Plain SELECT * - select all columns from all tables
						for (const table of tables) {
							const tableSchema = getTable(table.tableName);
							if (tableSchema) {
								table.selectedColumns = tableSchema.columns.map((c) => c.name);
							}
						}
					}
				} else if (colObj.expr?.type === 'column_ref') {
					const columnName = typeof colObj.expr.column === 'string'
						? colObj.expr.column
						: colObj.expr.column?.expr?.value;
					if (columnName && columnName !== '*') {
						const tableName = colObj.expr.table
							? resolveTableName(colObj.expr.table, tableAliasMap)
							: findTableForColumn(tables, columnName)?.tableName;
						const table = tables.find((t) => t.tableName === tableName);
						if (table && !table.selectedColumns.includes(columnName)) {
							table.selectedColumns.push(columnName);
						}
					}
				} else if (colObj.expr?.type === 'aggr_func' && colObj.expr.name) {
					// Handle aggregate functions (COUNT, SUM, AVG, MIN, MAX)
					const funcName = colObj.expr.name.toUpperCase();
					if (isAggregateFunction(funcName)) {
						const alias = colObj.as || undefined;
						const args = colObj.expr.args;

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
								const columnName = typeof args.expr.column === 'string'
									? args.expr.column
									: args.expr.column?.expr?.value;
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
								// Expression aggregate - standalone
								selectAggregates.push({
									function: funcName as AggregateFunction,
									expression: '*',
									alias
								});
							}
						}
					}
				}
			}
		}
	}

	// Parse WHERE clause recursively (to support nested subqueries)
	const filters = stmt.where ? parseWhereClause(stmt.where, tableAliasMap, tables, subqueries, cteNameSet, effectiveValidTableNames) : [];

	// Parse GROUP BY (simplified)
	const groupBy: ParsedGroupBy[] = [];
	if (stmt.groupby?.columns && Array.isArray(stmt.groupby.columns)) {
		for (const group of stmt.groupby.columns) {
			const g = group as { type?: string; table?: string; column?: string | { expr?: { value?: string } } };
			if (g.type === 'column_ref') {
				const columnName = typeof g.column === 'string' ? g.column : g.column?.expr?.value;
				if (columnName) {
					const tableName = g.table ? resolveTableName(g.table, tableAliasMap) : findTableForColumn(tables, columnName)?.tableName;
					const fullColumn = tableName ? `${tableName}.${columnName}` : columnName;
					groupBy.push({ column: fullColumn });
				}
			}
		}
	}

	// Parse HAVING (also extracts subqueries from HAVING comparisons)
	const having: ParsedHaving[] = [];
	if (stmt.having) {
		parseHavingClause(stmt.having, having, 'AND', subqueries, cteNameSet, effectiveValidTableNames);
	}

	// Parse ORDER BY (simplified)
	const orderBy: ParsedOrderBy[] = [];
	if (stmt.orderby && Array.isArray(stmt.orderby)) {
		for (const order of stmt.orderby) {
			const o = order as { expr?: { type?: string; table?: string; column?: string | { expr?: { value?: string } } }; type?: string };
			if (o.expr?.type === 'column_ref') {
				const columnName = typeof o.expr.column === 'string' ? o.expr.column : o.expr.column?.expr?.value;
				if (columnName) {
					const tableName = o.expr.table ? resolveTableName(o.expr.table, tableAliasMap) : findTableForColumn(tables, columnName)?.tableName;
					const fullColumn = tableName ? `${tableName}.${columnName}` : columnName;
					orderBy.push({
						column: fullColumn,
						direction: (o.type?.toUpperCase() as 'ASC' | 'DESC') || 'ASC'
					});
				}
			}
		}
	}

	// Parse LIMIT
	let limit: number | null = null;
	if (stmt.limit?.value && Array.isArray(stmt.limit.value) && stmt.limit.value.length > 0) {
		const limitVal = stmt.limit.value[0] as { type?: string; value?: number };
		if (limitVal.type === 'number' && typeof limitVal.value === 'number') {
			limit = limitVal.value;
		}
	}

	return { tables, joins, filters, groupBy, having, orderBy, limit, selectAggregates, columnAggregates, subqueries };
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
		'NOT IN': 'NOT IN',
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
 * Also handles subqueries in HAVING comparisons (e.g., SUM(stock) < (SELECT AVG(...)))
 */
function parseHavingClause(
	expr: unknown,
	having: ParsedHaving[],
	connector: 'AND' | 'OR',
	subqueries?: ParsedSubquery[],
	cteNameSet?: Set<string>,
	validTableNames?: string[] | null
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
		parseHavingClause(e.left, having, connector, subqueries, cteNameSet, validTableNames);
		parseHavingClause(e.right, having, e.operator as 'AND' | 'OR', subqueries, cteNameSet, validTableNames);
		return;
	}

	// Handle comparison with aggregate function
	if (e.type === 'binary_expr') {
		const left = e.left as {
			type?: string;
			name?: string;
			args?: { expr?: { type?: string; column?: string | { expr?: { value?: string } } } };
		};
		const right = e.right as { type?: string; value?: unknown; ast?: unknown };

		// Check if left side is an aggregate function
		if (left?.type === 'aggr_func' && left.name) {
			const funcName = left.name.toUpperCase();
			if (isAggregateFunction(funcName)) {
				const operator = mapHavingOperator(e.operator);

				// Check if right side is a subquery
				const rightAsSubquery = right as { ast?: unknown; parentheses?: boolean };
				if (rightAsSubquery?.ast && subqueries) {
					// HAVING with subquery (e.g., SUM(stock) < (SELECT AVG(...)))
					const subqueryParsed = parseSubqueryAst(rightAsSubquery.ast, cteNameSet, validTableNames);
					if (subqueryParsed && operator) {
						// For now, we don't have a way to represent HAVING subqueries in the filter panel
						// But we can still parse the subquery so it shows on canvas
						const subqueryIndex = subqueries.length;
						subqueries.push({
							id: `subquery-${subqueryIndex}`,
							role: 'where', // Subqueries in HAVING also act like WHERE subqueries
							innerQuery: subqueryParsed
						});

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

						// Add the HAVING condition with a placeholder value
						// The subquery relationship is tracked separately
						having.push({
							aggregateFunction: funcName as AggregateFunction,
							column,
							operator,
							value: '(subquery)',
							connector
						});
					}
					return;
				}

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
