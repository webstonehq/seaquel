<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import { TableIcon, FileSearchIcon } from "@lucide/svelte";
	import type { QuerySource } from "$lib/types";

	type Props = {
		data: {
			source: QuerySource;
			isFirst: boolean;
		};
		sourcePosition?: Position;
		targetPosition?: Position;
	};

	let { data, sourcePosition = Position.Bottom, targetPosition = Position.Top }: Props = $props();
</script>

<div
	class="min-w-40 rounded-lg border-2 border-blue-500 bg-background shadow-md"
>
	<div class="flex items-center gap-2 px-3 py-2 bg-blue-500/10 border-b border-blue-500/20 rounded-t-md">
		{#if data.source.type === 'subquery'}
			<FileSearchIcon class="size-4 text-blue-500" />
		{:else}
			<TableIcon class="size-4 text-blue-500" />
		{/if}
		<span class="text-sm font-semibold text-blue-700 dark:text-blue-300">
			{data.source.type === 'subquery' ? 'Subquery' : 'Table'}
		</span>
	</div>
	<div class="px-3 py-2">
		<div class="text-sm font-medium">
			{#if data.source.schema}
				<span class="text-muted-foreground">{data.source.schema}.</span>
			{/if}
			{data.source.name}
		</div>
		{#if data.source.alias}
			<div class="text-xs text-muted-foreground mt-0.5">
				AS {data.source.alias}
			</div>
		{/if}
	</div>

	<!-- Output handle for connecting to next node -->
	<Handle type="source" position={sourcePosition} class="!bg-blue-500" />

	<!-- Input handle if not first node -->
	{#if !data.isFirst}
		<Handle type="target" position={targetPosition} class="!bg-blue-500" />
	{/if}
</div>
