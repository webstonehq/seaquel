import dagre from "@dagrejs/dagre";
import { Position } from "@xyflow/svelte";
import type { Node, Edge } from "@xyflow/svelte";
import type { SchemaTable } from "$lib/types";

const NODE_WIDTH = 250;
const NODE_HEIGHT_BASE = 60; // Header height
const COLUMN_HEIGHT = 24; // Height per column row

/**
 * Calculates the height of a table node based on the number of columns.
 * Caps at a maximum height to prevent overly tall nodes.
 */
function calculateNodeHeight(columnCount: number): number {
  const maxVisibleColumns = 15;
  const visibleColumns = Math.min(columnCount, maxVisibleColumns);
  return NODE_HEIGHT_BASE + (visibleColumns * COLUMN_HEIGHT);
}

/**
 * Converts schema tables to xyflow nodes and edges for ERD visualization.
 * Uses Dagre for automatic graph layout.
 */
export function layoutErdDiagram(tables: SchemaTable[]): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];

  // Create a map for quick table lookup by schema.name
  const tableMap = new Map<string, SchemaTable>();
  for (const table of tables) {
    const key = `${table.schema}.${table.name}`;
    tableMap.set(key, table);
  }

  // Create nodes for each table
  for (const table of tables) {
    const nodeId = `${table.schema}.${table.name}`;
    const nodeHeight = calculateNodeHeight(table.columns.length);

    nodes.push({
      id: nodeId,
      type: "erdTableNode",
      position: { x: 0, y: 0 }, // Will be set by dagre
      data: {
        table,
        width: NODE_WIDTH,
        height: nodeHeight,
      },
    });

    // Create edges for foreign key relationships
    for (const column of table.columns) {
      if (column.isForeignKey && column.foreignKeyRef) {
        const targetKey = `${column.foreignKeyRef.referencedSchema}.${column.foreignKeyRef.referencedTable}`;

        // Only create edge if target table exists in our schema
        if (tableMap.has(targetKey)) {
          edges.push({
            id: `edge-${nodeId}-${column.name}-${targetKey}`,
            source: nodeId,
            target: targetKey,
            type: "smoothstep",
            animated: false,
            style: "stroke-width: 2px; stroke: #737373;",
            label: `${column.name} → ${column.foreignKeyRef.referencedColumn}`,
            labelStyle: "font-size: 10px; fill: #737373;",
          });
        }
      }
    }
  }

  // Use dagre for layout
  const dagreGraph = new dagre.graphlib.Graph();
  dagreGraph.setDefaultEdgeLabel(() => ({}));
  dagreGraph.setGraph({
    rankdir: "LR", // Left to right for ERD
    nodesep: 80,   // Horizontal separation
    ranksep: 120,  // Vertical separation
    marginx: 40,
    marginy: 40,
  });

  // Add nodes to dagre with their calculated heights
  for (const node of nodes) {
    const height = (node.data as { height: number }).height;
    dagreGraph.setNode(node.id, { width: NODE_WIDTH, height });
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
    const height = (node.data as { height: number }).height;
    return {
      ...node,
      targetPosition: Position.Left,
      sourcePosition: Position.Right,
      position: {
        x: nodeWithPosition.x - NODE_WIDTH / 2,
        y: nodeWithPosition.y - height / 2,
      },
    };
  });

  return { nodes: layoutedNodes, edges };
}
