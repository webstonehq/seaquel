import type { Node, Edge } from "@xyflow/svelte";
import type { SchemaTable } from "$lib/types";

interface SvgExportOptions {
  theme: "light" | "dark";
  padding?: number;
}

interface ThemeColors {
  background: string;
  headerBg: string;
  headerText: string;
  bodyBg: string;
  bodyBorder: string;
  columnText: string;
  mutedText: string;
  edgeStroke: string;
  pkColor: string;
  fkColor: string;
}

const THEMES: Record<"light" | "dark", ThemeColors> = {
  light: {
    background: "#ffffff",
    headerBg: "#171717",
    headerText: "#fafafa",
    bodyBg: "#ffffff",
    bodyBorder: "#e5e5e5",
    columnText: "#171717",
    mutedText: "#737373",
    edgeStroke: "#a3a3a3",
    pkColor: "#f59e0b",
    fkColor: "#3b82f6",
  },
  dark: {
    background: "#0a0a0a",
    headerBg: "#fafafa",
    headerText: "#171717",
    bodyBg: "#0a0a0a",
    bodyBorder: "#262626",
    columnText: "#fafafa",
    mutedText: "#a3a3a3",
    edgeStroke: "#525252",
    pkColor: "#f59e0b",
    fkColor: "#3b82f6",
  },
};

const NODE_WIDTH = 250;
const HEADER_HEIGHT = 32;
const COLUMN_HEIGHT = 24;
const MAX_VISIBLE_COLUMNS = 15;
const FONT_FAMILY = "system-ui, -apple-system, sans-serif";
const MONO_FONT = "ui-monospace, monospace";

/**
 * Escapes special characters for XML/SVG content.
 */
function escapeXml(str: string): string {
  return str
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&apos;");
}

/**
 * Generates a cubic bezier path for an edge (smoothstep approximation).
 */
function generateEdgePath(
  sourceX: number,
  sourceY: number,
  targetX: number,
  targetY: number
): string {
  const midX = (sourceX + targetX) / 2;
  return `M ${sourceX} ${sourceY} C ${midX} ${sourceY}, ${midX} ${targetY}, ${targetX} ${targetY}`;
}

/**
 * Abbreviates data types for display (mirrors erd-table-node.svelte logic).
 */
function abbreviateType(type: string): string {
  const abbrevMap: Record<string, string> = {
    "character varying": "varchar",
    "timestamp without time zone": "timestamp",
    "timestamp with time zone": "timestamptz",
    "double precision": "double",
    boolean: "bool",
    integer: "int",
  };

  const lowerType = type.toLowerCase();
  for (const [full, abbrev] of Object.entries(abbrevMap)) {
    if (lowerType.startsWith(full)) {
      return abbrev;
    }
  }

  if (type.length > 12) {
    return type.substring(0, 10) + "...";
  }
  return type;
}

/**
 * Generates a clean, lightweight SVG string from ERD nodes and edges.
 */
