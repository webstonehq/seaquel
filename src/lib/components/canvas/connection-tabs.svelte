<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
	import { DatabaseIcon, CircleIcon } from "@lucide/svelte";
	import { cn } from "$lib/utils";

	const db = useDatabase();

	// Get connections for the current project
	const connections = $derived(db.state.projectConnections);

	async function handleConnectionClick(connection: (typeof connections)[0]) {
		if (isConnected(connection)) {
			db.connections.setActive(connection.id);
		} else {
			const autoReconnected = await db.connections.autoReconnect(connection.id);
			if (autoReconnected) {
				return;
			}
			connectionDialogStore.open({
				id: connection.id,
				name: connection.name,
				type: connection.type,
				host: connection.host,
				port: connection.port,
				databaseName: connection.databaseName,
				username: connection.username,
				sslMode: connection.sslMode,
				connectionString: connection.connectionString,
				sshTunnel: connection.sshTunnel,
				savePassword: connection.savePassword,
				saveSshPassword: connection.saveSshPassword,
				saveSshKeyPassphrase: connection.saveSshKeyPassphrase,
			});
		}
	}

	function isConnected(connection: (typeof connections)[0]): boolean {
		return !!(
			connection.providerConnectionId ||
			connection.mssqlConnectionId ||
			connection.database
		);
	}
</script>

<div class="flex items-center gap-1 px-4 py-2 bg-muted/30 border-b border-border overflow-x-auto">
	{#each connections as connection}
		{@const active = db.state.activeConnectionId === connection.id}
		{@const connected = isConnected(connection)}

		<button
			class={cn(
				"flex items-center gap-2 px-3 py-1.5 rounded-md text-sm transition-colors shrink-0",
				active
					? "bg-primary text-primary-foreground"
					: "hover:bg-muted text-muted-foreground hover:text-foreground"
			)}
			onclick={() => handleConnectionClick(connection)}
		>
			<CircleIcon
				class={cn(
					"size-2",
					connected ? "fill-green-500 text-green-500" : "fill-muted-foreground text-muted-foreground"
				)}
			/>
			<span class="truncate max-w-[120px]">{connection.name}</span>
		</button>
	{/each}

	{#if connections.length === 0}
		<div class="flex items-center gap-2 px-3 py-1.5 text-sm text-muted-foreground">
			<DatabaseIcon class="size-4" />
			<span>No connections in this project</span>
		</div>
	{/if}
</div>
