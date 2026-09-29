import type { Node, Edge, XYPosition, Connection } from "@xyflow/svelte";
import type { NodeChange, EdgeChange } from "@xyflow/system";
import type { DatabaseState } from "./state.svelte.js";
import type { WorkflowState } from "./workflow-state.svelte.js";
import type {
  WorkflowNodeData,
  WorkflowTableNodeData,
  WorkflowQueryNodeData,
  WorkflowResultNodeData,
  WorkflowChartNodeData,
  SavedWorkflow,
  WorkflowTimelineEntry,
  SerializedWorkflowNode,
  SerializedWorkflowEdge,
} from "$lib/types/workflow";
import type { SchemaTable, ChartConfig } from "$lib/types";
import type { ReadOnlyRows } from "$lib/providers";
import { createDefaultChartConfig } from "$lib/components/charts/chart-utils";
import { READ_ONLY_REFUSAL } from "$lib/sql";
import { m } from "$lib/paraglide/messages.js";

const DEFAULT_NODE_WIDTH = 320;

/**
 * The most rows a workflow query node fetches (phase 5c, Decision 10).
 * Result rows are saved with the workflow, so the cap is well under the
 * engine's 100,000; a result past it is cut short and says so.
 */
export const WORKFLOW_MAX_ROWS = 10_000;

/**
 * Runs a query node's SQL read-only on the node's own saved connection
 * (`QueryCrudManager.executeReadOnly`), at most `maxRows` rows. Aborting
 * `signal` cancels it.
 */
export type RunWorkflowQuery = (
  connectionId: string,
  sql: string,
  signal: AbortSignal,
  maxRows: number,
) => Promise<ReadOnlyRows>;

/** A read-only refusal (the token check's, or the database's `READ_ONLY`), worded for a node. */
function nodeError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  if (message.startsWith(READ_ONLY_REFUSAL) || /^READ_ONLY\b/.test(message)) {
    return m.workflow_node_read_only();
  }
  return message;
}

/**
 * Workflow manager - handles all workflow operations
 */
export class WorkflowManager {
  /** Each query node's run in flight: re-running or deleting the node cancels it. */
  private runs = new Map<string, AbortController>();

  constructor(
    private state: DatabaseState,
    private workflowState: WorkflowState,
    private schedulePersistence: (projectId: string | null) => void,
    private executeQuery: RunWorkflowQuery,
  ) {}

  /** Cancel a query node's run in flight. */
  private cancelRun(nodeId: string): void {
    this.runs.get(nodeId)?.abort();
    this.runs.delete(nodeId);
  }

  // === NODE MANAGEMENT ===

  /**
   * Add a table node to the workflow
   */
  addTableNode(table: SchemaTable, position?: XYPosition): string {
    const id = `workflow-node-${crypto.randomUUID()}`;
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) {
      throw new Error("No active connection");
    }

    const nodePosition = position ?? this.getNextNodePosition();

    const data: WorkflowTableNodeData = {
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

    const node: Node<WorkflowTableNodeData> = {
      id,
      type: "tableNode",
      position: nodePosition,
      data,
      width: 280,
      height: 300,
    };

    this.workflowState.nodes = [...this.workflowState.nodes, node];

    // Add timeline entry
    this.addTimelineEntry({
      type: "table-open",
      description: `Opened ${table.schema}.${table.name}`,
      nodeId: id,
    });

    return id;
  }

  /**
   * Add a query node to the workflow
   */
  addQueryNode(query?: string, position?: XYPosition): string {
    const id = `workflow-node-${crypto.randomUUID()}`;
    const connectionId = this.state.activeConnectionId;
    if (!connectionId) {
      throw new Error("No active connection");
    }

    const nodePosition = position ?? this.getNextNodePosition();

    const data: WorkflowQueryNodeData = {
      type: "query",
      name: "Query",
      query: query ?? "",
      connectionId,
      isExecuting: false,
    };

    const node: Node<WorkflowQueryNodeData> = {
      id,
      type: "queryNode",
      position: nodePosition,
      data,
      width: 300,
      height: 150,
    };

    this.workflowState.nodes = [...this.workflowState.nodes, node];

    return id;
  }

