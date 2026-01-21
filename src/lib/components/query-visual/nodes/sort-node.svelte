<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import { ArrowUpDownIcon, ArrowUpIcon, ArrowDownIcon } from "@lucide/svelte";
	import type { QueryOrderBy } from "$lib/types";

	type Props = {
		data: {
			orderBy: QueryOrderBy[];
		};
		sourcePosition?: Position;
		targetPosition?: Position;
	};

	let { data, sourcePosition = Position.Bottom, targetPosition = Position.Top }: Props = $props();
</script>

<div
	class="min-w-40 rounded-lg border-2 border-indigo-500 bg-background shadow-md"
>
	<Handle type="target" position={targetPosition} class="!bg-indigo-500" />

	<div class="flex items-center gap-2 px-3 py-2 bg-indigo-500/10 border-b border-indigo-500/20 rounded-t-md">
		<ArrowUpDownIcon class="size-4 text-indigo-500" />
		<span class="text-sm font-semibold text-indigo-700 dark:text-indigo-300">
			ORDER BY
		</span>
	</div>
	<div class="px-3 py-2 space-y-1">
		{#each data.orderBy as order}
			<div class="flex items-center gap-1.5 text-xs">
				{#if order.direction === 'ASC'}
					<ArrowUpIcon class="size-3 text-indigo-500 shrink-0" />
				{:else}
					<ArrowDownIcon class="size-3 text-indigo-500 shrink-0" />
				{/if}
				<span class="font-mono bg-muted/50 px-1.5 py-0.5 rounded truncate max-w-40" title={order.expression}>
					{order.expression}
				</span>
				<span class="text-muted-foreground">{order.direction}</span>
			</div>
		{/each}
	</div>

	<Handle type="source" position={sourcePosition} class="!bg-indigo-500" />
</div>
