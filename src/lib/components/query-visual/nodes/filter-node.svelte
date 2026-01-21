<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import { FilterIcon } from "@lucide/svelte";
	import type { QueryFilter } from "$lib/types";

	type Props = {
		data: {
			filter: QueryFilter;
			filterType: 'WHERE' | 'HAVING';
		};
		sourcePosition?: Position;
		targetPosition?: Position;
	};

	let { data, sourcePosition = Position.Bottom, targetPosition = Position.Top }: Props = $props();
</script>

<div
	class="min-w-48 rounded-lg border-2 border-orange-500 bg-background shadow-md"
>
	<Handle type="target" position={targetPosition} class="!bg-orange-500" />

	<div class="flex items-center gap-2 px-3 py-2 bg-orange-500/10 border-b border-orange-500/20 rounded-t-md">
		<FilterIcon class="size-4 text-orange-500" />
		<span class="text-sm font-semibold text-orange-700 dark:text-orange-300">
			{data.filterType}
		</span>
	</div>
	<div class="px-3 py-2">
		<div class="text-xs font-mono bg-muted/50 px-2 py-1 rounded max-w-72 break-words">
			{data.filter.expression}
		</div>
	</div>

	<Handle type="source" position={sourcePosition} class="!bg-orange-500" />
</div>
