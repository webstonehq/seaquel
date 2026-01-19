import type { NodeTypes } from "@xyflow/svelte";
import CanvasTableNode from "./table-node.svelte";
import CanvasQueryNode from "./query-node.svelte";
import CanvasResultNode from "./result-node.svelte";
import CanvasChartNode from "./chart-node.svelte";

export { CanvasTableNode, CanvasQueryNode, CanvasResultNode, CanvasChartNode };

export const canvasNodeTypes: NodeTypes = {
	tableNode: CanvasTableNode,
	queryNode: CanvasQueryNode,
	resultNode: CanvasResultNode,
	chartNode: CanvasChartNode,
};
