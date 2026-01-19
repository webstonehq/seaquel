import type { Node, Edge, XYPosition, Connection } from "@xyflow/svelte";
import type { NodeChange, EdgeChange } from "@xyflow/system";
import type { DatabaseState } from "./state.svelte.js";
import type { CanvasState } from "./canvas-state.svelte.js";
import type {
	CanvasNodeData,
	CanvasTableNodeData,
	CanvasQueryNodeData,
	CanvasResultNodeData,
	CanvasChartNodeData,
	SavedCanvas,
	CanvasTimelineEntry,
	SerializedCanvasNode,
	SerializedCanvasEdge,
} from "$lib/types/canvas";
import type { SchemaTable, ChartConfig } from "$lib/types";
import { createDefaultChartConfig } from "$lib/components/charts/chart-utils";

const DEFAULT_NODE_WIDTH = 320;
const DEFAULT_TABLE_NODE_HEIGHT = 300;
const DEFAULT_QUERY_NODE_HEIGHT = 200;
const DEFAULT_RESULT_NODE_HEIGHT = 250;

/**
 * Canvas manager - handles all canvas operations
 */
export class CanvasManager {
	constructor(
		private state: DatabaseState,
		private canvasState: CanvasState,
		private schedulePersistence: (projectId: string | null) => void,
		private executeQuery: (query: string) => Promise<Record<string, unknown>[]>
	) {}

	// === NODE MANAGEMENT ===

	/**
	 * Add a table node to the canvas
	 */
	addTableNode(
		table: SchemaTable,
		position?: XYPosition
	): string {
		const id = crypto.randomUUID();
		const connectionId = this.state.activeConnectionId;
		if (!connectionId) {
			throw new Error("No active connection");
		}

		const nodePosition = position ?? this.getNextNodePosition();

		const data: CanvasTableNodeData = {
			type: "table",
			tableName: table.name,
			schemaName: table.schema,
			connectionId,
			tableType: table.type,
			rowCount: table.rowCount,
			columns: table.columns.map((c) => ({
				name: c.name,
				type: c.type,
				nullable: c.nullable,
				defaultValue: c.defaultValue,
				isPrimaryKey: c.isPrimaryKey,
				isForeignKey: c.isForeignKey,
				foreignKeyRef: c.foreignKeyRef,
			})),
			indexes: table.indexes.map((idx) => ({
				name: idx.name,
				columns: idx.columns,
				unique: idx.unique,
				type: idx.type,
			})),
		};

		const node: Node<CanvasTableNodeData> = {
			id,
			type: "tableNode",
			position: nodePosition,
			data,
			width: 280,
			height: 300,
		};

		this.canvasState.nodes = [...this.canvasState.nodes, node];

		// Add timeline entry
		this.addTimelineEntry({
			type: "table-open",
			description: `Opened ${table.schema}.${table.name}`,
			nodeId: id,
		});

		return id;
	}

	/**
	 * Add a query node to the canvas
	 */
	addQueryNode(query?: string, position?: XYPosition): string {
		const id = crypto.randomUUID();
		const connectionId = this.state.activeConnectionId;
		if (!connectionId) {
			throw new Error("No active connection");
		}

		const nodePosition = position ?? this.getNextNodePosition();

		const data: CanvasQueryNodeData = {
			type: "query",
			name: "Query",
			query: query ?? "",
			connectionId,
			isExecuting: false,
		};

		const node: Node<CanvasQueryNodeData> = {
			id,
			type: "queryNode",
			position: nodePosition,
			data,
			width: 300,
			height: 150,
		};

		this.canvasState.nodes = [...this.canvasState.nodes, node];

		return id;
	}

