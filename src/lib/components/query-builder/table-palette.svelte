<script lang="ts">
	import { TUTORIAL_SCHEMA } from '$lib/tutorial/schema';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import { TableIcon, GripVerticalIcon, BracesIcon, LayersIcon } from '@lucide/svelte';
	import { useDnD } from './dnd-provider.svelte';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte.js';
	import type { QueryBuilderTable } from '$lib/types';
	import { tutorialToQueryBuilder } from '$lib/utils/schema-adapter';
	import { m } from '$lib/paraglide/messages.js';

	interface Props {
		/** Optional schema to use. Defaults to TUTORIAL_SCHEMA converted to QueryBuilderTable format. */
		schema?: QueryBuilderTable[];
		/** Whether to show subquery and CTE options. Defaults to true for tutorial, false for manage. */
		showAdvancedOptions?: boolean;
	}

	const { schema, showAdvancedOptions = true }: Props = $props();

	// Use provided schema or convert tutorial schema as default
	const tables = $derived(schema ?? tutorialToQueryBuilder(TUTORIAL_SCHEMA));

	const type = useDnD();
	const qb = useQueryBuilder();

	function handleDragStart(event: DragEvent, dragType: string) {
		if (!event.dataTransfer) return;
		// Store the drag type in shared context (more reliable than dataTransfer)
		type.current = dragType;
		// setData is required for the drag operation to be valid in some browsers
		event.dataTransfer.setData('text/plain', dragType);
		event.dataTransfer.effectAllowed = 'move';
	}
</script>

<div class="h-full flex flex-col bg-muted/30">
	<!-- Header -->
	<div class="p-3 border-b border-border">
		<h3 class="font-medium text-sm">{m.table_palette_title()}</h3>
		<p class="text-xs text-muted-foreground">{m.table_palette_hint()}</p>
	</div>

	<!-- Table List -->
	<ScrollArea class="flex-1">
		<div class="p-2 space-y-0.5" role="list">
			{#if tables.length === 0}
				<div class="px-2 py-4 text-sm text-muted-foreground text-center">
					{m.table_palette_no_tables()}
				</div>
			{/if}
			{#each tables as table (table.name)}
				<div
					role="listitem"
					class="flex items-center gap-2 px-2 py-1.5 rounded text-sm cursor-grab hover:bg-muted"
					draggable={true}
					ondragstart={(e) => handleDragStart(e, table.name)}
				>
					<!-- Drag handle -->
					<GripVerticalIcon class="size-4 text-muted-foreground shrink-0" />

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

			{#if showAdvancedOptions}
				<!-- Divider -->
				<div class="my-2 border-t border-border"></div>

				<!-- Subquery -->
				<div
					role="listitem"
					class="flex items-center gap-2 px-2 py-1.5 rounded text-sm cursor-grab hover:bg-indigo-500/10 border border-dashed border-indigo-500/30"
					draggable={true}
					ondragstart={(e) => handleDragStart(e, '__subquery__')}
				>
					<GripVerticalIcon class="size-4 text-indigo-500/70 shrink-0" />
					<BracesIcon class="size-4 text-indigo-500 shrink-0" />
					<span class="flex-1 truncate text-indigo-600 dark:text-indigo-400">
						{m.table_palette_subquery()}
					</span>
					<span class="text-xs text-muted-foreground shrink-0">
						{m.table_palette_nested()}
					</span>
				</div>

				<!-- CTE -->
				<div
					role="listitem"
					class="flex items-center gap-2 px-2 py-1.5 rounded text-sm cursor-grab hover:bg-violet-500/10 border border-dashed border-violet-500/30"
					draggable={true}
					ondragstart={(e) => handleDragStart(e, '__cte__')}
				>
					<GripVerticalIcon class="size-4 text-violet-500/70 shrink-0" />
					<LayersIcon class="size-4 text-violet-500 shrink-0" />
					<span class="flex-1 truncate text-violet-600 dark:text-violet-400">
						{m.table_palette_cte()}
					</span>
					<span class="text-xs text-muted-foreground shrink-0">
						{m.table_palette_reusable()}
					</span>
				</div>

				<!-- CTEs Section - show defined CTEs that can be referenced -->
				{#if qb.ctes.length > 0}
					<div class="my-2 border-t border-border"></div>
					<div class="px-2 py-1 text-xs text-muted-foreground font-medium">{m.table_palette_defined_ctes()}</div>
					{#each qb.ctes as cte (cte.id)}
						{#if cte.name}
							<div
								role="listitem"
								class="flex items-center gap-2 px-2 py-1.5 rounded text-sm cursor-grab hover:bg-violet-500/10"
								draggable={true}
								ondragstart={(e) => handleDragStart(e, `__cte__${cte.id}`)}
							>
								<GripVerticalIcon class="size-4 text-muted-foreground shrink-0" />
								<LayersIcon class="size-4 text-violet-500 shrink-0" />
								<span class="flex-1 truncate font-mono text-violet-600 dark:text-violet-400">
									{cte.name}
								</span>
								<span class="text-xs text-muted-foreground shrink-0">
									{cte.innerQuery.tables.length}
								</span>
							</div>
						{/if}
					{/each}
				{/if}
			{/if}
		</div>
	</ScrollArea>
</div>
