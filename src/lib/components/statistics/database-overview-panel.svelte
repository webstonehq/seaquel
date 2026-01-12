<script lang="ts">
	import type { DatabaseOverview } from '$lib/types';
	import StatsCard from './stats-card.svelte';

	interface Props {
		overview: DatabaseOverview;
	}

	let { overview }: Props = $props();
</script>

<div class="space-y-4">
	<h3 class="font-medium">Database Overview</h3>
	<div class="grid grid-cols-2 gap-4 md:grid-cols-4">
		<StatsCard label="Database" value={overview.databaseName} />
		<StatsCard label="Total Size" value={overview.totalSize} />
		<StatsCard label="Tables" value={overview.tableCount} />
		<StatsCard label="Indexes" value={overview.indexCount} />
		{#if overview.connectionCount !== undefined}
			<StatsCard label="Connections" value={overview.connectionCount} />
		{/if}
	</div>
</div>
