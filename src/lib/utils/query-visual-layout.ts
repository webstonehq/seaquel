/**
 * Layout engine for query visualization.
 * Converts parsed query structure to xyflow nodes and edges.
 */
import dagre from "@dagrejs/dagre";
import { Position } from "@xyflow/svelte";
import type { Node, Edge } from "@xyflow/svelte";
import type { ParsedQueryVisual } from "$lib/types";

const NODE_WIDTH = 200;
const NODE_HEIGHT = 80;

/**
 * Node type definitions for the query visualization.
 */
export type QueryVisualNodeType =
	| 'tableSourceNode'
	| 'joinNode'
	| 'filterNode'
	| 'groupNode'
	| 'projectionNode'
	| 'sortNode'
	| 'limitNode';

/**
 * Layout direction options.
 */
export type LayoutDirection = 'TB' | 'BT' | 'LR' | 'RL';

/**
 * Layout alignment options.
 */
export type LayoutAlign = 'UL' | 'UR' | 'DL' | 'DR';

/**
 * Configurable layout options for the query visualization.
 */
export interface QueryLayoutOptions {
	/** Layout direction: TB (top-bottom), BT (bottom-top), LR (left-right), RL (right-left) */
	direction: LayoutDirection;
	/** Horizontal spacing between nodes */
	nodeSpacing: number;
	/** Vertical spacing between ranks/levels */
	rankSpacing: number;
}

/**
 * Default layout options.
 */
export const DEFAULT_LAYOUT_OPTIONS: QueryLayoutOptions = {
	direction: 'TB',
	nodeSpacing: 60,
	rankSpacing: 80
};

/**
 * Convert a parsed query to xyflow nodes and edges for visualization.
 */
export function layoutQueryVisualization(
	parsedQuery: ParsedQueryVisual,
	options: QueryLayoutOptions = DEFAULT_LAYOUT_OPTIONS
): {
	nodes: Node[];
	edges: Edge[];
} {
	const nodes: Node[] = [];
	const edges: Edge[] = [];
	let nodeIndex = 0;

	const createNodeId = (prefix: string) => `${prefix}-${nodeIndex++}`;

	// Track the previous node to connect edges in the flow
	let previousNodeId: string | null = null;

	const connectToPrevious = (currentNodeId: string, label?: string) => {
		if (previousNodeId) {
			edges.push({
				id: `edge-${previousNodeId}-${currentNodeId}`,
				source: previousNodeId,
				target: currentNodeId,
				type: 'smoothstep',
				animated: false,
				style: 'stroke-width: 2px;',
				...(label && { label, labelStyle: 'font-size: 10px; fill: #737373;' })
			});
		}
		previousNodeId = currentNodeId;
	};

	// 1. Create source nodes (FROM tables)
	// When we have JOINs, only create a source node for the FIRST table
	// The joined tables will be created during JOIN processing
	const sourceNodeIds: string[] = [];
	const sourcesToCreate = parsedQuery.joins.length > 0
		? parsedQuery.sources.slice(0, 1)  // Only first table when JOINs exist
		: parsedQuery.sources;              // All tables when no JOINs

	for (let i = 0; i < sourcesToCreate.length; i++) {
		const source = sourcesToCreate[i];
		const nodeId = createNodeId('source');
		nodes.push({
			id: nodeId,
			type: 'tableSourceNode',
			position: { x: 0, y: 0 },
			data: {
				source,
				isFirst: i === 0 && parsedQuery.joins.length === 0
			}
		});
		sourceNodeIds.push(nodeId);
	}

	// If no joins and single source, set it as the starting point
	if (sourceNodeIds.length === 1 && parsedQuery.joins.length === 0) {
		previousNodeId = sourceNodeIds[0];
	}

	// 2. Create JOIN nodes
	// If we have JOINs, connect first source to first join, and create source nodes for joined tables
	if (parsedQuery.joins.length > 0 && sourceNodeIds.length > 0) {
		// Connect first source table
		previousNodeId = sourceNodeIds[0];

		for (let i = 0; i < parsedQuery.joins.length; i++) {
			const join = parsedQuery.joins[i];

			// Create a source node for the joined table (right side of join)
			const joinedTableNodeId = createNodeId('source');
			nodes.push({
				id: joinedTableNodeId,
				type: 'tableSourceNode',
				position: { x: 0, y: 0 },
				data: {
					source: join.source,
					isFirst: false
				}
			});

			// Create the join node
			const joinNodeId = createNodeId('join');
			nodes.push({
				id: joinNodeId,
				type: 'joinNode',
				position: { x: 0, y: 0 },
				data: { join }
			});

			// Connect previous node (left side) to left input of join
			if (previousNodeId) {
				edges.push({
					id: `edge-${previousNodeId}-${joinNodeId}-left`,
					source: previousNodeId,
					target: joinNodeId,
					targetHandle: 'left',
					type: 'smoothstep',
					animated: false,
					style: 'stroke-width: 2px;'
				});
			}

			// Connect joined table (right side) to right input of join
			edges.push({
				id: `edge-${joinedTableNodeId}-${joinNodeId}-right`,
				source: joinedTableNodeId,
				target: joinNodeId,
				targetHandle: 'right',
				type: 'smoothstep',
				animated: false,
				style: 'stroke-width: 2px;'
			});

			previousNodeId = joinNodeId;
		}
	} else if (sourceNodeIds.length > 1) {
		// Multiple sources without explicit JOINs (CROSS JOIN implied or comma-separated tables)
		// Connect them sequentially
		previousNodeId = sourceNodeIds[0];
		for (let i = 1; i < sourceNodeIds.length; i++) {
			edges.push({
				id: `edge-${sourceNodeIds[i - 1]}-${sourceNodeIds[i]}`,
				source: sourceNodeIds[i - 1],
				target: sourceNodeIds[i],
				type: 'smoothstep',
				animated: false,
				style: 'stroke-width: 2px;'
			});
			previousNodeId = sourceNodeIds[i];
		}
	}

	// 3. Create WHERE filter node
	if (parsedQuery.filters.length > 0) {
		const filterNodeId = createNodeId('filter');
		nodes.push({
			id: filterNodeId,
			type: 'filterNode',
			position: { x: 0, y: 0 },
			data: {
				filter: parsedQuery.filters[0],
				filterType: 'WHERE'
			}
		});
		connectToPrevious(filterNodeId);
	}

	// 4. Create GROUP BY node
	if (parsedQuery.groupBy && parsedQuery.groupBy.length > 0) {
		const groupNodeId = createNodeId('group');
		nodes.push({
			id: groupNodeId,
			type: 'groupNode',
			position: { x: 0, y: 0 },
			data: { columns: parsedQuery.groupBy }
		});
		connectToPrevious(groupNodeId);
	}

	// 5. Create HAVING filter node
	if (parsedQuery.having) {
		const havingNodeId = createNodeId('having');
		nodes.push({
			id: havingNodeId,
			type: 'filterNode',
			position: { x: 0, y: 0 },
			data: {
				filter: parsedQuery.having,
				filterType: 'HAVING'
			}
		});
		connectToPrevious(havingNodeId);
	}

	// 6. Create SELECT/projection node
	if (parsedQuery.projections.length > 0) {
		const projectionNodeId = createNodeId('projection');
		nodes.push({
			id: projectionNodeId,
			type: 'projectionNode',
			position: { x: 0, y: 0 },
			data: {
				projections: parsedQuery.projections,
				distinct: parsedQuery.distinct
			}
		});
		connectToPrevious(projectionNodeId);
	}

	// 7. Create ORDER BY node
	if (parsedQuery.orderBy.length > 0) {
		const sortNodeId = createNodeId('sort');
		nodes.push({
			id: sortNodeId,
			type: 'sortNode',
			position: { x: 0, y: 0 },
			data: { orderBy: parsedQuery.orderBy }
		});
		connectToPrevious(sortNodeId);
	}

	// 8. Create LIMIT node
	if (parsedQuery.limit) {
		const limitNodeId = createNodeId('limit');
		nodes.push({
			id: limitNodeId,
			type: 'limitNode',
			position: { x: 0, y: 0 },
			data: { limit: parsedQuery.limit }
		});
		connectToPrevious(limitNodeId);
	}

	// Apply dagre layout
	const layoutedNodes = applyDagreLayout(nodes, edges, options);

	return { nodes: layoutedNodes, edges };
}