	/**
	 * Add a result node to the canvas
	 */
	addResultNode(
		queryNodeId: string,
		columns: string[],
		rows: Record<string, unknown>[],
		totalRows: number,
		executionTime?: number,
		position?: XYPosition
	): string {
		const id = crypto.randomUUID();

		// Position to the right of the query node
		const queryNode = this.canvasState.getNode(queryNodeId);
		const nodePosition = position ?? {
			x: (queryNode?.position.x ?? 0) + DEFAULT_NODE_WIDTH + 50,
			y: queryNode?.position.y ?? 0,
		};

		const data: CanvasResultNodeData = {
			type: "result",
			sourceQueryNodeId: queryNodeId,
			columns,
			rows,
			totalRows,
			executionTime,
		};

		const node: Node<CanvasResultNodeData> = {
			id,
			type: "resultNode",
			position: nodePosition,
			data,
			width: 400,
			height: 350,
		};

		this.canvasState.nodes = [...this.canvasState.nodes, node];

		// Auto-connect query to result
		this.connect(queryNodeId, id, "output", "input");

		return id;
	}

	/**
	 * Add a chart node to the canvas
	 */
	addChartNode(
		sourceNodeId: string,
		columns: string[],
		rows: Record<string, unknown>[],
		chartConfig?: ChartConfig,
		position?: XYPosition
	): string {
		const id = crypto.randomUUID();

		// Position to the right of the source node
		const sourceNode = this.canvasState.getNode(sourceNodeId);
		const nodePosition = position ?? {
			x: (sourceNode?.position.x ?? 0) + DEFAULT_NODE_WIDTH + 50,
			y: sourceNode?.position.y ?? 0,
		};

		const config = chartConfig ?? createDefaultChartConfig(columns, rows);

		const data: CanvasChartNodeData = {
			type: "chart",
			sourceNodeId,
			columns,
			rows,
			chartConfig: config,
		};

		const node: Node<CanvasChartNodeData> = {
			id,
			type: "chartNode",
			position: nodePosition,
			data,
			width: 450,
			height: 350,
		};

		this.canvasState.nodes = [...this.canvasState.nodes, node];

		// Auto-connect source to chart
		this.connect(sourceNodeId, id, "output", "input");

		return id;
	}

	/**
	 * Remove a node and its connected edges
	 */
	removeNode(nodeId: string): void {
		// Remove connected edges first
		const connectedEdges = this.canvasState.getConnectedEdges(nodeId);
		for (const edge of connectedEdges) {
			this.disconnect(edge.id);
		}

		// Remove the node
		this.canvasState.nodes = this.canvasState.nodes.filter((n) => n.id !== nodeId);
	}

	/**
	 * Update node data
	 */
	updateNodeData<T extends CanvasNodeData>(
		nodeId: string,
		updates: Partial<T>
	): void {
		this.canvasState.nodes = this.canvasState.nodes.map((node) => {
			if (node.id === nodeId) {
				return {
					...node,
					data: { ...node.data, ...updates } as CanvasNodeData,
				};
			}
			return node;
		});
	}

	/**
	 * Update node dimensions after resize
	 */
	updateNodeDimensions(nodeId: string, width: number, height: number): void {
		this.canvasState.nodes = this.canvasState.nodes.map((node) => {
			if (node.id === nodeId) {
				return {
					...node,
					width,
					height,
				};
			}
			return node;
		});
	}

	// === EDGE MANAGEMENT ===

	/**
	 * Connect two nodes
	 */
	connect(
		sourceId: string,
		targetId: string,
		sourceHandle?: string,
		targetHandle?: string
	): string {
		const id = `${sourceId}-${targetId}`;

		const edge: Edge = {
			id,
			source: sourceId,
			target: targetId,
			sourceHandle: sourceHandle ?? null,
			targetHandle: targetHandle ?? null,
		};

		this.canvasState.edges = [...this.canvasState.edges, edge];

		return id;
	}

	/**
	 * Disconnect an edge
	 */
	disconnect(edgeId: string): void {
		this.canvasState.edges = this.canvasState.edges.filter((e) => e.id !== edgeId);
	}

	// === XYFLOW CALLBACKS ===

