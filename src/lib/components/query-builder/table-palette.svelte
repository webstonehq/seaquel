<script lang="ts">
	import { TUTORIAL_SCHEMA } from '$lib/tutorial/schema';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import TableIcon from '@lucide/svelte/icons/table';
	import GripVerticalIcon from '@lucide/svelte/icons/grip-vertical';
	import BracesIcon from '@lucide/svelte/icons/braces';
	import LayersIcon from '@lucide/svelte/icons/layers';
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

	const dnd = useDnD();
	const qb = useQueryBuilder();

	let ghost: HTMLElement | null = null;

	function handlePointerDown(event: PointerEvent, dragType: string) {
		if (event.button !== 0) return;

		const sourceEl = event.currentTarget as HTMLElement;
		dnd.current = dragType;

		// Clone the source element as a floating ghost
		ghost = sourceEl.cloneNode(true) as HTMLElement;
		ghost.style.position = 'fixed';
		ghost.style.pointerEvents = 'none';
		ghost.style.zIndex = '9999';
		ghost.style.opacity = '0.85';
		ghost.style.width = `${sourceEl.offsetWidth}px`;
		ghost.style.left = `${event.clientX - sourceEl.offsetWidth / 2}px`;
		ghost.style.top = `${event.clientY - 16}px`;
		document.body.appendChild(ghost);

		function handlePointerMove(e: PointerEvent) {
			if (ghost) {
				ghost.style.left = `${e.clientX - sourceEl.offsetWidth / 2}px`;
				ghost.style.top = `${e.clientY - 16}px`;
			}
		}

		function handlePointerUp(e: PointerEvent) {
			if (ghost) {
				ghost.remove();
				ghost = null;
			}
			if (dnd.current) {
				dnd.drop(e.clientX, e.clientY);
			}
			window.removeEventListener('pointermove', handlePointerMove);
			window.removeEventListener('pointerup', handlePointerUp);
		}

		window.addEventListener('pointermove', handlePointerMove);
		window.addEventListener('pointerup', handlePointerUp);
	}
</script>

<div class="h-full flex flex-col bg-muted/30">
	<!-- Header -->
	<div class="p-3 border-b border-border">
		<h3 class="font-medium text-sm">{m.table_palette_title()}</h3>
		<p class="text-xs text-muted-foreground">{m.table_palette_hint()}</p>
	</div>

	<!-- Table List -->
	<div class="flex-1 overflow-hidden">
		<ScrollArea class="h-full">
		<div class="p-2 space-y-0.5" role="list">
			{#if tables.length === 0}
				<div class="px-2 py-4 text-sm text-muted-foreground text-center">
					{m.table_palette_no_tables()}
				</div>
			{/if}
			{#each tables as table (table.name)}
				<div
					role="listitem"
					class="flex items-center gap-2 px-2 py-1.5 rounded text-sm cursor-grab hover:bg-muted select-none"
					onpointerdown={(e) => handlePointerDown(e, table.name)}
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
					class="flex items-center gap-2 px-2 py-1.5 rounded text-sm cursor-grab hover:bg-indigo-500/10 border border-dashed border-indigo-500/30 select-none"
					onpointerdown={(e) => handlePointerDown(e, '__subquery__')}
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
					class="flex items-center gap-2 px-2 py-1.5 rounded text-sm cursor-grab hover:bg-violet-500/10 border border-dashed border-violet-500/30 select-none"
					onpointerdown={(e) => handlePointerDown(e, '__cte__')}
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
								class="flex items-center gap-2 px-2 py-1.5 rounded text-sm cursor-grab hover:bg-violet-500/10 select-none"
								onpointerdown={(e) => handlePointerDown(e, `__cte__${cte.id}`)}
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
</div>
