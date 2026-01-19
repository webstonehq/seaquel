import type { Node, Edge, XYPosition, Viewport } from "@xyflow/svelte";
import type { ForeignKeyRef, ChartType, ChartConfig } from "$lib/types";

// Node type identifiers
export type CanvasNodeType = "table" | "query" | "result" | "chart";

// Table node data - displays table metadata (columns, indexes, etc.)
export interface CanvasTableNodeData extends Record<string, unknown> {
	type: "table";
	tableName: string;
	schemaName: string;
	connectionId: string;
	tableType: "table" | "view";
	rowCount?: number;
	columns: {
		name: string;
		type: string;
		nullable: boolean;
		defaultValue?: string;
		isPrimaryKey: boolean;
		isForeignKey: boolean;
		foreignKeyRef?: ForeignKeyRef;
	}[];
	indexes: {
		name: string;
		columns: string[];
		unique: boolean;
		type: string;
	}[];
}

// Query node data - SQL editor for executing queries
export interface CanvasQueryNodeData extends Record<string, unknown> {
	type: "query";
	name: string;
	query: string;
	connectionId: string;
	isExecuting: boolean;
	error?: string;
	executionTime?: number;
}

// Result node data - displays query results
export interface CanvasResultNodeData extends Record<string, unknown> {
	type: "result";
	sourceQueryNodeId: string;
	columns: string[];
	rows: Record<string, unknown>[];
	totalRows: number;
	executionTime?: number;
	error?: string;
}

// Chart node data - visualizes data as a chart
export interface CanvasChartNodeData extends Record<string, unknown> {
	type: "chart";
	sourceNodeId: string;
	columns: string[];
	rows: Record<string, unknown>[];
	chartConfig: ChartConfig;
}

// Union type for all node data
export type CanvasNodeData =
	| CanvasTableNodeData
	| CanvasQueryNodeData
	| CanvasResultNodeData
	| CanvasChartNodeData;

// Type alias for canvas nodes (using generic Node with our data types)
export type CanvasNode = Node<CanvasNodeData>;

// Serialized node for persistence
export interface SerializedCanvasNode {
	id: string;
	type: string;
	position: XYPosition;
	data: CanvasNodeData;
	width?: number;
	height?: number;
}

// Serialized edge for persistence
export interface SerializedCanvasEdge {
	id: string;
	source: string;
	target: string;
	sourceHandle?: string | null;
	targetHandle?: string | null;
}

// Saved canvas - a complete canvas state
export interface SavedCanvas {
	id: string;
	name: string;
	projectId: string;
	nodes: SerializedCanvasNode[];
	edges: SerializedCanvasEdge[];
	viewport: Viewport;
	createdAt: string;
	updatedAt: string;
}

// Timeline entry for activity log
export interface CanvasTimelineEntry {
	id: string;
	type: "query" | "table-open" | "canvas-save" | "canvas-load";
	description: string;
	timestamp: string;
	nodeId?: string;
}

// Canvas viewport state
export interface CanvasViewport {
	x: number;
	y: number;
	zoom: number;
}

// Canvas tab - represents an open canvas workspace
export interface CanvasTab {
	id: string;
	name: string;
	connectionId: string;
}
