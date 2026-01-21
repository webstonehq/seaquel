<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import { ListIcon, FunctionSquareIcon } from "@lucide/svelte";
	import type { QueryProjection } from "$lib/types";

	type Props = {
		data: {
			projections: QueryProjection[];
			distinct: boolean;
		};
		sourcePosition?: Position;
		targetPosition?: Position;
	};

	let { data, sourcePosition = Position.Bottom, targetPosition = Position.Top }: Props = $props();

	const maxDisplayed = 8;
	const displayedProjections = $derived(data.projections.slice(0, maxDisplayed));
	const remainingCount = $derived(Math.max(0, data.projections.length - maxDisplayed));
</script>

<div
	class="min-w-48 rounded-lg border-2 border-slate-500 bg-background shadow-md"
>
	<Handle type="target" position={targetPosition} class="!bg-slate-500" />

	<div class="flex items-center gap-2 px-3 py-2 bg-slate-500/10 border-b border-slate-500/20 rounded-t-md">
		<ListIcon class="size-4 text-slate-500" />
		<span class="text-sm font-semibold text-slate-700 dark:text-slate-300">
			SELECT
			{#if data.distinct}
				<span class="text-xs font-normal ml-1 text-muted-foreground">DISTINCT</span>
			{/if}
		</span>
	</div>
	<div class="px-3 py-2 space-y-1 max-h-48 overflow-y-auto">
		{#each displayedProjections as proj}
			<div class="flex items-center gap-1.5 text-xs">
				{#if proj.isAggregate}
					<FunctionSquareIcon class="size-3 text-green-500 shrink-0" />
				{/if}
				<span
					class="font-mono bg-muted/50 px-1.5 py-0.5 rounded truncate max-w-48"
					class:text-green-700={proj.isAggregate}
					class:dark:text-green-300={proj.isAggregate}
					title={proj.expression + (proj.alias ? ` AS ${proj.alias}` : '')}
				>
					{proj.expression}
					{#if proj.alias}
						<span class="text-muted-foreground"> AS {proj.alias}</span>
					{/if}
				</span>
			</div>
		{/each}
		{#if remainingCount > 0}
			<div class="text-xs text-muted-foreground italic">
				+{remainingCount} more column{remainingCount > 1 ? 's' : ''}
			</div>
		{/if}
	</div>

	<Handle type="source" position={sourcePosition} class="!bg-slate-500" />
</div>