  /**
   * Add a result node to the workflow
   */
  addResultNode(
    queryNodeId: string,
    columns: string[],
    rows: unknown[][],
    totalRows: number,
    executionTime?: number,
    position?: XYPosition,
    truncated?: boolean,
  ): string {
    const id = `workflow-node-${crypto.randomUUID()}`;

    // Position to the right of the query node
    const queryNode = this.workflowState.getNode(queryNodeId);
    const nodePosition = position ?? {
      x: (queryNode?.position.x ?? 0) + DEFAULT_NODE_WIDTH + 50,
      y: queryNode?.position.y ?? 0,
    };

    const data: WorkflowResultNodeData = {
      type: "result",
      sourceQueryNodeId: queryNodeId,
      columns,
      rows,
      totalRows,
      executionTime,
      ...(truncated ? { truncated: true } : {}),
    };

    const node: Node<WorkflowResultNodeData> = {
      id,
      type: "resultNode",
      position: nodePosition,
      data,
      width: 400,
      height: 350,
    };

    this.workflowState.nodes = [...this.workflowState.nodes, node];

    // Auto-connect query to result
    this.connect(queryNodeId, id, "output", "input");

    return id;
  }

  /**
   * Add a chart node to the workflow
   */
  addChartNode(
    sourceNodeId: string,
    columns: string[],
    rows: unknown[][],
    chartConfig?: ChartConfig,
    position?: XYPosition,
  ): string {
    const id = `workflow-node-${crypto.randomUUID()}`;

    // Position to the right of the source node
    const sourceNode = this.workflowState.getNode(sourceNodeId);
    const nodePosition = position ?? {
      x: (sourceNode?.position.x ?? 0) + DEFAULT_NODE_WIDTH + 50,
      y: sourceNode?.position.y ?? 0,
    };

    const config = chartConfig ?? createDefaultChartConfig(columns, rows);

    const data: WorkflowChartNodeData = {
      type: "chart",
      sourceNodeId,
      columns,
      rows,
      chartConfig: config,
    };

    const node: Node<WorkflowChartNodeData> = {
      id,
      type: "chartNode",
      position: nodePosition,
      data,
      width: 450,
      height: 350,
    };

    this.workflowState.nodes = [...this.workflowState.nodes, node];

    // Auto-connect source to chart
    this.connect(sourceNodeId, id, "output", "input");

    return id;
  }

  /**
   * Remove a node and its connected edges
   */
  removeNode(nodeId: string): void {
    this.cancelRun(nodeId);
    // Remove connected edges first
    const connectedEdges = this.workflowState.getConnectedEdges(nodeId);
    for (const edge of connectedEdges) {
      this.disconnect(edge.id);
    }

    // Remove the node
    this.workflowState.nodes = this.workflowState.nodes.filter((n) => n.id !== nodeId);
  }

  /**
   * Update node data
   */
  updateNodeData<T extends WorkflowNodeData>(nodeId: string, updates: Partial<T>): void {
    this.workflowState.nodes = this.workflowState.nodes.map((node) => {
      if (node.id === nodeId) {
        return {
          ...node,
          data: { ...node.data, ...updates } as WorkflowNodeData,
        };
      }
      return node;
    });
  }

