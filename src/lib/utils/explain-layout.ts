import dagre from "@dagrejs/dagre";
import { Position } from "@xyflow/svelte";
import type { Node, Edge } from "@xyflow/svelte";
import type { ExplainResult, ExplainPlanNode } from "$lib/types";

const NODE_WIDTH = 280;
const NODE_HEIGHT = 180;

export function layoutExplainPlan(result: ExplainResult): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];

  // Recursively collect nodes and edges from the plan tree
  function collectNodesAndEdges(node: ExplainPlanNode, parentId?: string) {
    nodes.push({
      id: node.id,
      type: "planNode",
      position: { x: 0, y: 0 }, // Will be set by dagre
      data: node as unknown as Record<string, unknown>,
    });

    if (parentId) {
      edges.push({
        id: `edge-${parentId}-${node.id}`,
        source: parentId,
        target: node.id,
        type: "smoothstep",
        animated: false,
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
