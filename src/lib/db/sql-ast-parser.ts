/**
 * SQL AST Parser wrapper for query visualization.
 * Uses node-sql-parser to parse SQL and extract structured information.
 */
import { Parser } from 'node-sql-parser';
import type {
	ParsedQueryVisual,
	QuerySource,
	QueryJoin,
	QueryFilter,
	QueryProjection,
	QueryOrderBy
} from '$lib/types';
import type { DatabaseType } from '$lib/types';

// Parser instance
const parser = new Parser();

// Map database types to parser dialects
const DIALECT_MAP: Record<DatabaseType, string> = {
	postgres: 'PostgreSQL',
	mysql: 'MySQL',
	mariadb: 'MariaDB',
	sqlite: 'SQLite',
	mssql: 'TransactSQL',
	duckdb: 'PostgreSQL' // DuckDB is mostly PostgreSQL compatible
};

/**
 * Aggregate function names to detect in expressions.
 */
const AGGREGATE_FUNCTIONS = new Set([
	'count',
	'sum',
	'avg',
	'min',
	'max',
	'array_agg',
	'string_agg',
	'group_concat',
	'json_agg',
	'jsonb_agg',
	'bool_and',
	'bool_or',
	'every',
	'stddev',
	'variance',
	'first',
	'last',
	'median',
	'mode',
	'percentile_cont',
	'percentile_disc'
]);

/**
 * Parse a SQL query and extract structured information for visualization.
 */
export function parseQueryForVisualization(
	sql: string,
	dbType: DatabaseType = 'postgres'
): ParsedQueryVisual | null {
	try {
		const dialect = DIALECT_MAP[dbType] || 'PostgreSQL';
		const ast = parser.astify(sql, { database: dialect });

		// Handle multiple statements - take the first one
		const statement = Array.isArray(ast) ? ast[0] : ast;

		if (!statement) {
			return null;
		}

		return convertAstToVisual(statement);
	} catch (error) {
		console.error('SQL parse error:', error);
		return null;
	}
}

/**
 * Convert parser AST to our visualization structure.
 */
function convertAstToVisual(ast: any): ParsedQueryVisual | null {
	// Default structure
	const result: ParsedQueryVisual = {
		type: 'other',
		sources: [],
		joins: [],
		filters: [],
		groupBy: null,
		having: null,
		projections: [],
		orderBy: [],
		limit: null,
		distinct: false
	};

	// Determine query type
	if (ast.type === 'select') {
		result.type = 'select';
		result.distinct = ast.distinct === 'DISTINCT';

		// Extract sources from FROM clause
		if (ast.from) {
			result.sources = extractSources(ast.from);
		}

		// Extract JOINs
		if (ast.from) {
			result.joins = extractJoins(ast.from);
		}

		// Extract WHERE clause
		if (ast.where) {
			result.filters = [extractFilter(ast.where)];
		}

		// Extract GROUP BY
		if (ast.groupby) {
			// groupby can be an array or an object with columns property
			const groupbyList = Array.isArray(ast.groupby)
				? ast.groupby
				: ast.groupby.columns || ast.groupby.expr || [ast.groupby];
			result.groupBy = Array.isArray(groupbyList)
				? groupbyList.map((g: any) => expressionToString(g.expr || g))
				: [expressionToString(groupbyList)];
		}

		// Extract HAVING
		if (ast.having) {
			result.having = extractFilter(ast.having);
		}

		// Extract SELECT columns
		if (ast.columns) {
			result.projections = extractProjections(ast.columns);
		}

		// Extract ORDER BY
		if (ast.orderby) {
			result.orderBy = ast.orderby.map((o: any) => ({
				expression: expressionToString(o.expr),
				direction: (o.type?.toUpperCase() || 'ASC') as 'ASC' | 'DESC'
			}));
		}

		// Extract LIMIT
		if (ast.limit) {
			const limitVal = ast.limit.value?.[0]?.value ?? ast.limit.value ?? null;
			const offsetVal = ast.limit.value?.[1]?.value ?? null;
			if (limitVal !== null) {
				result.limit = {
					count: typeof limitVal === 'number' ? limitVal : parseInt(limitVal, 10),
					offset: offsetVal !== null ? (typeof offsetVal === 'number' ? offsetVal : parseInt(offsetVal, 10)) : undefined
				};
			}
		}
	} else if (ast.type === 'insert') {
		result.type = 'insert';
		if (ast.table) {
			result.sources = [
				{
					type: 'table',
					schema: ast.table[0]?.db,
					name: ast.table[0]?.table || 'unknown',
					alias: ast.table[0]?.as
				}
			];
		}
	} else if (ast.type === 'update') {
		result.type = 'update';
		if (ast.table) {
			const tables = Array.isArray(ast.table) ? ast.table : [ast.table];
			result.sources = tables.map((t: any) => ({
				type: 'table' as const,
				schema: t.db,
				name: t.table || 'unknown',
				alias: t.as
			}));
		}
		if (ast.where) {
			result.filters = [extractFilter(ast.where)];
		}
	} else if (ast.type === 'delete') {
		result.type = 'delete';
		if (ast.from) {
			result.sources = extractSources(ast.from);
		}
		if (ast.where) {
			result.filters = [extractFilter(ast.where)];
		}
	}

	return result;
}