	/**
	 * Handle node changes from xyflow
	 */
	onNodesChange = (changes: NodeChange[]): void => {
		let nodes = [...this.canvasState.nodes];

		for (const change of changes) {
			switch (change.type) {
				case "position":
					if (change.position) {
						nodes = nodes.map((n) =>
							n.id === change.id ? { ...n, position: change.position! } : n
						);
					}
					break;
				case "dimensions":
					if (change.dimensions) {
						nodes = nodes.map((n) =>
							n.id === change.id
								? { ...n, measured: change.dimensions }
								: n
						);
					}
					break;
				case "select":
					nodes = nodes.map((n) =>
						n.id === change.id ? { ...n, selected: change.selected } : n
					);
					break;
				case "remove":
					nodes = nodes.filter((n) => n.id !== change.id);
					break;
			}
		}

		this.canvasState.nodes = nodes;
	};

	/**
	 * Handle edge changes from xyflow
	 */
	onEdgesChange = (changes: EdgeChange[]): void => {
		let edges = [...this.canvasState.edges];

		for (const change of changes) {
			switch (change.type) {
				case "select":
					edges = edges.map((e) =>
						e.id === change.id ? { ...e, selected: change.selected } : e
					);
					break;
				case "remove":
					edges = edges.filter((e) => e.id !== change.id);
					break;
			}
		}

		this.canvasState.edges = edges;
	};

	/**
	 * Handle new connection from xyflow
	 */
	onConnect = (connection: Connection): void => {
		if (connection.source && connection.target) {
			this.connect(
				connection.source,
				connection.target,
				connection.sourceHandle ?? undefined,
				connection.targetHandle ?? undefined
			);
		}
	};

	// === QUERY EXECUTION ===

	/**
	 * Execute a query node and create/update result node
	 */
	async executeQueryNode(nodeId: string): Promise<void> {
		const node = this.canvasState.getNode(nodeId);
		if (!node || node.data.type !== "query") {
			throw new Error("Invalid query node");
		}

		const queryData = node.data as CanvasQueryNodeData;

		// Mark as executing
		this.updateNodeData<CanvasQueryNodeData>(nodeId, {
			isExecuting: true,
			error: undefined,
		});

		const startTime = performance.now();

		try {
			const rows = await this.executeQuery(queryData.query);
			const executionTime = performance.now() - startTime;
			const columns = rows.length > 0 ? Object.keys(rows[0]) : [];

			// Update query node
			this.updateNodeData<CanvasQueryNodeData>(nodeId, {
				isExecuting: false,
				executionTime,
				error: undefined,
			});

			// Find existing result node or create new one
			const existingResultNode = this.canvasState.nodes.find(
				(n) =>
					n.data.type === "result" &&
					(n.data as CanvasResultNodeData).sourceQueryNodeId === nodeId
			);

			let resultNodeId: string;

			if (existingResultNode) {
				resultNodeId = existingResultNode.id;
				// Update existing result node
				this.updateNodeData<CanvasResultNodeData>(existingResultNode.id, {
					columns,
					rows,
					totalRows: rows.length,
					executionTime,
					error: undefined,
				});
			} else {
				// Create new result node
				resultNodeId = this.addResultNode(nodeId, columns, rows, rows.length, executionTime);
			}

			// Update any downstream chart nodes connected to the result node
			this.updateDownstreamChartNodes(resultNodeId, columns, rows);

			// Add timeline entry
			this.addTimelineEntry({
				type: "query",
				description: `Executed query (${rows.length} rows)`,
				nodeId,
			});
		} catch (error) {
			const errorMessage = error instanceof Error ? error.message : String(error);
			this.updateNodeData<CanvasQueryNodeData>(nodeId, {
				isExecuting: false,
				error: errorMessage,
			});

			// Update result node with error if it exists
			const existingResultNode = this.canvasState.nodes.find(
				(n) =>
					n.data.type === "result" &&
					(n.data as CanvasResultNodeData).sourceQueryNodeId === nodeId
			);

			if (existingResultNode) {
				this.updateNodeData<CanvasResultNodeData>(existingResultNode.id, {
					error: errorMessage,
					columns: [],
					rows: [],
					totalRows: 0,
				});

				// Clear downstream chart nodes on error
				this.updateDownstreamChartNodes(existingResultNode.id, [], []);
			}
		}
	}