export function generateErdSvg(
  nodes: Node[],
  edges: Edge[],
  options: SvgExportOptions
): string {
  const { theme, padding = 40 } = options;
  const colors = THEMES[theme];

  // Calculate bounding box
  let minX = Infinity,
    minY = Infinity,
    maxX = -Infinity,
    maxY = -Infinity;

  for (const node of nodes) {
    const { x, y } = node.position;
    const data = node.data as { table: SchemaTable; width: number; height: number };
    minX = Math.min(minX, x);
    minY = Math.min(minY, y);
    maxX = Math.max(maxX, x + data.width);
    maxY = Math.max(maxY, y + data.height);
  }

  // Handle empty case
  if (nodes.length === 0) {
    return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text x="50" y="50" text-anchor="middle">No tables</text></svg>`;
  }

  const width = maxX - minX + padding * 2;
  const height = maxY - minY + padding * 2;
  const offsetX = -minX + padding;
  const offsetY = -minY + padding;

  // Build node lookup for edge calculations
  const nodeMap = new Map<string, { x: number; y: number; height: number }>();
  for (const node of nodes) {
    const data = node.data as { table: SchemaTable; height: number };
    nodeMap.set(node.id, {
      x: node.position.x + offsetX,
      y: node.position.y + offsetY,
      height: data.height,
    });
  }

  // Generate edges SVG
  const edgesSvg = edges
    .map((edge) => {
      const source = nodeMap.get(edge.source);
      const target = nodeMap.get(edge.target);
      if (!source || !target) return "";

      const sourceX = source.x + NODE_WIDTH;
      const sourceY = source.y + source.height / 2;
      const targetX = target.x;
      const targetY = target.y + target.height / 2;

      const path = generateEdgePath(sourceX, sourceY, targetX, targetY);
      return `<path d="${path}" fill="none" stroke="${colors.edgeStroke}" stroke-width="2"/>`;
    })
    .join("\n    ");

  // Generate nodes SVG
  const nodesSvg = nodes
    .map((node) => {
      const data = node.data as { table: SchemaTable; width: number; height: number };
      const { table } = data;
      const x = node.position.x + offsetX;
      const y = node.position.y + offsetY;

      const visibleColumns = table.columns.slice(0, MAX_VISIBLE_COLUMNS);
      const hiddenCount = table.columns.length - MAX_VISIBLE_COLUMNS;
      const bodyHeight = data.height - HEADER_HEIGHT;

      // Column rows
      const columnRows = visibleColumns
        .map((col, i) => {
          const colY = HEADER_HEIGHT + 4 + i * COLUMN_HEIGHT + 16;
          const icons: string[] = [];
          let iconOffset = 8;

          if (col.isPrimaryKey) {
            icons.push(
              `<text x="${iconOffset}" y="${colY}" fill="${colors.pkColor}" font-size="10">PK</text>`
            );
            iconOffset += 20;
          }
          if (col.isForeignKey) {
            icons.push(
              `<text x="${iconOffset}" y="${colY}" fill="${colors.fkColor}" font-size="10">FK</text>`
            );
            iconOffset += 20;
          }

          const nameX = 40;
          const typeX = NODE_WIDTH - 8;
          const nullable = col.nullable ? "?" : "";

          return `
      ${icons.join("")}
      <text x="${nameX}" y="${colY}" fill="${colors.columnText}" font-family="${MONO_FONT}" font-size="11">${escapeXml(col.name)}</text>
      <text x="${typeX}" y="${colY}" fill="${colors.mutedText}" font-family="${MONO_FONT}" font-size="11" text-anchor="end">${escapeXml(abbreviateType(col.type))}${nullable}</text>`;
        })
        .join("");

      // Hidden count row
      const hiddenRow =
        hiddenCount > 0
          ? `<text x="${NODE_WIDTH / 2}" y="${HEADER_HEIGHT + visibleColumns.length * COLUMN_HEIGHT + 20}" fill="${colors.mutedText}" font-size="11" text-anchor="middle" font-style="italic">+${hiddenCount} more columns</text>`
          : "";

      return `
  <g transform="translate(${x}, ${y})">
    <!-- Table box -->
    <rect x="0" y="0" width="${NODE_WIDTH}" height="${data.height}" rx="6" fill="${colors.bodyBg}" stroke="${colors.bodyBorder}" stroke-width="2"/>
    <!-- Header -->
    <rect x="0" y="0" width="${NODE_WIDTH}" height="${HEADER_HEIGHT}" rx="6" fill="${colors.headerBg}"/>
    <rect x="0" y="${HEADER_HEIGHT - 6}" width="${NODE_WIDTH}" height="6" fill="${colors.headerBg}"/>
    <!-- Header text -->
    <text x="12" y="21" fill="${colors.headerText}" font-family="${FONT_FAMILY}" font-size="13" font-weight="500">${escapeXml(table.name)}</text>
    <text x="${NODE_WIDTH - 12}" y="21" fill="${colors.headerText}" font-family="${FONT_FAMILY}" font-size="11" text-anchor="end" opacity="0.7">${escapeXml(table.schema)}</text>
    <!-- Columns -->
    ${columnRows}
    ${hiddenRow}
  </g>`;
    })
    .join("\n");

  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${width} ${height}" width="${width}" height="${height}">
  <rect x="0" y="0" width="${width}" height="${height}" fill="${colors.background}"/>
  <g>
    ${edgesSvg}
  </g>
  <g>
    ${nodesSvg}
  </g>
</svg>`;
}