/**
 * Extract table sources from FROM clause.
 */
function extractSources(fromClause: any[]): QuerySource[] {
	const sources: QuerySource[] = [];

	for (const item of fromClause) {
		if (item.type === 'dual') {
			// Skip DUAL pseudo-table
			continue;
		}

		// Check if this is a table reference
		if (item.table) {
			sources.push({
				type: 'table',
				schema: item.db || undefined,
				name: item.table,
				alias: item.as || undefined
			});
		}
		// Check for subquery
		else if (item.expr && item.expr.ast) {
			const subquery = convertAstToVisual(item.expr.ast);
			sources.push({
				type: 'subquery',
				name: item.as || 'subquery',
				alias: item.as || undefined,
				subquery: subquery || undefined
			});
		}
	}

	return sources;
}

/**
 * Extract JOIN clauses from FROM clause.
 */
function extractJoins(fromClause: any[]): QueryJoin[] {
	const joins: QueryJoin[] = [];

	for (const item of fromClause) {
		if (item.join) {
			const joinType = normalizeJoinType(item.join);
			const source: QuerySource = item.table
				? {
						type: 'table',
						schema: item.db || undefined,
						name: item.table,
						alias: item.as || undefined
					}
				: item.expr?.ast
					? {
							type: 'subquery',
							name: item.as || 'subquery',
							alias: item.as || undefined,
							subquery: convertAstToVisual(item.expr.ast) || undefined
						}
					: {
							type: 'table',
							name: 'unknown'
						};

			const condition = item.on ? expressionToString(item.on) : '';

			joins.push({
				type: joinType,
				source,
				condition
			});
		}
	}

	return joins;
}

/**
 * Normalize join type string to our enum.
 */
function normalizeJoinType(joinStr: string): 'INNER' | 'LEFT' | 'RIGHT' | 'FULL' | 'CROSS' {
	const upper = joinStr.toUpperCase();
	if (upper.includes('LEFT')) return 'LEFT';
	if (upper.includes('RIGHT')) return 'RIGHT';
	if (upper.includes('FULL')) return 'FULL';
	if (upper.includes('CROSS')) return 'CROSS';
	return 'INNER';
}

/**
 * Extract filter from WHERE/HAVING clause.
 */
function extractFilter(whereClause: any): QueryFilter {
	if (!whereClause) {
		return { expression: '' };
	}

	// Handle binary expressions (AND, OR, etc.)
	if (whereClause.type === 'binary_expr') {
		const operator = whereClause.operator?.toUpperCase();

		// If it's AND/OR, create compound filter
		if (operator === 'AND' || operator === 'OR') {
			return {
				expression: expressionToString(whereClause),
				operator,
				children: [extractFilter(whereClause.left), extractFilter(whereClause.right)]
			};
		}
	}

	// Simple expression
	return {
		expression: expressionToString(whereClause)
	};
}

/**
 * Extract projections (SELECT columns).
 */
function extractProjections(columns: any[] | '*'): QueryProjection[] {
	if (columns === '*') {
		return [{ expression: '*', isAggregate: false }];
	}

	return columns.map((col: any) => {
		const expr = col.expr;
		const alias = col.as || undefined;
		const exprStr = expressionToString(expr);

		// Check for aggregate function
		const { isAggregate, aggregateFunction } = detectAggregate(expr);

		return {
			expression: exprStr,
			alias,
			isAggregate,
			aggregateFunction
		};
	});
}

/**
 * Detect if an expression contains an aggregate function.
 */
function detectAggregate(expr: any): { isAggregate: boolean; aggregateFunction?: string } {
	if (!expr) {
		return { isAggregate: false };
	}

	// Check if this is a function call
	if (expr.type === 'aggr_func' || expr.type === 'function') {
		const funcName = (expr.name || '').toLowerCase();
		if (AGGREGATE_FUNCTIONS.has(funcName)) {
			return { isAggregate: true, aggregateFunction: funcName.toUpperCase() };
		}
	}

	// Recursively check nested expressions
	if (expr.args) {
		const args = Array.isArray(expr.args) ? expr.args : [expr.args];
		for (const arg of args) {
			const result = detectAggregate(arg.expr || arg);
			if (result.isAggregate) {
				return result;
			}
		}
	}

	if (expr.left) {
		const result = detectAggregate(expr.left);
		if (result.isAggregate) return result;
	}

	if (expr.right) {
		const result = detectAggregate(expr.right);
		if (result.isAggregate) return result;
	}

	return { isAggregate: false };
}