	/**
	 * Update all chart nodes that are connected to a given source node
	 */
	private updateDownstreamChartNodes(
		sourceNodeId: string,
		columns: string[],
		rows: Record<string, unknown>[]
	): void {
		const chartNodes = this.canvasState.nodes.filter(
			(n) =>
				n.data.type === "chart" &&
				(n.data as CanvasChartNodeData).sourceNodeId === sourceNodeId
		);

		for (const chartNode of chartNodes) {
			const chartData = chartNode.data as CanvasChartNodeData;
			// Recalculate chart config if columns changed significantly
			const newConfig = this.shouldRecalculateChartConfig(chartData, columns)
				? createDefaultChartConfig(columns, rows)
				: chartData.chartConfig;

			this.updateNodeData<CanvasChartNodeData>(chartNode.id, {
				columns,
				rows,
				chartConfig: newConfig,
			});
		}
	}

	/**
	 * Check if chart config should be recalculated based on column changes
	 */
	private shouldRecalculateChartConfig(
		chartData: CanvasChartNodeData,
		newColumns: string[]
	): boolean {
		// Recalculate if the configured axes are no longer valid
		const { xAxis, yAxis } = chartData.chartConfig;

		if (xAxis && !newColumns.includes(xAxis)) {
			return true;
		}

		if (yAxis.length > 0 && !yAxis.some((col) => newColumns.includes(col))) {
			return true;
		}

		return false;
	}

	// === CANVAS MANAGEMENT ===

	/**
	 * Clear the current canvas
	 */
	clearCanvas(): void {
		this.canvasState.nodes = [];
		this.canvasState.edges = [];
		this.canvasState.activeCanvasId = null;
	}

	/**
	 * Save the current canvas
	 */
	saveCanvas(name: string): SavedCanvas {
		const projectId = this.state.activeProjectId;
		if (!projectId) {
			throw new Error("No active project");
		}

		const now = new Date().toISOString();
		const existingId = this.canvasState.activeCanvasId;

		// Serialize nodes and edges
		const serializedNodes: SerializedCanvasNode[] = this.canvasState.nodes.map((node) => ({
			id: node.id,
			type: node.type ?? "unknown",
			position: node.position,
			data: node.data,
			width: node.measured?.width ?? node.width,
			height: node.measured?.height ?? node.height,
		}));

		const serializedEdges: SerializedCanvasEdge[] = this.canvasState.edges.map((edge) => ({
			id: edge.id,
			source: edge.source,
			target: edge.target,
			sourceHandle: edge.sourceHandle,
			targetHandle: edge.targetHandle,
		}));

		if (existingId) {
			// Update existing canvas
			const savedCanvases = this.state.savedCanvasesByProject[projectId] ?? [];
			const index = savedCanvases.findIndex((c) => c.id === existingId);

			if (index !== -1) {
				const updated: SavedCanvas = {
					...savedCanvases[index],
					name,
					nodes: serializedNodes,
					edges: serializedEdges,
					viewport: this.canvasState.viewport,
					updatedAt: now,
				};

				this.state.savedCanvasesByProject = {
					...this.state.savedCanvasesByProject,
					[projectId]: [
						...savedCanvases.slice(0, index),
						updated,
						...savedCanvases.slice(index + 1),
					],
				};

				this.addTimelineEntry({
					type: "canvas-save",
					description: `Saved canvas "${name}"`,
				});

				this.schedulePersistence(projectId);
				return updated;
			}
		}

		// Create new canvas
		const newCanvas: SavedCanvas = {
			id: crypto.randomUUID(),
			name,
			projectId,
			nodes: serializedNodes,
			edges: serializedEdges,
			viewport: this.canvasState.viewport,
			createdAt: now,
			updatedAt: now,
		};

		const savedCanvases = this.state.savedCanvasesByProject[projectId] ?? [];
		this.state.savedCanvasesByProject = {
			...this.state.savedCanvasesByProject,
			[projectId]: [...savedCanvases, newCanvas],
		};
		this.canvasState.activeCanvasId = newCanvas.id;

		this.addTimelineEntry({
			type: "canvas-save",
			description: `Saved canvas "${name}"`,
		});

		this.schedulePersistence(projectId);
		return newCanvas;
	}

