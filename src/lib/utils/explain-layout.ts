import dagre from "@dagrejs/dagre";
import { Position } from "@xyflow/svelte";
import type { Node, Edge } from "@xyflow/svelte";
import type { ExplainResult, ExplainPlanNode } from "$lib/types";
import type { HotPathAnalysis } from "./explain-analysis";

const NODE_WIDTH = 280;
const NODE_HEIGHT = 180;

export function layoutExplainPlan(
  result: ExplainResult,
  analysis?: HotPathAnalysis
): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];

  // Recursively collect nodes and edges from the plan tree
  function collectNodesAndEdges(node: ExplainPlanNode, parentId?: string) {
    // Get analysis data for this node if available
    const nodeAnalysis = analysis?.nodeAnalysis.get(node.id);

    nodes.push({
      id: node.id,
      type: "planNode",
      position: { x: 0, y: 0 }, // Will be set by dagre
      data: {
        ...node,
        // Inject hot path analysis data
        tier: nodeAnalysis?.tier ?? "normal",
        percentageOfTotal: nodeAnalysis?.percentageOfTotal ?? 0,
        hasEstimationError: nodeAnalysis?.hasEstimationError ?? false,
        rowEstimationRatio: nodeAnalysis?.rowEstimationRatio ?? 1,
      } as unknown as Record<string, unknown>,
    });

    if (parentId) {
      // Check if this edge connects hot nodes (parent or child is critical/warning)
      const parentAnalysis = analysis?.nodeAnalysis.get(parentId);
      const isHotEdge =
        nodeAnalysis?.tier === "critical" ||
        nodeAnalysis?.tier === "warning" ||
        parentAnalysis?.tier === "critical" ||
        parentAnalysis?.tier === "warning";
      const isCriticalEdge =
        nodeAnalysis?.tier === "critical" || parentAnalysis?.tier === "critical";

      edges.push({
        id: `edge-${parentId}-${node.id}`,
        source: parentId,
        target: node.id,
        type: "smoothstep",
        animated: isCriticalEdge,
        style: isHotEdge
          ? isCriticalEdge
            ? "stroke: #ef4444; stroke-width: 3px;"
            : "stroke: #f97316; stroke-width: 2px;"
          : undefined,
      });
    }

    for (const child of node.children) {
      collectNodesAndEdges(child, node.id);
    }
  }

  collectNodesAndEdges(result.plan);

  // Use dagre for layout
  const dagreGraph = new dagre.graphlib.Graph();
  dagreGraph.setDefaultEdgeLabel(() => ({}));
  dagreGraph.setGraph({
    rankdir: "TB", // Top to bottom
    nodesep: 60,   // Horizontal separation
    ranksep: 100,  // Vertical separation
    marginx: 40,
    marginy: 40,
  });

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

  // Apply positions from dagre
  const layoutedNodes = nodes.map((node) => {
    const nodeWithPosition = dagreGraph.node(node.id);
    return {
      ...node,
      targetPosition: Position.Top,
      sourcePosition: Position.Bottom,
      position: {
        x: nodeWithPosition.x - NODE_WIDTH / 2,
        y: nodeWithPosition.y - NODE_HEIGHT / 2,
      },
    };
  });

  return { nodes: layoutedNodes, edges };
}
