/**
 * Query builder types for the interactive SELECT tutorial.
 * Used to represent canvas state, query configuration, and tutorial challenges.
 * @module types/query-builder
 */

/**
 * Column definition from the sample database schema.
 * Represents a single column within a tutorial table.
 */
export interface TutorialColumn {
	/** Column name */
	name: string;
	/** Data type (e.g., 'integer', 'varchar', 'timestamp') */
	type: string;
	/** Whether this column is the primary key */
	primaryKey?: boolean;
	/** Foreign key reference, if this column references another table */
	foreignKey?: { table: string; column: string };
}

/**
 * Table definition from the sample database schema.
 * Represents a table available for use in the tutorial.
 */
export interface TutorialTable {
	/** Table name */
	name: string;
	/** Column definitions for this table */
	columns: TutorialColumn[];
}

/**
 * A table placed on the canvas.
 * Represents an instance of a table that the user has dragged onto the query builder canvas.
 */
export interface CanvasTable {
	/** Unique identifier for this canvas table instance */
	id: string;
	/** Name of the table from the schema */
	tableName: string;
	/** Position on the canvas */
	position: { x: number; y: number };
	/** Set of column names currently selected for the query */
	selectedColumns: Set<string>;
	/** Map of column name to aggregate function applied to it */
	columnAggregates: Map<string, ColumnAggregate>;
	/** If set, this table references a CTE instead of schema table */
	cteId?: string;
}

/**
 * Join type options for SQL JOIN clauses.
 */
export type JoinType = 'INNER' | 'LEFT' | 'RIGHT' | 'FULL';

/**
 * A join between two tables on the canvas.
 * Represents a connection drawn between tables to create a JOIN clause.
 */
export interface CanvasJoin {
	/** Unique identifier for this join */
	id: string;
	/** Name of the source table */
	sourceTable: string;
	/** Name of the column in the source table */
	sourceColumn: string;
	/** Name of the target table */
	targetTable: string;
	/** Name of the column in the target table */
	targetColumn: string;
	/** Type of join to perform */
	joinType: JoinType;
}

/**
 * Comparison operators for WHERE clauses.
 * Includes standard SQL comparison operators and special conditions.
 */
export type FilterOperator =
	| '='
	| '!='
	| '>'
	| '<'
	| '>='
	| '<='
	| 'LIKE'
	| 'NOT LIKE'
	| 'IS NULL'
	| 'IS NOT NULL'
	| 'IS TRUE'
	| 'IS FALSE'
	| 'IS NOT TRUE'
	| 'IS NOT FALSE'
	| 'IN'
	| 'NOT IN'
	| 'BETWEEN';

/**
 * A single WHERE condition.
 * Represents one filter criterion in the query's WHERE clause.
 */
export interface FilterCondition {
	/** Unique identifier for this filter */
	id: string;
	/** Column to filter on, in format "table.column" */
	column: string;
	/** Comparison operator */
	operator: FilterOperator;
	/** Value to compare against (ignored if subqueryId is set) */
	value: string;
	/** Logical connector to the next condition */
	connector: 'AND' | 'OR';
	/** Optional linked subquery ID for WHERE subqueries */
	subqueryId?: string;
}

/**
 * Sort direction for ORDER BY clauses.
 */
export type SortDirection = 'ASC' | 'DESC';

/**
 * Aggregate functions for HAVING clauses.
 */
export type AggregateFunction = 'COUNT' | 'SUM' | 'AVG' | 'MIN' | 'MAX';

/**
 * A standalone aggregate in the SELECT clause.
 * Used for COUNT(*), expressions, or aggregates not tied to a specific column.
 */
export interface SelectAggregate {
	/** Unique identifier for this aggregate */
	id: string;
	/** Aggregate function (COUNT, SUM, AVG, MIN, MAX) */
	function: AggregateFunction;
	/** Expression inside the aggregate (*, column name, or expression like "price * quantity") */
	expression: string;
	/** Optional alias for AS clause */
	alias?: string;
}

