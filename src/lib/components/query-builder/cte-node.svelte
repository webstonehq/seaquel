<script lang="ts">
	import { Handle, Position, NodeResizer } from '@xyflow/svelte';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte.js';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import LayersIcon from '@lucide/svelte/icons/layers';
	import XIcon from '@lucide/svelte/icons/x';
	import CalculatorIcon from '@lucide/svelte/icons/calculator';

	interface Props {
		id: string;
		data: {
			cteId: string;
			name: string;
			tableCount: number;
			hasAggregates: boolean;
		};
		isConnectable?: boolean;
		selected?: boolean;
	}

	let { id, data, isConnectable = true, selected = false }: Props = $props();

	const qb = useQueryBuilder();

	function handleDelete() {
		qb.removeCte(data.cteId);
	}

	function handleNameChange(e: Event) {
		const target = e.target as HTMLInputElement;
		qb.updateCteName(data.cteId, target.value);
	}

	function handleKeyDown(e: KeyboardEvent) {
		// Stop propagation to prevent SvelteFlow from capturing Delete/Backspace
		e.stopPropagation();
	}

	function handleClick() {
		// Select this CTE to edit its inner query in the filter panel
		qb.selectedCteId = data.cteId;
		qb.selectedSubqueryId = null; // Clear any selected subquery
	}
</script>

<NodeResizer
	minWidth={250}
	minHeight={150}
	isVisible={selected}
	lineStyle="border-color: var(--color-primary)"
	handleStyle="background-color: var(--color-primary); width: 8px; height: 8px;"
	onResize={(_, params) => {
		qb.updateCteSize(data.cteId, { width: params.width, height: params.height });
	}}
/>

<!-- svelte-ignore a11y_click_events_have_key_events -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	class="rounded-lg border-2 border-dashed bg-violet-500/5 min-w-[250px] min-h-[150px] h-full w-full flex flex-col {selected ? 'border-violet-500' : 'border-violet-500/50'}"
	onclick={handleClick}
>
	<!-- Header -->
	<div
		class="flex items-center justify-between gap-2 px-3 py-2 rounded-t-md border-b bg-violet-500/10 border-violet-500/30"
	>
		<div class="flex items-center gap-2 min-w-0 flex-1">
			<LayersIcon class="size-4 shrink-0 text-violet-500" />

			<!-- CTE Name Input -->
			<span class="text-xs text-muted-foreground font-medium">WITH</span>
			<Input
				type="text"
				placeholder="cte_name"
				value={data.name}
				oninput={handleNameChange}
				onkeydown={handleKeyDown}
				class="h-6 text-xs w-28 font-mono bg-violet-500/5 border-violet-500/30 focus:border-violet-500"
			/>
			<span class="text-xs text-muted-foreground font-medium">AS</span>
		</div>

		<!-- Info badges -->
		<div class="flex items-center gap-1 shrink-0">
			{#if data.tableCount > 0}
				<span class="text-xs bg-violet-500/20 rounded px-1.5 py-0.5 text-violet-600 dark:text-violet-400">
					{data.tableCount} {data.tableCount === 1 ? 'table' : 'tables'}
				</span>
			{/if}
			{#if data.hasAggregates}
				<CalculatorIcon class="size-3 text-muted-foreground" />
			{/if}
		</div>

		<!-- Delete button -->
		<Button
			variant="ghost"
			size="icon-sm"
			class="size-6 text-muted-foreground hover:text-destructive shrink-0"
			onclick={handleDelete}
		>
			<XIcon class="size-3" />
		</Button>
	</div>

	<!-- Content area - child nodes will render here -->
	<div class="flex-1 p-2 relative pointer-events-none">
		{#if data.tableCount === 0}
			<div class="absolute inset-0 flex items-center justify-center text-muted-foreground text-xs pointer-events-none">
				Drag tables here to build CTE query
			</div>
		{/if}
	</div>
</div>
