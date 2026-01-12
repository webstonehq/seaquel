<script lang="ts">
	import type { IndexUsageInfo } from '$lib/types';
	import { ArrowUpDown as ArrowUpDownIcon, AlertTriangle as AlertTriangleIcon } from '@lucide/svelte';
	import { Button } from '$lib/components/ui/button';

	interface Props {
		indexes: IndexUsageInfo[];
	}

	let { indexes }: Props = $props();

	type SortKey = 'indexName' | 'table' | 'scans';
	let sortKey = $state<SortKey>('scans');
	let sortAsc = $state(true);

	const sortedIndexes = $derived(() => {
		return [...indexes].sort((a, b) => {
			let cmp = 0;
			if (sortKey === 'indexName') {
				cmp = a.indexName.localeCompare(b.indexName);
			} else if (sortKey === 'table') {
				cmp = a.table.localeCompare(b.table);
			} else {
				cmp = a.scans - b.scans;
			}
			return sortAsc ? cmp : -cmp;
		});
	});

	function toggleSort(key: SortKey) {
		if (sortKey === key) {
			sortAsc = !sortAsc;
		} else {
			sortKey = key;
			sortAsc = true;
		}
	}

	function formatNumber(num: number): string {
		return num.toLocaleString();
	}

	const unusedCount = $derived(indexes.filter((i) => i.unused).length);
</script>

<div class="space-y-4">
	<div class="flex items-center justify-between">
		<h3 class="font-medium">Index Usage</h3>
		{#if unusedCount > 0}
			<span class="flex items-center gap-1 text-sm text-yellow-600 dark:text-yellow-500">
				<AlertTriangleIcon class="size-4" />
				{unusedCount} unused {unusedCount === 1 ? 'index' : 'indexes'}
			</span>
		{/if}
	</div>
	<div class="rounded-lg border">
		<table class="w-full text-sm">
			<thead class="border-b bg-muted/50">
				<tr>
					<th class="px-4 py-2 text-left">
						<Button variant="ghost" size="sm" class="h-auto -ml-2 p-1" onclick={() => toggleSort('indexName')}>
							Index
							<ArrowUpDownIcon class="ml-1 size-3" />
						</Button>
					</th>
					<th class="px-4 py-2 text-left">
						<Button variant="ghost" size="sm" class="h-auto -ml-2 p-1" onclick={() => toggleSort('table')}>
							Table
							<ArrowUpDownIcon class="ml-1 size-3" />
						</Button>
					</th>
					<th class="px-4 py-2 text-right">
						<Button variant="ghost" size="sm" class="h-auto -mr-2 p-1" onclick={() => toggleSort('scans')}>
							Scans
							<ArrowUpDownIcon class="ml-1 size-3" />
						</Button>
					</th>
					<th class="px-4 py-2 text-right">Size</th>
				</tr>
			</thead>
			<tbody>
				{#each sortedIndexes() as index (index.schema + '.' + index.table + '.' + index.indexName)}
					<tr
						class={[
							"border-b last:border-0 hover:bg-muted/30",
							index.unused && "bg-yellow-50 dark:bg-yellow-950/20"
						]}
					>
						<td class="px-4 py-2">
							{index.indexName}
							{#if index.unused}
								<span class="ml-2 rounded bg-yellow-100 px-1.5 py-0.5 text-xs text-yellow-700 dark:bg-yellow-900/50 dark:text-yellow-400">
									unused
								</span>
							{/if}
						</td>
						<td class="px-4 py-2 text-muted-foreground">
							{index.table}
						</td>
						<td class="px-4 py-2 text-right font-mono text-muted-foreground">
							{formatNumber(index.scans)}
						</td>
						<td class="px-4 py-2 text-right font-mono">
							{index.size}
						</td>
					</tr>
				{:else}
					<tr>
						<td colspan="4" class="px-4 py-8 text-center text-muted-foreground">
							No indexes found
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
