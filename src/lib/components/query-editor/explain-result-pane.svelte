<script lang="ts">
	import { SvelteFlow, Background, Controls, MiniMap } from "@xyflow/svelte";
	import "@xyflow/svelte/dist/style.css";
	import { LoaderIcon, DatabaseIcon } from "@lucide/svelte";
	import ExplainPlanNode from "$lib/components/explain-plan-node.svelte";
	import { layoutExplainPlan } from "$lib/utils/explain-layout";
	import type { Node, Edge, NodeTypes, ColorMode } from "@xyflow/svelte";
	import type { EmbeddedExplainResult } from "$lib/types";
	import { mode } from "mode-watcher";
	import { m } from "$lib/paraglide/messages.js";

	type Props = {
		explainResult: EmbeddedExplainResult;
	};

	let { explainResult }: Props = $props();

	// Map mode-watcher theme to xyflow colorMode
	const colorMode: ColorMode = $derived(mode.current === "dark" ? "dark" : "light");

	// Custom node types
	const nodeTypes: NodeTypes = {
		planNode: ExplainPlanNode,
	};

	// Convert explain result to xyflow nodes and edges
	const flowData = $derived.by(() => {
		if (!explainResult.result) {
			return { nodes: [] as Node[], edges: [] as Edge[] };
		}
		return layoutExplainPlan(explainResult.result);
	});

	let nodes = $derived(flowData.nodes);
	let edges = $derived(flowData.edges);
</script>

<div class="flex-1 min-h-0">
	{#if explainResult.isExecuting}
		<!-- Loading state -->
		<div class="h-full flex items-center justify-center">
			<div class="flex flex-col items-center gap-3">
				<LoaderIcon class="size-8 animate-spin text-muted-foreground" />
				<p class="text-sm text-muted-foreground">{m.explain_analyzing()}</p>
			</div>
		</div>
	{:else if explainResult.result}
		<!-- Flow Diagram -->
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
			proOptions={{ hideAttribution: true }}
		>
			<Background />
			<Controls />
			<MiniMap />
		</SvelteFlow>
	{:else}
		<!-- Empty state -->
		<div class="h-full flex items-center justify-center text-muted-foreground">
			<div class="text-center">
				<DatabaseIcon class="size-12 mx-auto mb-2 opacity-20" />
				<p class="text-sm">{m.explain_no_plan_available()}</p>
			</div>
		</div>
	{/if}
</div>
