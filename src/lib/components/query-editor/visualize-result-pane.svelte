<script lang="ts">
	import { SvelteFlow, Background, Controls, MiniMap } from "@xyflow/svelte";
	import "@xyflow/svelte/dist/style.css";
	import { AlertCircleIcon, NetworkIcon } from "@lucide/svelte";
	import {
		TableSourceNode,
		JoinNode,
		FilterNode,
		GroupNode,
		ProjectionNode,
		SortNode,
		LimitNode
	} from "$lib/components/query-visual/nodes";
	import { layoutQueryVisualization, type QueryLayoutOptions } from "$lib/utils/query-visual-layout";
	import type { Node, Edge, NodeTypes, ColorMode } from "@xyflow/svelte";
	import type { EmbeddedVisualizeResult } from "$lib/types";
	import { mode } from "mode-watcher";

	type Props = {
		visualizeResult: EmbeddedVisualizeResult;
		layoutOptions: QueryLayoutOptions;
	};

	let { visualizeResult, layoutOptions }: Props = $props();

	// Map mode-watcher theme to xyflow colorMode
	const colorMode: ColorMode = $derived(mode.current === "dark" ? "dark" : "light");

	// Custom node types
	const nodeTypes: NodeTypes = {
		tableSourceNode: TableSourceNode,
		joinNode: JoinNode,
		filterNode: FilterNode,
		groupNode: GroupNode,
		projectionNode: ProjectionNode,
		sortNode: SortNode,
		limitNode: LimitNode
	};

	// Compute flow data from visualize result
	const flowData = $derived.by(() => {
		if (!visualizeResult.parsedQuery) {
			return { nodes: [] as Node[], edges: [] as Edge[] };
		}
		return layoutQueryVisualization(visualizeResult.parsedQuery, layoutOptions);
	});

	let nodes = $derived(flowData.nodes);
	let edges = $derived(flowData.edges);
</script>

<div class="flex-1 min-h-0">
	{#if visualizeResult.parsedQuery}
		<!-- Flow Diagram -->
		{#if nodes.length > 0}
			<SvelteFlow
				{nodes}
				{edges}
				{nodeTypes}
				{colorMode}
				fitView
				minZoom={0.1}
				maxZoom={2}
				nodesDraggable={true}
				nodesConnectable={false}
				elementsSelectable={true}
				deleteKey={null}
				proOptions={{ hideAttribution: true }}
			>
				<Background />
				<Controls />
				<MiniMap />
			</SvelteFlow>
		{:else}
			<div class="h-full flex items-center justify-center text-muted-foreground">
				<div class="text-center">
					<NetworkIcon class="size-12 mx-auto mb-2 opacity-20" />
					<p class="text-sm">Unable to visualize query structure</p>
				</div>
			</div>
		{/if}
	{:else if visualizeResult.parseError}
		<!-- Parse error state -->
		<div class="h-full flex items-center justify-center text-muted-foreground p-4">
			<div class="text-center max-w-md">
				<AlertCircleIcon class="size-12 mx-auto mb-4 text-destructive opacity-50" />
				<h3 class="text-lg font-semibold mb-2">Could not parse query</h3>
				<p class="text-sm mb-4">{visualizeResult.parseError}</p>
				<div class="bg-muted p-3 rounded-md">
					<code class="text-xs font-mono break-all text-left block">
						{visualizeResult.sourceQuery}
					</code>
				</div>
			</div>
		</div>
	{/if}
</div>