/**
 * Apply dagre layout to position nodes automatically.
 */
function applyDagreLayout(nodes: Node[], edges: Edge[], options: QueryLayoutOptions): Node[] {
	if (nodes.length === 0) return [];

	const dagreGraph = new dagre.graphlib.Graph();
	dagreGraph.setDefaultEdgeLabel(() => ({}));
	dagreGraph.setGraph({
		rankdir: options.direction,
		nodesep: options.nodeSpacing,
		ranksep: options.rankSpacing,
		marginx: 40,
		marginy: 40,
		align: 'UL'
	});

	// Determine source/target positions based on direction
	const isHorizontal = options.direction === 'LR' || options.direction === 'RL';
	const targetPos = isHorizontal
		? (options.direction === 'LR' ? Position.Left : Position.Right)
		: (options.direction === 'TB' ? Position.Top : Position.Bottom);
	const sourcePos = isHorizontal
		? (options.direction === 'LR' ? Position.Right : Position.Left)
		: (options.direction === 'TB' ? Position.Bottom : Position.Top);

	// Add nodes to dagre
	for (const node of nodes) {
		dagreGraph.setNode(node.id, { width: NODE_WIDTH, height: NODE_HEIGHT });
	}

	// Add edges to dagre
	for (const edge of edges) {
		dagreGraph.setEdge(edge.source, edge.target);
	}

	// Calculate layout
	dagre.layout(dagreGraph);

	// Get positions from dagre
	const positionedNodes = nodes.map((node) => {
		const nodeWithPosition = dagreGraph.node(node.id);
		return {
			...node,
			targetPosition: targetPos,
			sourcePosition: sourcePos,
			position: {
				x: nodeWithPosition.x - NODE_WIDTH / 2,
				y: nodeWithPosition.y - NODE_HEIGHT / 2
			}
		};
	});

	// Center the graph horizontally
	// Find the bounding box
	let minX = Infinity;
	let maxX = -Infinity;
	for (const node of positionedNodes) {
		minX = Math.min(minX, node.position.x);
		maxX = Math.max(maxX, node.position.x + NODE_WIDTH);
	}

	// Calculate offset to center around x=0
	const graphWidth = maxX - minX;
	const offsetX = -minX - graphWidth / 2 + NODE_WIDTH / 2;

	// Apply centering offset
	return positionedNodes.map((node) => ({
		...node,
		position: {
			x: node.position.x + offsetX,
			y: node.position.y
		}
	}));
}
