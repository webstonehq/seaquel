<script lang="ts">
	import type { StatisticsTab } from '$lib/types';
	import { RefreshCw as RefreshCwIcon, Loader2 as Loader2Icon } from '@lucide/svelte';
	import { Button } from '$lib/components/ui/button';
	import { useDatabase } from '$lib/hooks/database.svelte';
	import DatabaseOverviewPanel from './database-overview-panel.svelte';
	import TableSizesPanel from './table-sizes-panel.svelte';
	import IndexUsagePanel from './index-usage-panel.svelte';

	interface Props {
		tab: StatisticsTab;
	}

	let { tab }: Props = $props();

	const db = useDatabase();

	function handleRefresh() {
		db.statisticsTabs.refresh(tab.id);
	}

	function formatLastRefreshed(date: Date | undefined): string {
		if (!date) return '';
		return new Intl.DateTimeFormat(undefined, {
			hour: 'numeric',
			minute: '2-digit',
			second: '2-digit'
		}).format(date);
	}
</script>

<div class="flex h-full flex-col overflow-hidden">
	<div class="flex items-center justify-between border-b px-4 py-2">
		<div class="flex items-center gap-2">
			<h2 class="font-medium">{tab.name}</h2>
			{#if tab.lastRefreshed}
				<span class="text-sm text-muted-foreground">
					Updated {formatLastRefreshed(tab.lastRefreshed)}
				</span>
			{/if}
		</div>
		<Button variant="outline" size="sm" onclick={handleRefresh} disabled={tab.isLoading}>
			{#if tab.isLoading}
				<Loader2Icon class="mr-2 size-4 animate-spin" />
				Loading...
			{:else}
				<RefreshCwIcon class="mr-2 size-4" />
				Refresh
			{/if}
		</Button>
	</div>

	<div class="flex-1 overflow-auto p-4">
		{#if tab.isLoading && !tab.data}
			<div class="flex h-full items-center justify-center">
				<Loader2Icon class="size-8 animate-spin text-muted-foreground" />
			</div>
		{:else if tab.error}
			<div class="flex h-full flex-col items-center justify-center gap-2 text-destructive">
				<p class="font-medium">Failed to load statistics</p>
				<p class="text-sm">{tab.error}</p>
				<Button variant="outline" size="sm" onclick={handleRefresh} class="mt-4">
					Try Again
				</Button>
			</div>
		{:else if tab.data}
			<div class="space-y-8">
				<DatabaseOverviewPanel overview={tab.data.overview} />

				<div class="grid gap-8 lg:grid-cols-2">
					<TableSizesPanel tables={tab.data.tableSizes} />
					<IndexUsagePanel indexes={tab.data.indexUsage} />
				</div>
			</div>
		{/if}
	</div>
</div>
