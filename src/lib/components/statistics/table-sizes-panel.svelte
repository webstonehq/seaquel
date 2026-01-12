<script lang="ts">
	import type { TableSizeInfo } from '$lib/types';
	import { ArrowUpDown as ArrowUpDownIcon } from '@lucide/svelte';
	import { Button } from '$lib/components/ui/button';

	interface Props {
		tables: TableSizeInfo[];
	}

	let { tables }: Props = $props();

	type SortKey = 'name' | 'rowCount' | 'totalSizeBytes';
	let sortKey = $state<SortKey>('totalSizeBytes');
	let sortAsc = $state(false);

	const sortedTables = $derived(() => {
		return [...tables].sort((a, b) => {
			let cmp = 0;
			if (sortKey === 'name') {
				cmp = a.name.localeCompare(b.name);
			} else if (sortKey === 'rowCount') {
				cmp = a.rowCount - b.rowCount;
			} else {
				cmp = a.totalSizeBytes - b.totalSizeBytes;
			}
			return sortAsc ? cmp : -cmp;
		});
	});

	function toggleSort(key: SortKey) {
		if (sortKey === key) {
			sortAsc = !sortAsc;
		} else {
			sortKey = key;
			sortAsc = false;
		}
	}

	function formatNumber(num: number): string {
		return num.toLocaleString();
	}
</script>

<div class="space-y-4">
	<h3 class="font-medium">Table Sizes</h3>
	<div class="rounded-lg border">
		<table class="w-full text-sm">
			<thead class="border-b bg-muted/50">
				<tr>
					<th class="px-4 py-2 text-left">
						<Button variant="ghost" size="sm" class="h-auto -ml-2 p-1" onclick={() => toggleSort('name')}>
							Table
							<ArrowUpDownIcon class="ml-1 size-3" />
						</Button>
					</th>
					<th class="px-4 py-2 text-right">
						<Button variant="ghost" size="sm" class="h-auto -mr-2 p-1" onclick={() => toggleSort('rowCount')}>
							Rows
							<ArrowUpDownIcon class="ml-1 size-3" />
						</Button>
					</th>
					<th class="px-4 py-2 text-right">
						<Button variant="ghost" size="sm" class="h-auto -mr-2 p-1" onclick={() => toggleSort('totalSizeBytes')}>
							Size
							<ArrowUpDownIcon class="ml-1 size-3" />
						</Button>
					</th>
				</tr>
			</thead>
			<tbody>
				{#each sortedTables() as table (table.schema + '.' + table.name)}
					<tr class="border-b last:border-0 hover:bg-muted/30">
						<td class="px-4 py-2">
							<span class="text-muted-foreground">{table.schema}.</span>{table.name}
						</td>
						<td class="px-4 py-2 text-right font-mono text-muted-foreground">
							{formatNumber(table.rowCount)}
						</td>
						<td class="px-4 py-2 text-right font-mono">
							{table.totalSize}
						</td>
					</tr>
				{:else}
					<tr>
						<td colspan="3" class="px-4 py-8 text-center text-muted-foreground">
							No tables found
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
</div>
