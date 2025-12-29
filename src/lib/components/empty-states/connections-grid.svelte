<script lang="ts">
	import PlusIcon from "@lucide/svelte/icons/plus";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
	import ConnectionCard from "./connection-card.svelte";

	const db = useDatabase();

	// Sort connections by last connected (most recent first)
	const sortedConnections = $derived(
		[...db.connections].sort((a, b) => {
			const aTime = a.lastConnected?.getTime() ?? 0;
			const bTime = b.lastConnected?.getTime() ?? 0;
			return bTime - aTime;
		})
	);
</script>

<div class="flex-1 overflow-y-auto p-8">
	<div class="max-w-3xl mx-auto space-y-6">
		<div class="text-center space-y-1">
			<h1 class="text-xl font-semibold">Select a Connection</h1>
			<p class="text-muted-foreground text-sm">Click a connection to get started</p>
		</div>

		<div class="flex flex-wrap justify-center items-stretch gap-4">
			<div class="w-56">
				<button
					type="button"
					class="w-full h-full flex flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed border-muted-foreground/25 hover:border-muted-foreground/50 hover:bg-muted/50 transition-colors cursor-pointer"
					onclick={() => connectionDialogStore.open()}
				>
					<PlusIcon class="size-8 text-muted-foreground" />
					<span class="text-sm text-muted-foreground">Add Connection</span>
				</button>
			</div>
			{#each sortedConnections as connection (connection.id)}
				<div class="w-56">
					<ConnectionCard {connection} />
				</div>
			{/each}
		</div>
	</div>
</div>
