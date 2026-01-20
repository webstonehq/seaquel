<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import { GroupIcon } from "@lucide/svelte";

	type Props = {
		data: {
			columns: string[];
		};
	};

	let { data }: Props = $props();
</script>

<div
	class="min-w-40 rounded-lg border-2 border-teal-500 bg-background shadow-md"
>
	<Handle type="target" position={Position.Top} class="!bg-teal-500" />

	<div class="flex items-center gap-2 px-3 py-2 bg-teal-500/10 border-b border-teal-500/20 rounded-t-md">
		<GroupIcon class="size-4 text-teal-500" />
		<span class="text-sm font-semibold text-teal-700 dark:text-teal-300">
			GROUP BY
		</span>
	</div>
	<div class="px-3 py-2 space-y-1">
		{#each data.columns as column}
			<div class="text-xs font-mono bg-muted/50 px-2 py-0.5 rounded">
				{column}
			</div>
		{/each}
	</div>

	<Handle type="source" position={Position.Bottom} class="!bg-teal-500" />
</div>
