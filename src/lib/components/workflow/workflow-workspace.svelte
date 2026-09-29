<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { SvelteFlow, Background, Controls, MiniMap, type ColorMode, type Node, type Connection } from "@xyflow/svelte";
	import "@xyflow/svelte/dist/style.css";
	import { mode } from "mode-watcher";
	import { workflowNodeTypes } from "./nodes";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import MousePointerIcon from "@lucide/svelte/icons/mouse-pointer";
	import type { WorkflowNodeData } from "$lib/types/workflow";
	import { m } from "$lib/paraglide/messages.js";

	const db = useDatabase();

	// Map mode-watcher theme to xyflow colorMode
	const colorMode: ColorMode = $derived(mode.current === "dark" ? "dark" : "light");

	// Check if connected
	const isConnected = $derived(!!db.state.activeConnectionId);

	// Handle node drag stop to sync position back to state
	function handleNodeDragStop({ targetNode, nodes }: { targetNode: Node<WorkflowNodeData> | null; nodes: Node<WorkflowNodeData>[]; event: MouseEvent | TouchEvent }) {
		if (targetNode) {
			// Update the node position in our state
			db.workflowState.nodes = db.workflowState.nodes.map((n) =>
				n.id === targetNode.id ? { ...n, position: targetNode.position } : n
			);
		}
	}

	// Handle new connections
	function handleConnect(connection: Connection) {
		if (connection.source && connection.target) {
			db.workflow.connect(
				connection.source,
				connection.target,
				connection.sourceHandle ?? undefined,
				connection.targetHandle ?? undefined
			);
		}
	}
</script>

<div class="flex-1 min-h-0 w-full relative">
	{#if isConnected}
		<SvelteFlow
			nodes={db.workflowState.nodes}
			edges={db.workflowState.edges}
			nodeTypes={workflowNodeTypes}
			{colorMode}
			fitView
			minZoom={0.1}
			maxZoom={2}
			nodesDraggable={true}
			nodesConnectable={true}
			elementsSelectable={true}
			deleteKey={null}
			onnodedragstop={handleNodeDragStop}
			onconnect={handleConnect}
			proOptions={{ hideAttribution: true }}
			panOnScroll={true}
			snapGrid={[20, 20]}
			zoomOnScroll={false}
		>
			<Background />
			<Controls />
			<MiniMap />
		</SvelteFlow>

		<!-- Empty state overlay -->
		{#if db.workflowState.nodes.length === 0}
			<div class="absolute inset-0 flex items-center justify-center pointer-events-none">
				<div class="text-center text-muted-foreground">
					<MousePointerIcon class="size-12 mx-auto mb-3 opacity-20" />
					<p class="text-sm">{m.workflow_add_table_hint()}</p>
					<p class="text-xs mt-1 opacity-70">{m.workflow_add_query_hint()}</p>
				</div>
			</div>
		{/if}
	{:else}
		<div class="h-full flex items-center justify-center text-muted-foreground">
			<div class="text-center">
				<DatabaseIcon class="size-12 mx-auto mb-3 opacity-20" />
				<p class="text-sm">{m.workflow_connect_hint()}</p>
				<p class="text-xs mt-1 opacity-70">{m.workflow_select_connection_hint()}</p>
			</div>
		</div>
	{/if}
</div>
