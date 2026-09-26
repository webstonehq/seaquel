<script lang="ts">
	import type { DashboardWidget } from '$lib/types';
	import { RefreshCwIcon, Loader2Icon } from '@lucide/svelte';
	import { Button } from '$lib/components/ui/button';
	import * as ContextMenu from '$lib/components/ui/context-menu';
	import QueryChart from '$lib/components/charts/query-chart.svelte';
	import DashboardKpiWidget from './dashboard-kpi-widget.svelte';
	import DashboardTextWidget from './dashboard-text-widget.svelte';

	interface Props {
		widget: DashboardWidget;
		isEditing?: boolean;
		onclick: () => void;
		onRefresh: () => void;
		onDuplicate: () => void;
		onDelete: () => void;
	}

	let { widget, isEditing = false, onclick, onRefresh, onDuplicate, onDelete }: Props = $props();

	const columns = $derived(
		widget.result && widget.result.length > 0
			? Object.keys(widget.result[0])
			: []
	);

	// QueryChart wants columnar rows; widget.result comes from `executeReadOnly`
	// which still returns row objects. Convert here — dashboard datasets are
	// small, so the per-render allocation is inconsequential.
	const columnarRows = $derived(
		widget.result ? widget.result.map((r) => columns.map((c) => r[c])) : [],
	);
</script>

<ContextMenu.Root>
	<ContextMenu.Trigger class="h-full w-full">
		<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
		<div
			class="group relative flex h-full flex-col rounded-lg border bg-card shadow-sm overflow-hidden cursor-pointer hover:border-foreground/20 transition-colors {isEditing ? 'ring-2 ring-primary border-primary' : ''}"
			{onclick}
		>
			<!-- Title overlay -->
			<div class="absolute top-1.5 left-2 z-10 flex items-center gap-1">
				<span class="text-xs font-medium truncate text-muted-foreground">{widget.title}</span>
				{#if widget.isLoading}
					<Loader2Icon class="size-3 animate-spin text-muted-foreground shrink-0" />
				{/if}
			</div>

			<!-- Refresh button -->
			{#if widget.widgetType !== 'text'}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<div class="absolute top-1 right-1 z-10" onclick={(e) => e.stopPropagation()}>
					<Button
						variant="ghost"
						size="icon"
						class="size-5 opacity-0 group-hover:opacity-100 transition-opacity shrink-0"
						onclick={(e) => { e.stopPropagation(); onRefresh(); }}
					>
						<RefreshCwIcon class="size-3" />
					</Button>
				</div>
			{/if}

			<!-- Content -->
			<div class="flex-1 min-h-0 overflow-hidden pt-6">
				{#if widget.widgetType === 'text'}
					<DashboardTextWidget {widget} />
				{:else if widget.error}
					<div class="flex h-full items-center justify-center p-4 text-destructive text-xs">
						{widget.error}
					</div>
				{:else if widget.widgetType === 'kpi'}
					<DashboardKpiWidget {widget} />
				{:else if widget.result && widget.result.length > 0 && columns.length > 0}
					<QueryChart
						columns={columns}
						rows={columnarRows}
						config={widget.chartConfig}
					/>
				{:else if !widget.isLoading}
					<div class="flex h-full items-center justify-center text-xs text-muted-foreground">
						{widget.query ? 'No data returned' : 'No query configured'}
					</div>
				{/if}
			</div>
		</div>
	</ContextMenu.Trigger>
	<ContextMenu.Portal>
		<ContextMenu.Content>
			<ContextMenu.Item onclick={onclick}>Edit</ContextMenu.Item>
			<ContextMenu.Item onclick={onDuplicate}>Duplicate</ContextMenu.Item>
			<ContextMenu.Separator />
			<ContextMenu.Item class="text-destructive data-[highlighted]:text-destructive" onclick={onDelete}>Delete</ContextMenu.Item>
		</ContextMenu.Content>
	</ContextMenu.Portal>
</ContextMenu.Root>
