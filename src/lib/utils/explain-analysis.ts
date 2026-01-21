import type { ExplainResult, ExplainPlanNode } from "$lib/types";

/**
 * Hot path tier classification based on percentage of total execution time.
 */
export type HotPathTier = "critical" | "warning" | "normal";

/**
 * Analysis data for a single node in the execution plan.
 */
export interface NodeAnalysis {
	/** Effective time = actualTotalTime × actualLoops */
	effectiveTime: number;
	/** Percentage of total execution time consumed by this node */
	percentageOfTotal: number;
	/** Classification tier for visual styling */
	tier: HotPathTier;
	/** Ratio of actualRows to planRows (for estimation error detection) */
	rowEstimationRatio: number;
	/** True if row estimation is off by more than 10x */
	hasEstimationError: boolean;
}

/**
 * Information about a performance bottleneck.
 */
export interface BottleneckInfo {
	/** Node ID */
	nodeId: string;
	/** Node type (e.g., 'Seq Scan', 'Hash Join') */
	nodeType: string;
	/** Table or relation name if applicable */
	relationName?: string;
	/** Effective execution time in ms */
	effectiveTime: number;
	/** Percentage of total time */
	percentageOfTotal: number;
	/** Tier classification */
	tier: HotPathTier;
	/** Whether this node has row estimation errors */
	hasEstimationError: boolean;
}

/**
 * Complete hot path analysis for an EXPLAIN ANALYZE result.
 */
export interface HotPathAnalysis {
	/** Total execution time in ms */
	totalTime: number;
	/** Analysis data keyed by node ID */
	nodeAnalysis: Map<string, NodeAnalysis>;
	/** Sorted list of bottlenecks (critical first, then warning) */
	bottlenecks: BottleneckInfo[];
	/** Whether this result has ANALYZE data */
	hasAnalyzeData: boolean;
}

/** Threshold for critical tier (≥40% of total time) */
const CRITICAL_THRESHOLD = 0.4;
/** Threshold for warning tier (≥20% of total time) */
const WARNING_THRESHOLD = 0.2;
/** Row estimation error threshold (10x difference) */
const ROW_ESTIMATION_ERROR_THRESHOLD = 10;

/**
 * Analyzes an EXPLAIN ANALYZE result to identify hot paths and performance bottlenecks.
 *
 * @param result - The explain result to analyze
 * @returns Hot path analysis with node data and bottleneck list
 */
export function analyzeExplainPlan(result: ExplainResult): HotPathAnalysis {
	const nodeAnalysis = new Map<string, NodeAnalysis>();
	const bottlenecks: BottleneckInfo[] = [];

	// Check if we have ANALYZE data
	const hasAnalyzeData = result.isAnalyze && result.plan.actualTotalTime !== undefined;

	if (!hasAnalyzeData) {
		return {
			totalTime: 0,
			nodeAnalysis,
			bottlenecks,
			hasAnalyzeData: false,
		};
	}

	// Calculate total time from root node (includes all child operations)
	const totalTime = calculateEffectiveTime(result.plan);

	// Traverse the tree and analyze each node
	traverseAndAnalyze(result.plan, totalTime, nodeAnalysis, bottlenecks);

	// Sort bottlenecks by percentage (highest first)
	bottlenecks.sort((a, b) => b.percentageOfTotal - a.percentageOfTotal);

	// Filter to only include warning and critical tiers
	const significantBottlenecks = bottlenecks.filter((b) => b.tier !== "normal");

	return {
		totalTime,
		nodeAnalysis,
		bottlenecks: significantBottlenecks,
		hasAnalyzeData: true,
	};
}

/**
 * Calculates the effective execution time for a node.
 * Effective time = actualTotalTime × actualLoops
 */
function calculateEffectiveTime(node: ExplainPlanNode): number {
	const time = node.actualTotalTime ?? 0;
	const loops = node.actualLoops ?? 1;
	return time * loops;
}

/**
 * Calculates the row estimation ratio (actual / planned).
 * Returns 1 if either value is unavailable.
 */
function calculateRowEstimationRatio(node: ExplainPlanNode): number {
	const actualRows = node.actualRows;
	const planRows = node.planRows;

	if (actualRows === undefined || planRows === 0) {
		return 1;
	}

	// Calculate ratio, handling zero actual rows
	if (actualRows === 0 && planRows > 0) {
		return planRows; // Planned many, got none
	}

	if (actualRows > planRows) {
		return actualRows / planRows;
	} else {
		return planRows / actualRows;
	}
}

/**
 * Determines the tier classification based on percentage of total time.
 */
function determineTier(percentage: number): HotPathTier {
	if (percentage >= CRITICAL_THRESHOLD) {
		return "critical";
	}
	if (percentage >= WARNING_THRESHOLD) {
		return "warning";
	}
	return "normal";
}

/**
 * Recursively traverses the plan tree and builds analysis data.
 */
function traverseAndAnalyze(
	node: ExplainPlanNode,
	totalTime: number,
	nodeAnalysis: Map<string, NodeAnalysis>,
	bottlenecks: BottleneckInfo[]
): void {
	const effectiveTime = calculateEffectiveTime(node);
	const percentageOfTotal = totalTime > 0 ? effectiveTime / totalTime : 0;
	const tier = determineTier(percentageOfTotal);
	const rowEstimationRatio = calculateRowEstimationRatio(node);
	const hasEstimationError = rowEstimationRatio >= ROW_ESTIMATION_ERROR_THRESHOLD;

	const analysis: NodeAnalysis = {
		effectiveTime,
		percentageOfTotal,
		tier,
		rowEstimationRatio,
		hasEstimationError,
	};

	nodeAnalysis.set(node.id, analysis);

	// Add to bottlenecks list (we'll filter later)
	bottlenecks.push({
		nodeId: node.id,
		nodeType: node.nodeType,
		relationName: node.relationName,
		effectiveTime,
		percentageOfTotal,
		tier,
		hasEstimationError,
	});

	// Recurse into children
	for (const child of node.children) {
		traverseAndAnalyze(child, totalTime, nodeAnalysis, bottlenecks);
	}
}
