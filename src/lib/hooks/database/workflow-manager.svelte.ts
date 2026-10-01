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
  SavedWorkflowSummary,
  WorkflowTimelineEntry,
  SerializedWorkflowNode,
  SerializedWorkflowEdge,
} from "$lib/types/workflow";
import type { SchemaTable, ChartConfig } from "$lib/types";
import type { ReadOnlyRows } from "$lib/providers";
import { createDefaultChartConfig } from "$lib/components/charts/chart-utils";
import { READ_ONLY_REFUSAL } from "$lib/sql";
import { m } from "$lib/paraglide/messages.js";
import { fromStorable, toStorable } from "$lib/values";
import { errorToast } from "$lib/utils/toast";
import { log } from "$lib/utils/logger";
import { errorCode } from "$lib/core/client";
import { getLibrary, rowKey, NEW, WORKFLOW_NOT_FOUND } from "./library/index.js";
import { workflowSummaryFromWire } from "./library/convert.js";
import { libraryErrorMessage, limitMessage, limitOf } from "./library/messages.js";

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

/** A node's rows, if its data holds any. */
function rowsOf(data: WorkflowNodeData | undefined): unknown[][] | null {
  const rows = (data as { rows?: unknown } | undefined)?.rows;
  return Array.isArray(rows) && rows.length > 0 ? (rows as unknown[][]) : null;
}

/**
 * The nodes as stored (Q16, Decision 23): a chart node whose source, in the
 * same workflow, holds rows is stored with `rows: []`, since the load
 * rebuilds it from there. Any other chart keeps its rows.
 */
export function dropChartCopies(nodes: SerializedWorkflowNode[]): SerializedWorkflowNode[] {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  return nodes.map((node) => {
    if (node.data.type !== "chart") return node;
    const data = node.data as WorkflowChartNodeData;
    const source = byId.get(data.sourceNodeId)?.data;
    // A chart fed by another chart keeps its rows: that source's own copy
    // may be dropped here too, and the load fills from the stored nodes,
    // so there would be nothing to rebuild this one from.
    if (source?.type === "chart") return node;
    if (!rowsOf(data) || !rowsOf(source)) return node;
    return { ...node, data: { ...data, rows: [] } };
  });
}

/**
 * The nodes as shown: a chart node stored without rows takes its source's
 * columns and rows, as a run does, keeping its chart config. A chart that
 * has rows (a workflow saved before 5d-2) uses them as they are.
 */
export function fillChartsFromSources(nodes: SerializedWorkflowNode[]): SerializedWorkflowNode[] {
  const byId = new Map(nodes.map((n) => [n.id, n]));
  return nodes.map((node) => {
    if (node.data.type !== "chart") return node;
    const data = node.data as WorkflowChartNodeData;
    if (rowsOf(data)) return node;
    const source = byId.get(data.sourceNodeId)?.data;
    const rows = rowsOf(source);
    if (!rows) return node;
    const columns = (source as { columns?: string[] }).columns ?? data.columns;
    return { ...node, data: { ...data, columns, rows } };
  });
}

/**
 * Workflow manager - handles all workflow operations
 */
export class WorkflowManager {
  /** Each query node's run in flight: re-running or deleting the node cancels it. */
  private runs = new Map<string, AbortController>();

  /** Workflows (by id, or `new:<name>`) whose save was refused as too large, told once. */
  private toldTooLarge = new Set<string>();
  /** Counts opens (and new workflows): only the latest open's body lands. */
  private opening = 0;
  /** The workflow the latest open is reading, until it lands. */
  private openingId: string | null = null;

  constructor(
    private state: DatabaseState,
    private workflowState: WorkflowState,
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
    // An open still reading its body doesn't land over the new workflow.
    this.opening++;
    this.workflowState.nodes = [];
    this.workflowState.edges = [];
    this.workflowState.activeWorkflowId = null;
  }