/**
 * Convert an AST expression node to a readable string.
 */
function expressionToString(expr: any): string {
	if (!expr) return '';

	// String, number, or null literal
	if (expr.type === 'string' || expr.type === 'single_quote_string') {
		return `'${expr.value}'`;
	}
	if (expr.type === 'number') {
		return String(expr.value);
	}
	if (expr.type === 'null') {
		return 'NULL';
	}
	if (expr.type === 'bool') {
		return expr.value ? 'TRUE' : 'FALSE';
	}

	// Column reference
	if (expr.type === 'column_ref') {
		const parts: string[] = [];
		if (expr.table) parts.push(expr.table);
		parts.push(expr.column);
		return parts.join('.');
	}

	// Star/wildcard
	if (expr.type === 'star') {
		return expr.table ? `${expr.table}.*` : '*';
	}

	// Binary expression
	if (expr.type === 'binary_expr') {
		const left = expressionToString(expr.left);
		const right = expressionToString(expr.right);
		const op = expr.operator || '=';
		return `${left} ${op} ${right}`;
	}

	// Unary expression
	if (expr.type === 'unary_expr') {
		return `${expr.operator || ''} ${expressionToString(expr.expr)}`.trim();
	}

	// Function call
	if (expr.type === 'function' || expr.type === 'aggr_func') {
		const funcName = expr.name || 'FUNC';
		const args = expr.args;

		if (args) {
			if (args.type === 'star') {
				return `${funcName}(*)`;
			}
			if (args.distinct) {
				const argStr = Array.isArray(args.expr)
					? args.expr.map((a: any) => expressionToString(a)).join(', ')
					: expressionToString(args.expr);
				return `${funcName}(DISTINCT ${argStr})`;
			}
			const argStr = Array.isArray(args.expr)
				? args.expr.map((a: any) => expressionToString(a)).join(', ')
				: expressionToString(args.expr);
			return `${funcName}(${argStr})`;
		}
		return `${funcName}()`;
	}

	// CASE expression
	if (expr.type === 'case') {
		let result = 'CASE';
		if (expr.expr) {
			result += ` ${expressionToString(expr.expr)}`;
		}
		if (expr.args) {
			for (const arg of expr.args) {
				if (arg.type === 'when') {
					result += ` WHEN ${expressionToString(arg.cond)} THEN ${expressionToString(arg.result)}`;
				} else if (arg.type === 'else') {
					result += ` ELSE ${expressionToString(arg.result)}`;
				}
			}
		}
		result += ' END';
		return result;
	}

	// IN expression
	if (expr.type === 'expr_list') {
		const values = expr.value?.map((v: any) => expressionToString(v)).join(', ') || '';
		return `(${values})`;
	}

	// BETWEEN expression
	if (expr.type === 'between') {
		return `${expressionToString(expr.expr)} BETWEEN ${expressionToString(expr.start)} AND ${expressionToString(expr.end)}`;
	}

	// LIKE expression
	if (expr.type === 'like') {
		const notStr = expr.not ? 'NOT ' : '';
		return `${expressionToString(expr.expr)} ${notStr}LIKE ${expressionToString(expr.right)}`;
	}

	// Cast expression
	if (expr.type === 'cast') {
		return `CAST(${expressionToString(expr.expr)} AS ${expr.target?.dataType || 'unknown'})`;
	}

	// Subquery
	if (expr.ast) {
		return '(subquery)';
	}

	// Parameter placeholder
	if (expr.type === 'param') {
		return expr.value ? `$${expr.value}` : '?';
	}

	// Default: try to use value directly
	if (typeof expr.value !== 'undefined') {
		return String(expr.value);
	}

	return '';
}

/**
 * Try to get a more user-friendly error message for parse failures.
 */
export function getParseError(sql: string, dbType: DatabaseType = 'postgres'): string | null {
	try {
		const dialect = DIALECT_MAP[dbType] || 'PostgreSQL';
		parser.astify(sql, { database: dialect });
		return null;
	} catch (error: any) {
		// Extract meaningful error message
		if (error.message) {
			// Clean up error message
			const msg = error.message
				.replace(/Syntax error at line \d+ col \d+:/, 'Syntax error:')
				.replace(/\n\n[\s\S]*$/, '')
				.trim();
			return msg || 'Unable to parse SQL query';
		}
		return 'Unable to parse SQL query';
	}
}
