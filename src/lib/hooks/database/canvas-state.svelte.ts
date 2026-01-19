import type { Node, Edge, Viewport } from "@xyflow/svelte";
import type {
	CanvasTimelineEntry,
	CanvasNodeData,
} from "$lib/types/canvas";

/**
 * Canvas state - manages the state of the canvas view
 * Follows the Svelte 5 runes pattern used by DatabaseState
 */
export class CanvasState {
	// Current canvas nodes and edges
	nodes = $state<Node<CanvasNodeData>[]>([]);
	edges = $state<Edge[]>([]);

	// Timeline entries for current session
	timeline = $state<CanvasTimelineEntry[]>([]);

	// Active canvas ID (null = unsaved canvas)
	activeCanvasId = $state<string | null>(null);

	// Viewport state
	viewport = $state<Viewport>({ x: 0, y: 0, zoom: 1 });

	// Track which connection is active for the canvas
	activeConnectionId = $state<string | null>(null);

	// Check if there are unsaved changes
	hasUnsavedChanges = $derived(
		this.nodes.length > 0 || this.edges.length > 0
	);

	// Get node by ID
	getNode(nodeId: string): Node<CanvasNodeData> | undefined {
		return this.nodes.find((n) => n.id === nodeId);
	}

	// Get edges connected to a node
	getConnectedEdges(nodeId: string): Edge[] {
		return this.edges.filter(
			(e) => e.source === nodeId || e.target === nodeId
		);
	}
}
