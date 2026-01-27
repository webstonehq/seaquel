<script lang="ts">
	import { TUTORIAL_SCHEMA } from '$lib/tutorial/schema';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import { TableIcon, GripVerticalIcon, CheckIcon } from '@lucide/svelte';
    import { useDnD } from './dnd-provider.svelte';

	const qb = useQueryBuilder();
	const type = useDnD();

	// Track which tables are already on the canvas
	const tablesOnCanvas = $derived(new Set(qb.tables.map((t) => t.tableName)));

	function handleDragStart(event: DragEvent, tableName: string) {
		if (!event.dataTransfer) return;
		console.log("jjj");
		// Store the table name in shared context (more reliable than dataTransfer)
		type.current = tableName;
		// setData is required for the drag operation to be valid in some browsers
		event.dataTransfer.setData('text/plain', tableName);
		event.dataTransfer.effectAllowed = 'move';
	}
</script>

<div class="flex-1 flex flex-col bg-muted/30">
	<!-- Header -->
	<div class="p-3 border-b border-border">
		<h3 class="font-medium text-sm">Tables</h3>
		<p class="text-xs text-muted-foreground">Drag onto canvas</p>
	</div>

	<!-- Table List -->
	<ScrollArea class="flex-1">
		<div class="p-2 space-y-0.5" role="list">
			{#each TUTORIAL_SCHEMA as table (table.name)}
				{@const isOnCanvas = tablesOnCanvas.has(table.name)}
				<div
					role="listitem"
					class="flex items-center gap-2 px-2 py-1.5 rounded text-sm {isOnCanvas
						? 'opacity-50 cursor-default'
						: 'cursor-grab hover:bg-muted'}"
					draggable={!isOnCanvas}
					ondragstart={(e) => !isOnCanvas && handleDragStart(e, table.name)}
				>
					<!-- Drag handle or check icon -->
					{#if isOnCanvas}
						<CheckIcon class="size-4 text-green-500 shrink-0" />
					{:else}
						<GripVerticalIcon class="size-4 text-muted-foreground shrink-0" />
					{/if}

					<!-- Table icon -->
					<TableIcon class="size-4 text-muted-foreground shrink-0" />

					<!-- Table name -->
					<span class="flex-1 truncate" title={table.name}>
						{table.name}
					</span>

					<!-- Column count -->
					<span class="text-xs text-muted-foreground shrink-0">
						{table.columns.length}
					</span>
				</div>
			{/each}
		</div>
	</ScrollArea>
</div>
