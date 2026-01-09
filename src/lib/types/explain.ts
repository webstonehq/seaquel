/**
 * EXPLAIN/ANALYZE query plan types.
 * @module types/explain
 */

/**
 * A node in the query execution plan tree.
 * Represents a single operation in the database's query plan.
 */
export interface ExplainPlanNode {
	/** Unique identifier for this node */
	id: string;
	/** Type of operation (e.g., 'Seq Scan', 'Index Scan', 'Hash Join') */
	nodeType: string;
	/** Table or relation name being accessed */
	relationName?: string;
	/** Alias for the relation in the query */
	alias?: string;
	/** Estimated cost to start returning rows */
	startupCost: number;
	/** Estimated total cost to complete the operation */
	totalCost: number;
	/** Estimated number of rows to be returned */
	planRows: number;
	/** Estimated average width of rows in bytes */
	planWidth: number;

	// ANALYZE fields (actual execution statistics)
	/** Actual time to start returning rows (ms) */
	actualStartupTime?: number;
	/** Actual total execution time (ms) */
	actualTotalTime?: number;
	/** Actual number of rows returned */
	actualRows?: number;
	/** Number of times this node was executed */
	actualLoops?: number;

	// Conditions and additional info
	/** Filter condition applied to rows */
	filter?: string;
	/** Name of the index being used */
	indexName?: string;
	/** Index condition for index scans */
	indexCond?: string;
	/** Type of join (for join nodes) */
	joinType?: string;
	/** Hash condition for hash joins */
	hashCond?: string;
	/** Sort keys for sort operations */
	sortKey?: string[];

	/** Child nodes in the plan tree */
	children: ExplainPlanNode[];
}

/**
 * Complete result of an EXPLAIN or EXPLAIN ANALYZE query.
 */
export interface ExplainResult {
	/** Root node of the execution plan tree */
	plan: ExplainPlanNode;
	/** Time spent planning the query (ms) */
	planningTime: number;
	/** Time spent executing the query (ms) - only for ANALYZE */
	executionTime?: number;
	/** Whether this was an EXPLAIN ANALYZE (vs plain EXPLAIN) */
	isAnalyze: boolean;
}

/**
 * Represents an open EXPLAIN plan viewer tab.
 */
export interface ExplainTab {
	/** Unique tab identifier */
	id: string;
	/** Tab display name */
	name: string;
	/** The original query that was explained */
	sourceQuery: string;
	/** The explain result, if available */
	result?: ExplainResult;
	/** Whether the explain is currently running */
	isExecuting: boolean;
}