	/**
	 * Load a saved canvas
	 */
	loadCanvas(canvasId: string): void {
		const projectId = this.state.activeProjectId;
		if (!projectId) {
			throw new Error("No active project");
		}

		const savedCanvases = this.state.savedCanvasesByProject[projectId] ?? [];
		const canvas = savedCanvases.find((c) => c.id === canvasId);

		if (!canvas) {
			throw new Error("Canvas not found");
		}

		// Restore nodes
		this.canvasState.nodes = canvas.nodes.map((serialized) => ({
			id: serialized.id,
			type: serialized.type,
			position: serialized.position,
			data: serialized.data,
			width: serialized.width,
			height: serialized.height,
		}));

		// Restore edges
		this.canvasState.edges = canvas.edges.map((serialized) => ({
			id: serialized.id,
			source: serialized.source,
			target: serialized.target,
			sourceHandle: serialized.sourceHandle,
			targetHandle: serialized.targetHandle,
		}));

		// Restore viewport
		this.canvasState.viewport = canvas.viewport;
		this.canvasState.activeCanvasId = canvasId;

		this.addTimelineEntry({
			type: "canvas-load",
			description: `Loaded canvas "${canvas.name}"`,
		});
	}

	/**
	 * Delete a saved canvas
	 */
	deleteCanvas(canvasId: string): void {
		const projectId = this.state.activeProjectId;
		if (!projectId) return;

		const savedCanvases = this.state.savedCanvasesByProject[projectId] ?? [];
		this.state.savedCanvasesByProject = {
			...this.state.savedCanvasesByProject,
			[projectId]: savedCanvases.filter((c) => c.id !== canvasId),
		};

		// Clear canvas if it was the active one
		if (this.canvasState.activeCanvasId === canvasId) {
			this.clearCanvas();
		}

		this.schedulePersistence(projectId);
	}

	/**
	 * Rename a saved canvas
	 */
	renameCanvas(canvasId: string, newName: string): void {
		const projectId = this.state.activeProjectId;
		if (!projectId) return;

		const savedCanvases = this.state.savedCanvasesByProject[projectId] ?? [];
		this.state.savedCanvasesByProject = {
			...this.state.savedCanvasesByProject,
			[projectId]: savedCanvases.map((c) =>
				c.id === canvasId
					? { ...c, name: newName, updatedAt: new Date().toISOString() }
					: c
			),
		};

		this.schedulePersistence(projectId);
	}

	// === TIMELINE ===

	/**
	 * Add a timeline entry
	 */
	addTimelineEntry(entry: Omit<CanvasTimelineEntry, "id" | "timestamp">): void {
		const newEntry: CanvasTimelineEntry = {
			id: crypto.randomUUID(),
			timestamp: new Date().toISOString(),
			...entry,
		};

		// Keep last 100 entries
		this.canvasState.timeline = [newEntry, ...this.canvasState.timeline].slice(0, 100);
	}

	/**
	 * Clear timeline
	 */
	clearTimeline(): void {
		this.canvasState.timeline = [];
	}

	// === HELPERS ===

	/**
	 * Get the next available position for a new node
	 */
	private getNextNodePosition(): XYPosition {
		if (this.canvasState.nodes.length === 0) {
			return { x: 100, y: 100 };
		}

		// Find the rightmost node and place new node to its right
		const rightmostNode = this.canvasState.nodes.reduce((rightmost, node) => {
			return node.position.x > rightmost.position.x ? node : rightmost;
		}, this.canvasState.nodes[0]);

		return {
			x: rightmostNode.position.x + DEFAULT_NODE_WIDTH + 50,
			y: rightmostNode.position.y,
		};
	}
}