/**
 * Per-column aggregate applied to a selected column.
 */
export interface ColumnAggregate {
	/** Aggregate function (COUNT, SUM, AVG, MIN, MAX) */
	function: AggregateFunction;
	/** Optional alias for AS clause */
	alias?: string;
}

/**
 * Unified aggregate for display in the AGGREGATES panel.
 * Can represent either a column aggregate or a standalone select aggregate.
 */
export interface DisplayAggregate {
	/** Unique identifier */
	id: string;
	/** Aggregate function */
	function: AggregateFunction;
	/** Expression (for column aggregate: table.column, for select aggregate: the expression) */
	expression: string;
	/** Optional alias */
	alias?: string;
	/** Source type to determine how to handle edits/removes */
	source: 'column' | 'select';
	/** For column aggregates: the table ID */
	tableId?: string;
	/** For column aggregates: the column name */
	columnName?: string;
}

/**
 * Comparison operators valid for HAVING conditions.
 * Limited to numeric comparisons (no LIKE, IS NULL, etc.).
 */
export type HavingOperator = '=' | '!=' | '>' | '<' | '>=' | '<=';

/**
 * An ORDER BY clause.
 * Represents a single column to sort by in the query results.
 */
export interface SortCondition {
	/** Unique identifier for this sort condition */
	id: string;
	/** Column to sort by, in format "table.column" */
	column: string;
	/** Sort direction */
	direction: SortDirection;
}

/**
 * A GROUP BY column.
 * Represents a column to group results by.
 */
export interface GroupByCondition {
	/** Unique identifier for this group by condition */
	id: string;
	/** Column to group by, in format "table.column" */
	column: string;
}

/**
 * A single HAVING condition.
 * Represents one filter criterion applied after GROUP BY aggregation.
 */
export interface HavingCondition {
	/** Unique identifier for this having condition */
	id: string;
	/** Aggregate function (COUNT, SUM, AVG, MIN, MAX) */
	aggregateFunction: AggregateFunction;
	/** Column for the aggregate, empty string means * for COUNT(*) */
	column: string;
	/** Comparison operator */
	operator: HavingOperator;
	/** Value to compare against */
	value: string;
	/** Logical connector to the next condition */
	connector: 'AND' | 'OR';
}

/**
 * Subquery role - determines where the subquery appears in the outer query.
 */
export type SubqueryRole = 'where' | 'from' | 'select';

/**
 * Inner query state for a subquery.
 * Contains all the query elements that can exist within a subquery.
 */
export interface SubqueryInnerState {
	/** Tables within the subquery */
	tables: CanvasTable[];
	/** Joins between tables within the subquery */
	joins: CanvasJoin[];
	/** WHERE clause conditions for the subquery */
	filters: FilterCondition[];
	/** GROUP BY columns for the subquery */
	groupBy: GroupByCondition[];
	/** HAVING clause conditions for the subquery */
	having: HavingCondition[];
	/** ORDER BY clauses for the subquery */
	orderBy: SortCondition[];
	/** LIMIT value for the subquery */
	limit: number | null;
	/** Standalone aggregates in the subquery's SELECT clause */
	selectAggregates: SelectAggregate[];
	/** Nested subqueries (unlimited depth) */
	subqueries: CanvasSubquery[];
}

/**
 * A subquery placed on the canvas.
 * Represents a nested query that can appear in WHERE, FROM, or SELECT clauses.
 */
export interface CanvasSubquery {
	/** Unique identifier for this subquery */
	id: string;
	/** Position on the canvas */
	position: { x: number; y: number };
	/** Size of the subquery container */
	size: { width: number; height: number };
	/** Role determines where this subquery appears (WHERE, FROM, SELECT) */
	role: SubqueryRole;
	/** For WHERE subqueries: the filter this subquery is linked to */
	linkedFilterId?: string;
	/** For FROM/SELECT subqueries: the alias for referencing */
	alias?: string;
	/** The inner query state */
	innerQuery: SubqueryInnerState;
}

