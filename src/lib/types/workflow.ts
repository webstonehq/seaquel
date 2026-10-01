import type { Node, XYPosition, Viewport } from "@xyflow/svelte";
import type { ForeignKeyRef, ChartConfig } from "$lib/types";

// Node type identifiers
export type WorkflowNodeType = "table" | "query" | "result" | "chart";

// Table node data - displays table metadata (columns, indexes, etc.)
export interface WorkflowTableNodeData extends Record<string, unknown> {
  type: "table";
  tableName: string;
  schemaName: string;
  connectionId: string;
  tableType: "table" | "view" | "materialized-view";
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
export interface WorkflowQueryNodeData extends Record<string, unknown> {
  type: "query";
  name: string;
  query: string;
  connectionId: string;
  isExecuting: boolean;
  error?: string;
  executionTime?: number;
}

// Result node data - displays query results
export interface WorkflowResultNodeData extends Record<string, unknown> {
  type: "result";
  sourceQueryNodeId: string;
  columns: string[];
  /** Columnar rows — `rows[i][j]` is the value in column `columns[j]`. */
  rows: unknown[][];
  totalRows: number;
  executionTime?: number;
  /** The query had more rows than a node fetches (`WORKFLOW_MAX_ROWS`); these are the first. */
  truncated?: boolean;
  error?: string;
}

// Chart node data - visualizes data as a chart
export interface WorkflowChartNodeData extends Record<string, unknown> {
  type: "chart";
  sourceNodeId: string;
  columns: string[];
  /** Columnar rows — `rows[i][j]` is the value in column `columns[j]`. */
  rows: unknown[][];
  chartConfig: ChartConfig;
}

// Union type for all node data
export type WorkflowNodeData =
  | WorkflowTableNodeData
  | WorkflowQueryNodeData
  | WorkflowResultNodeData
  | WorkflowChartNodeData;

// Type alias for workflow nodes (using generic Node with our data types)
export type WorkflowNode = Node<WorkflowNodeData>;

// Serialized node for persistence
export interface SerializedWorkflowNode {
  id: string;
  type: string;
  position: XYPosition;
  data: WorkflowNodeData;
  width?: number;
  height?: number;
}

// Serialized edge for persistence
export interface SerializedWorkflowEdge {
  id: string;
  source: string;
  target: string;
  sourceHandle?: string | null;
  targetHandle?: string | null;
}

// Saved workflow - a complete workflow state
export interface SavedWorkflow {
  id: string;
  name: string;
  projectId: string;
  nodes: SerializedWorkflowNode[];
  edges: SerializedWorkflowEdge[];
  viewport: Viewport;
  createdAt: string;
  updatedAt: string;
}

/**
 * A saved workflow as the sidebar lists it, without its nodes and edges
 * (`workflowsList`, phase 5d-2 Task 7): opening one fetches the body
 * (`WorkflowManager.loadWorkflow`). Times are absent on a workflow whose
 * stored JSON has none.
 */
export interface SavedWorkflowSummary {
  id: string;
  name: string;
  projectId: string;
  createdAt: string | null;
  updatedAt: string | null;
}

// Timeline entry for activity log
export interface WorkflowTimelineEntry {
  id: string;
  type: "query" | "table-open" | "workflow-save" | "workflow-load";
  description: string;
  timestamp: string;
  nodeId?: string;
}

// Workflow viewport state
export interface WorkflowViewport {
  x: number;
  y: number;
  zoom: number;
}

// Workflow tab - represents an open workflow workspace
export interface WorkflowTab {
  id: string;
  name: string;
  connectionId: string;
}