  /**
   * Update node dimensions after resize
   */
  updateNodeDimensions(nodeId: string, width: number, height: number): void {
    this.workflowState.nodes = this.workflowState.nodes.map((node) => {
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
    targetHandle?: string,
  ): string {
    const id = `${sourceId}-${targetId}`;

    const edge: Edge = {
      id,
      source: sourceId,
      target: targetId,
      sourceHandle: sourceHandle ?? null,
      targetHandle: targetHandle ?? null,
    };

    this.workflowState.edges = [...this.workflowState.edges, edge];

    return id;
  }

  /**
   * Disconnect an edge
   */
  disconnect(edgeId: string): void {
    this.workflowState.edges = this.workflowState.edges.filter((e) => e.id !== edgeId);
  }

  // === XYFLOW CALLBACKS ===

  /**
   * Handle node changes from xyflow
   */
  onNodesChange = (changes: NodeChange[]): void => {
    let nodes = [...this.workflowState.nodes];

    for (const change of changes) {
      switch (change.type) {
        case "position":
          if (change.position) {
            nodes = nodes.map((n) =>
              n.id === change.id ? { ...n, position: change.position! } : n,
            );
          }
          break;
        case "dimensions":
          if (change.dimensions) {
            nodes = nodes.map((n) =>
              n.id === change.id ? { ...n, measured: change.dimensions } : n,
            );
          }
          break;
        case "select":
          nodes = nodes.map((n) => (n.id === change.id ? { ...n, selected: change.selected } : n));
          break;
        case "remove":
          this.cancelRun(change.id);
          nodes = nodes.filter((n) => n.id !== change.id);
          break;
      }
    }

    this.workflowState.nodes = nodes;
  };

  /**
   * Handle edge changes from xyflow
   */
  onEdgesChange = (changes: EdgeChange[]): void => {
    let edges = [...this.workflowState.edges];

    for (const change of changes) {
      switch (change.type) {
        case "select":
          edges = edges.map((e) => (e.id === change.id ? { ...e, selected: change.selected } : e));
          break;
        case "remove":
          edges = edges.filter((e) => e.id !== change.id);
          break;
      }
    }

    this.workflowState.edges = edges;
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
        connection.targetHandle ?? undefined,
      );
    }
  };

  // === QUERY EXECUTION ===

