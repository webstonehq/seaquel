<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import { MergeIcon } from "@lucide/svelte";
	import type { QueryJoin } from "$lib/types";

	type Props = {
		data: {
			join: QueryJoin;
		};
	};

	let { data }: Props = $props();

	const joinColors: Record<string, { border: string; bg: string; text: string }> = {
		INNER: { border: 'border-violet-500', bg: 'bg-violet-500/10', text: 'text-violet-500' },
		LEFT: { border: 'border-amber-500', bg: 'bg-amber-500/10', text: 'text-amber-500' },
		RIGHT: { border: 'border-orange-500', bg: 'bg-orange-500/10', text: 'text-orange-500' },
		FULL: { border: 'border-emerald-500', bg: 'bg-emerald-500/10', text: 'text-emerald-500' },
		CROSS: { border: 'border-rose-500', bg: 'bg-rose-500/10', text: 'text-rose-500' }
	};

	const colors = $derived(joinColors[data.join.type] || joinColors.INNER);
</script>

<div class="min-w-48 rounded-lg border-2 bg-background shadow-md {colors.border}">
	<!-- Input handles (left side for sources) -->
	<Handle type="target" position={Position.Top} id="left" style="left: 30%;" class="!bg-violet-500" />
	<Handle type="target" position={Position.Top} id="right" style="left: 70%;" class="!bg-violet-500" />

	<div class="flex items-center gap-2 px-3 py-2 border-b rounded-t-md {colors.bg}">
		<MergeIcon class="size-4 {colors.text}" />
		<span class="text-sm font-semibold {colors.text}">
			{data.join.type} JOIN
		</span>
	</div>
	<div class="px-3 py-2">
		<!-- Joined table -->
		<div class="text-sm font-medium mb-1">
			{#if data.join.source.schema}
				<span class="text-muted-foreground">{data.join.source.schema}.</span>
			{/if}
			{data.join.source.name}
			{#if data.join.source.alias}
				<span class="text-muted-foreground"> AS {data.join.source.alias}</span>
			{/if}
		</div>
		<!-- ON condition -->
		{#if data.join.condition}
			<div class="text-xs text-muted-foreground font-mono bg-muted/50 px-2 py-1 rounded mt-1 max-w-64 break-words">
				ON {data.join.condition}
			</div>
		{/if}
	</div>

	<!-- Output handle -->
	<Handle type="source" position={Position.Bottom} class="!bg-violet-500" />
</div>
