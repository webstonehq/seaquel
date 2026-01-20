/**
 * Query Visualizer types.
 * @module types/visualize
 */

/**
 * Represents an open query visualizer tab.
 */
export interface VisualizeTab {
	/** Unique tab identifier */
	id: string;
	/** Tab display name */
	name: string;
	/** The original SQL query being visualized */
	sourceQuery: string;
	/** Parsed query structure for visualization */
	parsedQuery: ParsedQueryVisual | null;
	/** Error message if parsing failed */
	parseError?: string;
}

/**
 * Complete parsed query structure for visualization.
 */
export interface ParsedQueryVisual {
	/** Type of query (SELECT, INSERT, UPDATE, DELETE) */
	type: 'select' | 'insert' | 'update' | 'delete' | 'other';
	/** Tables and subqueries in FROM clause */
	sources: QuerySource[];
	/** JOIN clauses */
	joins: QueryJoin[];
	/** WHERE conditions */
	filters: QueryFilter[];
	/** GROUP BY columns */
	groupBy: string[] | null;
	/** HAVING clause filter */
	having: QueryFilter | null;
	/** SELECT columns/expressions */
	projections: QueryProjection[];
	/** ORDER BY clauses */
	orderBy: QueryOrderBy[];
	/** LIMIT/OFFSET */
	limit: { count: number; offset?: number } | null;
	/** DISTINCT flag */
	distinct: boolean;
}

/**
 * A table or subquery source in FROM clause.
 */
export interface QuerySource {
	/** Type of source */
	type: 'table' | 'subquery';
	/** Schema name (if specified) */
	schema?: string;
	/** Table name or subquery alias */
	name: string;
	/** Alias for the source */
	alias?: string;
	/** For subqueries, the nested parsed query */
	subquery?: ParsedQueryVisual;
}

/**
 * A JOIN clause.
 */
export interface QueryJoin {
	/** Type of join */
	type: 'INNER' | 'LEFT' | 'RIGHT' | 'FULL' | 'CROSS';
	/** The joined table/source */
	source: QuerySource;
	/** ON condition as readable string */
	condition: string;
}

/**
 * A filter condition (WHERE or HAVING).
 */
export interface QueryFilter {
	/** The full condition as readable string */
	expression: string;
	/** Operator used (AND, OR, etc) at top level */
	operator?: string;
	/** For compound conditions, nested filters */
	children?: QueryFilter[];
}

/**
 * A SELECT column/expression.
 */
export interface QueryProjection {
	/** The expression or column name */
	expression: string;
	/** Alias if specified */
	alias?: string;
	/** Whether this is an aggregate function */
	isAggregate: boolean;
	/** Aggregate function name if applicable */
	aggregateFunction?: string;
}

/**
 * An ORDER BY clause.
 */
export interface QueryOrderBy {
	/** Column or expression */
	expression: string;
	/** Sort direction */
	direction: 'ASC' | 'DESC';
}