  /**
   * Run a query node read-only on its own saved connection (phase 5c,
   * Decision 10), at most `WORKFLOW_MAX_ROWS` rows, and create or update its
   * result node. A write is refused as read-only. Re-running the node
   * cancels its run in flight; only the latest run updates the canvas.
   */
  async executeQueryNode(nodeId: string): Promise<void> {
    const node = this.workflowState.getNode(nodeId);
    if (!node || node.data.type !== "query") {
      throw new Error("Invalid query node");
    }

    const queryData = node.data as WorkflowQueryNodeData;

    this.cancelRun(nodeId);
    const controller = new AbortController();
    this.runs.set(nodeId, controller);
    const superseded = () => controller.signal.aborted || this.runs.get(nodeId) !== controller;

    // Mark as executing
    this.updateNodeData<WorkflowQueryNodeData>(nodeId, {
      isExecuting: true,
      error: undefined,
    });

    const startTime = performance.now();

    try {
      const { rows: rowObjects, truncated } = await this.executeQuery(
        queryData.connectionId,
        queryData.query,
        controller.signal,
        WORKFLOW_MAX_ROWS,
      );
      if (superseded()) return;
      const executionTime = performance.now() - startTime;
      const columns = rowObjects.length > 0 ? Object.keys(rowObjects[0]) : [];
      // Result/chart nodes use the columnar row shape: convert once here.
      const rows: unknown[][] = rowObjects.map((r) => columns.map((c) => r[c]));

      // Update query node
      this.updateNodeData<WorkflowQueryNodeData>(nodeId, {
        isExecuting: false,
        executionTime,
        error: undefined,
      });

      // Find existing result node or create new one
      const existingResultNode = this.workflowState.nodes.find(
        (n) =>
          n.data.type === "result" &&
          (n.data as WorkflowResultNodeData).sourceQueryNodeId === nodeId,
      );

      let resultNodeId: string;

      if (existingResultNode) {
        resultNodeId = existingResultNode.id;
        // Update existing result node
        this.updateNodeData<WorkflowResultNodeData>(existingResultNode.id, {
          columns,
          rows,
          totalRows: rows.length,
          executionTime,
          truncated,
          error: undefined,
        });
      } else {
        // Create new result node
        resultNodeId = this.addResultNode(
          nodeId,
          columns,
          rows,
          rows.length,
          executionTime,
          undefined,
          truncated,
        );
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
      if (superseded()) return;
      const errorMessage = nodeError(error);
      this.updateNodeData<WorkflowQueryNodeData>(nodeId, {
        isExecuting: false,
        error: errorMessage,
      });

      // Update result node with error if it exists
      const existingResultNode = this.workflowState.nodes.find(
        (n) =>
          n.data.type === "result" &&
          (n.data as WorkflowResultNodeData).sourceQueryNodeId === nodeId,
      );

      if (existingResultNode) {
        this.updateNodeData<WorkflowResultNodeData>(existingResultNode.id, {
          error: errorMessage,
          columns: [],
          rows: [],
          totalRows: 0,
          truncated: false,
        });

        // Clear downstream chart nodes on error
        this.updateDownstreamChartNodes(existingResultNode.id, [], []);
      }
    } finally {
      if (this.runs.get(nodeId) === controller) this.runs.delete(nodeId);
    }
  }

  /**
   * Update all chart nodes that are connected to a given source node
   */
  private updateDownstreamChartNodes(
    sourceNodeId: string,
    columns: string[],
    rows: unknown[][],
  ): void {
    const chartNodes = this.workflowState.nodes.filter(
      (n) =>
        n.data.type === "chart" && (n.data as WorkflowChartNodeData).sourceNodeId === sourceNodeId,
    );

    for (const chartNode of chartNodes) {
      const chartData = chartNode.data as WorkflowChartNodeData;
      // Recalculate chart config if columns changed significantly
      const newConfig = this.shouldRecalculateChartConfig(chartData, columns)
        ? createDefaultChartConfig(columns, rows)
        : chartData.chartConfig;

      this.updateNodeData<WorkflowChartNodeData>(chartNode.id, {
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
    chartData: WorkflowChartNodeData,
    newColumns: string[],
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

  // === WORKFLOW MANAGEMENT ===

  /**
   * Clear the current workflow
   */
  clearWorkflow(): void {
    this.workflowState.nodes = [];
    this.workflowState.edges = [];
    this.workflowState.activeWorkflowId = null;
  }

  /**
   * Save the current workflow
   */
  saveWorkflow(name: string): SavedWorkflow {
    const projectId = this.state.activeProjectId;
    if (!projectId) {
      throw new Error("No active project");
    }

    const now = new Date().toISOString();
    const existingId = this.workflowState.activeWorkflowId;

    // Serialize nodes and edges
    const serializedNodes: SerializedWorkflowNode[] = this.workflowState.nodes.map((node) => ({
      id: node.id,
      type: node.type ?? "unknown",
      position: node.position,
      data: node.data,
      width: node.measured?.width ?? node.width,
      height: node.measured?.height ?? node.height,
    }));

    const serializedEdges: SerializedWorkflowEdge[] = this.workflowState.edges.map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      sourceHandle: edge.sourceHandle,
      targetHandle: edge.targetHandle,
    }));

    if (existingId) {
      // Update existing workflow
      const savedWorkflows = this.state.savedWorkflowsByProject[projectId] ?? [];
      const index = savedWorkflows.findIndex((c) => c.id === existingId);

      if (index !== -1) {
        const updated: SavedWorkflow = {
          ...savedWorkflows[index],
          name,
          nodes: serializedNodes,
          edges: serializedEdges,
          viewport: this.workflowState.viewport,
          updatedAt: now,
        };

        this.state.savedWorkflowsByProject = {
          ...this.state.savedWorkflowsByProject,
          [projectId]: [
            ...savedWorkflows.slice(0, index),
            updated,
            ...savedWorkflows.slice(index + 1),
          ],
        };

        this.addTimelineEntry({
          type: "workflow-save",
          description: `Saved workflow "${name}"`,
        });

        this.schedulePersistence(projectId);
        return updated;
      }
    }

    // Create new workflow
    const newWorkflow: SavedWorkflow = {
      id: `workflow-${crypto.randomUUID()}`,
      name,
      projectId,
      nodes: serializedNodes,
      edges: serializedEdges,
      viewport: this.workflowState.viewport,
      createdAt: now,
      updatedAt: now,
    };

    const savedWorkflows = this.state.savedWorkflowsByProject[projectId] ?? [];
    this.state.savedWorkflowsByProject = {
      ...this.state.savedWorkflowsByProject,
      [projectId]: [...savedWorkflows, newWorkflow],
    };
    this.workflowState.activeWorkflowId = newWorkflow.id;

    this.addTimelineEntry({
      type: "workflow-save",
      description: `Saved workflow "${name}"`,
    });

    this.schedulePersistence(projectId);
    return newWorkflow;
  }

  /**
   * Load a saved workflow
   */
  loadWorkflow(workflowId: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) {
      throw new Error("No active project");
    }

    const savedWorkflows = this.state.savedWorkflowsByProject[projectId] ?? [];
    const workflow = savedWorkflows.find((c) => c.id === workflowId);

    if (!workflow) {
      throw new Error("Workflow not found");
    }

    // Restore nodes
    this.workflowState.nodes = workflow.nodes.map((serialized) => ({
      id: serialized.id,
      type: serialized.type,
      position: serialized.position,
      data: serialized.data,
      width: serialized.width,
      height: serialized.height,
    }));

    // Restore edges
    this.workflowState.edges = workflow.edges.map((serialized) => ({
      id: serialized.id,
      source: serialized.source,
      target: serialized.target,
      sourceHandle: serialized.sourceHandle,
      targetHandle: serialized.targetHandle,
    }));

    // Restore viewport
    this.workflowState.viewport = workflow.viewport;
    this.workflowState.activeWorkflowId = workflowId;

    this.addTimelineEntry({
      type: "workflow-load",
      description: `Loaded workflow "${workflow.name}"`,
    });
  }

  /**
   * Delete a saved workflow
   */
  deleteWorkflow(workflowId: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const savedWorkflows = this.state.savedWorkflowsByProject[projectId] ?? [];
    this.state.savedWorkflowsByProject = {
      ...this.state.savedWorkflowsByProject,
      [projectId]: savedWorkflows.filter((c) => c.id !== workflowId),
    };

    // Clear workflow if it was the active one
    if (this.workflowState.activeWorkflowId === workflowId) {
      this.clearWorkflow();
    }

    this.schedulePersistence(projectId);
  }

  /**
   * Rename a saved workflow
   */
  renameWorkflow(workflowId: string, newName: string): void {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;

    const savedWorkflows = this.state.savedWorkflowsByProject[projectId] ?? [];
    this.state.savedWorkflowsByProject = {
      ...this.state.savedWorkflowsByProject,
      [projectId]: savedWorkflows.map((c) =>
        c.id === workflowId ? { ...c, name: newName, updatedAt: new Date().toISOString() } : c,
      ),
    };

    this.schedulePersistence(projectId);
  }

  // === TIMELINE ===

  /**
   * Add a timeline entry
   */
  addTimelineEntry(entry: Omit<WorkflowTimelineEntry, "id" | "timestamp">): void {
    const newEntry: WorkflowTimelineEntry = {
      id: `timeline-${crypto.randomUUID()}`,
      timestamp: new Date().toISOString(),
      ...entry,
    };

    // Keep last 100 entries
    this.workflowState.timeline = [newEntry, ...this.workflowState.timeline].slice(0, 100);
  }

  /**
   * Clear timeline
   */
  clearTimeline(): void {
    this.workflowState.timeline = [];
  }

  // === HELPERS ===

  /**
   * Get the next available position for a new node
   */
  private getNextNodePosition(): XYPosition {
    if (this.workflowState.nodes.length === 0) {
      return { x: 100, y: 100 };
    }

    // Find the rightmost node and place new node to its right
    const rightmostNode = this.workflowState.nodes.reduce((rightmost, node) => {
      return node.position.x > rightmost.position.x ? node : rightmost;
    }, this.workflowState.nodes[0]);

    return {
      x: rightmostNode.position.x + DEFAULT_NODE_WIDTH + 50,
      y: rightmostNode.position.y,
    };
  }
}