  /**
   * Save the current workflow (Decision 23): a new one is `workflowCreate`
   * (Core gives it its id and times), one already saved `workflowUpdate`.
   * `activeWorkflowId` is looked up in every project the page holds, since
   * the canvas is global: after a project switch it still names the
   * workflow it shows, which is updated where it lives (bug 22); an id no
   * project holds (deleted) saves a new one here. Chart nodes don't store
   * their source's rows again (Q16). `null` when it wasn't saved: the error
   * is shown and the canvas stays as it is.
   */
  async saveWorkflow(name?: string): Promise<SavedWorkflowSummary | null> {
    const projectId = this.state.activeProjectId;
    if (!projectId) {
      throw new Error("No active project");
    }
    const existing = this.findSaved(this.workflowState.activeWorkflowId);
    // Named: the name given; else the workflow's own (in whichever project
    // holds it), or a new one's default.
    const saveName = name ?? existing?.workflow.name ?? `Workflow ${new Date().toLocaleString()}`;

    // Serialize nodes and edges; chart nodes don't store their source's rows again
    const serializedNodes: SerializedWorkflowNode[] = dropChartCopies(
      this.workflowState.nodes.map((node) => ({
        id: node.id,
        type: node.type ?? "unknown",
        position: node.position,
        data: node.data,
        width: node.measured?.width ?? node.width,
        height: node.measured?.height ?? node.height,
      })),
    );

    const serializedEdges: SerializedWorkflowEdge[] = this.workflowState.edges.map((edge) => ({
      id: edge.id,
      source: edge.source,
      target: edge.target,
      sourceHandle: edge.sourceHandle,
      targetHandle: edge.targetHandle,
    }));

    const body = {
      name: saveName,
      nodes: serializedNodes,
      edges: serializedEdges,
      viewport: this.workflowState.viewport,
    };
    const saved = existing
      ? await this.update(existing.workflow.id, body, saveName)
      : await this.create(projectId, body, saveName);
    if (!saved) return null;
    this.workflowState.activeWorkflowId = saved.id;
    this.addTimelineEntry({
      type: "workflow-save",
      description: `Saved workflow "${saveName}"`,
    });
    return saved;
  }

  /** The saved workflow with `id` in any project the page holds, with its project. */
  private findSaved(
    id: string | null,
  ): { projectId: string; workflow: SavedWorkflowSummary } | null {
    if (!id) return null;
    for (const [projectId, list] of Object.entries(this.state.savedWorkflowsByProject)) {
      const workflow = list.find((w) => w.id === id);
      if (workflow) return { projectId, workflow };
    }
    return null;
  }

  /**
   * The stored workflow Core answered, listed as the sidebar lists it (the
   * page holds no bodies: opening one reads it, 5d-2 Task 7).
   */
  private show(stored: unknown): SavedWorkflowSummary | null {
    const o = (stored ?? {}) as Record<string, unknown>;
    const str = (v: unknown) => (typeof v === "string" ? v : null);
    const id = str(o.id);
    const projectId = str(o.projectId);
    if (!id || !projectId) return null;
    const workflow: SavedWorkflowSummary = {
      id,
      projectId,
      name: str(o.name) ?? "",
      createdAt: str(o.createdAt),
      updatedAt: str(o.updatedAt),
    };
    this.list(workflow);
    return workflow;
  }

  /** Shows `workflow` in its project's list, in place or at the end. */
  private list(workflow: SavedWorkflowSummary): void {
    const list = this.state.savedWorkflowsByProject[workflow.projectId] ?? [];
    this.state.savedWorkflowsByProject = {
      ...this.state.savedWorkflowsByProject,
      [workflow.projectId]: list.some((w) => w.id === workflow.id)
        ? list.map((w) => (w.id === workflow.id ? workflow : w))
        : [...list, workflow],
    };
  }

  private async create(
    projectId: string,
    body: Record<string, unknown>,
    name: string,
  ): Promise<SavedWorkflowSummary | null> {
    try {
      const { value, seq } = await this.state.librarySeqs.write([rowKey("workflow", NEW)], () =>
        getLibrary().createWorkflow(projectId, toStorable(body)),
      );
      const workflow = this.show(value);
      if (workflow) this.state.librarySeqs.note(rowKey("workflow", workflow.id), seq);
      return workflow;
    } catch (error) {
      this.refused(`new:${projectId}:${name}`, name, error);
      return null;
    }
  }

