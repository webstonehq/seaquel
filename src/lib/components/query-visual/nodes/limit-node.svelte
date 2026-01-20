<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import { ListEndIcon } from "@lucide/svelte";

	type Props = {
		data: {
			limit: { count: number; offset?: number };
		};
	};

	let { data }: Props = $props();
</script>

<div
	class="min-w-32 rounded-lg border-2 border-pink-500 bg-background shadow-md"
>
	<Handle type="target" position={Position.Top} class="!bg-pink-500" />

	<div class="flex items-center gap-2 px-3 py-2 bg-pink-500/10 border-b border-pink-500/20 rounded-t-md">
		<ListEndIcon class="size-4 text-pink-500" />
		<span class="text-sm font-semibold text-pink-700 dark:text-pink-300">
			LIMIT
		</span>
	</div>
	<div class="px-3 py-2">
		<div class="text-sm font-mono">
			{data.limit.count}
			{#if data.limit.offset !== undefined && data.limit.offset > 0}
				<span class="text-muted-foreground"> OFFSET {data.limit.offset}</span>
			{/if}
		</div>
	</div>
</div>
