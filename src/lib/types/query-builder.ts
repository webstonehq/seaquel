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
	| 'IN'
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
	/** Value to compare against */
	value: string;
	/** Logical connector to the next condition */
	connector: 'AND' | 'OR';
}

/**
 * Sort direction for ORDER BY clauses.
 */
export type SortDirection = 'ASC' | 'DESC';

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
	/** ORDER BY clauses */
	orderBy: SortCondition[];
	/** LIMIT value, or null for no limit */
	limit: number | null;
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