  private async update(
    id: string,
    body: Record<string, unknown>,
    name: string,
  ): Promise<SavedWorkflowSummary | null> {
    try {
      const { value, seq } = await this.state.librarySeqs.write([rowKey("workflow", id)], () =>
        getLibrary().updateWorkflow(id, toStorable(body)),
      );
      this.state.librarySeqs.note(rowKey("workflow", id), seq);
      return this.show(value);
    } catch (error) {
      this.refused(id, name, error);
      return null;
    }
  }

  /**
   * A refused save. Past the web's `max_workflow_bytes` (Q16) it says so
   * once per workflow, naming it and the limit; anything else each time.
   */
  private refused(key: string, name: string, error: unknown): void {
    void log.error("Failed to save a workflow:", error);
    const limit = limitOf(error);
    if (limit) {
      const other = limitMessage(limit);
      if (other) {
        errorToast(m.workflow_save_failed({ message: other }));
        return;
      }
      if (this.toldTooLarge.has(key)) return;
      this.toldTooLarge.add(key);
      errorToast(m.workflow_too_large({ name, limit }));
      return;
    }
    errorToast(m.workflow_save_failed({ message: libraryErrorMessage(error) }));
  }

  /**
   * Open a saved workflow on the canvas: its body is read now
   * (`workflowGet`), since the list holds none (5d-2 Task 7). Only the
   * latest open lands: one clicked while another is being read wins. A
   * workflow that can't be read is said and the canvas stays; one deleted
   * elsewhere is also dropped from the list. False when nothing opened.
   */
  async loadWorkflow(workflowId: string): Promise<boolean> {
    const projectId = this.state.activeProjectId;
    if (!projectId) {
      throw new Error("No active project");
    }

    const savedWorkflows = this.state.savedWorkflowsByProject[projectId] ?? [];
    const listed = savedWorkflows.find((c) => c.id === workflowId);

    if (!listed) {
      throw new Error("Workflow not found");
    }

    const open = ++this.opening;
    this.openingId = workflowId;
    let workflow: SavedWorkflow;
    try {
      const { value } = await getLibrary().getWorkflow(workflowId);
      workflow = fromStorable(value) as SavedWorkflow;
      if (!workflow || !Array.isArray(workflow.nodes) || !Array.isArray(workflow.edges)) {
        throw new Error("The saved workflow isn't a workflow.");
      }
    } catch (error) {
      if (open !== this.opening) return false;
      void log.error("Failed to open a saved workflow:", error);
      errorToast(m.workflow_open_failed({ message: libraryErrorMessage(error) }));
      if (errorCode(error) === WORKFLOW_NOT_FOUND) {
        void this.refreshFromLibrary(projectId, [workflowId]).catch(() => {});
      }
      return false;
    }
    // Another open (or a new workflow, or deleting this one) since this
    // one started wins.
    if (open !== this.opening) return false;
    this.openingId = null;

    // Restore nodes, charts filled from their sources' rows
    this.workflowState.nodes = fillChartsFromSources(workflow.nodes).map((serialized) => ({
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
      description: `Loaded workflow "${listed.name}"`,
    });
    return true;
  }

  /**
   * Delete a saved workflow (`workflowRemove`). The canvas showing it keeps
   * its nodes, no longer linked to it.
   */
  async deleteWorkflow(workflowId: string): Promise<void> {
    const found = this.findSaved(workflowId);
    try {
      const { seq } = await this.state.librarySeqs.write([rowKey("workflow", workflowId)], () =>
        getLibrary().removeWorkflow(workflowId),
      );
      this.state.librarySeqs.note(rowKey("workflow", workflowId), seq);
    } catch (error) {
      void log.error("Failed to delete a workflow:", error);
      errorToast(m.workflow_delete_failed({ message: libraryErrorMessage(error) }));
      return;
    }
    this.forget(found?.projectId ?? this.state.activeProjectId, workflowId);

    // An open of it still reading its body never lands (5d-2 Task 7 review).
    if (this.openingId === workflowId) {
      this.opening++;
      this.openingId = null;
    }

    // Clear workflow if it was the active one
    if (this.workflowState.activeWorkflowId === workflowId) {
      this.clearWorkflow();
    }
  }

  /** Drops a workflow from the page's list of its project. */
  private forget(projectId: string | null, workflowId: string): void {
    if (!projectId) return;
    const savedWorkflows = this.state.savedWorkflowsByProject[projectId] ?? [];
    this.state.savedWorkflowsByProject = {
      ...this.state.savedWorkflowsByProject,
      [projectId]: savedWorkflows.filter((c) => c.id !== workflowId),
    };
  }

  /**
   * Rename a saved workflow (`workflowRename`): Core changes only the
   * stored name, on the row as stored, so a save another window made isn't
   * undone and the body never crosses (5d-2 Task 7 review). A workflow
   * saved before 5d-2 keeps its chart copies: only `saveWorkflow` drops them
   * (Decision 23).
   */
  async renameWorkflow(workflowId: string, newName: string): Promise<void> {
    const found = this.findSaved(workflowId);
    if (!found) return;
    try {
      const { value, seq } = await this.state.librarySeqs.write(
        [rowKey("workflow", workflowId)],
        () => getLibrary().renameWorkflow(workflowId, newName),
      );
      this.state.librarySeqs.note(rowKey("workflow", workflowId), seq);
      this.list(workflowSummaryFromWire(value));
    } catch (error) {
      this.refused(workflowId, newName, error);
      if (errorCode(error) === WORKFLOW_NOT_FOUND) {
        void this.refreshFromLibrary(found.projectId, [workflowId]).catch(() => {});
      }
    }
  }

  /**
   * Another window changed saved workflows of `projectId` (a `workflow`
   * event): read the list again if the page holds it, and apply each row by
   * the `seq` rule, after this page's own writes to it have answered. A
   * workflow deleted elsewhere leaves the canvas showing it, unlinked.
   */
  async refreshFromLibrary(
    projectId: string,
    ids: readonly string[] | null,
    { again = true } = {},
  ): Promise<void> {
    if (!(projectId in this.state.savedWorkflowsByProject)) return;
    const seqs = this.state.librarySeqs;
    await Promise.all((ids ?? [""]).map((id) => seqs.settled(rowKey("workflow", id))));
    const { value, seq } = await getLibrary().listWorkflows(projectId);
    const stored = new Map<string, SavedWorkflowSummary>(
      value.map((w) => [w.id, workflowSummaryFromWire(w)]),
    );
    const shown = this.state.savedWorkflowsByProject[projectId] ?? [];
    const wanted = ids === null ? null : new Set(ids);
    let next = [...shown];
    /** Rows with a write of this page on its way: read again once it answers. */
    const skipped: string[] = [];
    for (const id of new Set([...shown.map((w) => w.id), ...stored.keys()])) {
      if (wanted && !wanted.has(id)) continue;
      if (seqs.busy(rowKey("workflow", id))) {
        skipped.push(id);
        continue;
      }
      if (!seqs.take(rowKey("workflow", id), seq)) continue;
      const workflow = stored.get(id);
      if (workflow) {
        next = next.some((w) => w.id === id)
          ? next.map((w) => (w.id === id ? workflow : w))
          : [...next, workflow];
      } else {
        next = next.filter((w) => w.id !== id);
        if (this.workflowState.activeWorkflowId === id) this.workflowState.activeWorkflowId = null;
      }
    }
    this.state.savedWorkflowsByProject = {
      ...this.state.savedWorkflowsByProject,
      [projectId]: next,
    };
    if (again && skipped.length > 0) {
      await Promise.all(skipped.map((id) => seqs.settled(rowKey("workflow", id))));
      await this.refreshFromLibrary(projectId, skipped, { again: false });
    }
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