/**
 * A CTE (Common Table Expression) container on the canvas.
 * Defines a named subquery in the WITH clause that can be referenced like a table.
 */
export interface CanvasCTE {
	/** Unique identifier for this CTE */
	id: string;
	/** CTE name (required, used in WITH clause) */
	name: string;
	/** Position on the canvas */
	position: { x: number; y: number };
	/** Size of the CTE container */
	size: { width: number; height: number };
	/** The inner query state defining the CTE */
	innerQuery: SubqueryInnerState;
}

/**
 * Complete query builder state.
 * Captures the full state of the query builder canvas and configuration.
 */
export interface QueryBuilderSnapshot {
	/** Tables placed on the canvas */
	tables: CanvasTable[];
	/** Joins between tables */
	joins: CanvasJoin[];
	/** WHERE clause conditions */
	filters: FilterCondition[];
	/** GROUP BY columns */
	groupBy: GroupByCondition[];
	/** HAVING clause conditions */
	having: HavingCondition[];
	/** ORDER BY clauses */
	orderBy: SortCondition[];
	/** LIMIT value, or null for no limit */
	limit: number | null;
	/** Standalone aggregates in SELECT clause */
	selectAggregates: SelectAggregate[];
	/** Subqueries on the canvas */
	subqueries: CanvasSubquery[];
	/** CTEs (Common Table Expressions) on the canvas */
	ctes: CanvasCTE[];
}

/**
 * Validation criterion for challenges.
 * Defines a single requirement that must be satisfied to complete a challenge.
 */
export interface ChallengeCriterion {
	/** Unique identifier for this criterion */
	id: string;
	/** Human-readable description of what needs to be done */
	description: string;
	/** Function to check if the criterion is satisfied */
	check: (state: QueryBuilderSnapshot, sql: string) => boolean;
	/** Whether this criterion has been satisfied */
	satisfied: boolean;
}

/**
 * A single challenge within a lesson.
 * Represents an interactive task for the user to complete.
 */
export interface Challenge {
	/** Unique identifier for this challenge */
	id: string;
	/** Challenge title */
	title: string;
	/** Description of what the user needs to accomplish */
	description: string;
	/** Optional hint to help the user */
	hint?: string;
	/** Criteria that must be satisfied to complete the challenge */
	criteria: Omit<ChallengeCriterion, 'satisfied'>[];
}

/**
 * A tutorial lesson.
 * Contains multiple challenges that teach a specific SQL concept.
 */
export interface TutorialLesson {
	/** Unique identifier for this lesson */
	id: string;
	/** Lesson title */
	title: string;
	/** Introduction text explaining the lesson concepts */
	introduction: string;
	/** Challenges within this lesson */
	challenges: Challenge[];
}

// ============================================
// Schema Adapter Types (for Manage section)
// ============================================

/**
 * Foreign key reference in unified format.
 */
export interface QueryBuilderForeignKey {
	/** Referenced table name */
	table: string;
	/** Referenced column name */
	column: string;
}

/**
 * Column definition in unified format for the query builder.
 * This interface bridges TutorialColumn and SchemaColumn formats.
 */
export interface QueryBuilderColumn {
	/** Column name */
	name: string;
	/** Data type (e.g., 'integer', 'varchar', 'timestamp') */
	type: string;
	/** Whether this column is the primary key */
	primaryKey: boolean;
	/** Foreign key reference, if this column references another table */
	foreignKey?: QueryBuilderForeignKey;
}

/**
 * Table definition in unified format for the query builder.
 * This interface bridges TutorialTable and SchemaTable formats.
 */
export interface QueryBuilderTable {
	/** Table name */
	name: string;
	/** Column definitions for this table */
	columns: QueryBuilderColumn[];
}
