<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
	import { dbeaverImportStore } from "$lib/stores/dbeaver-import.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { m } from "$lib/paraglide/messages.js";
	import { isTauri } from "$lib/utils/environment";
	import { getFeatures } from "$lib/features";
	import { PlusIcon, DownloadIcon, DatabaseIcon, PlugIcon } from "@lucide/svelte";

	const db = useDatabase();
	const features = getFeatures();

	// Get recent connections sorted by last connected time
	const recentConnections = $derived(
		[...db.state.projectConnections]
			.sort((a, b) => {
				const aTime = a.lastConnected?.getTime() ?? 0;
				const bTime = b.lastConnected?.getTime() ?? 0;
				return bTime - aTime;
			})
			.slice(0, 4)
	);

	const handleImportDbeaver = async () => {
		const existingIds = db.state.connections.map((c) => c.id);
		await dbeaverImportStore.checkAndShowDialog(existingIds);
	};

	const handleConnectionClick = async (connection: typeof db.state.connections[0]) => {
		if (connection.database || connection.mssqlConnectionId || connection.providerConnectionId) {
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
	};
</script>

<div class="flex-1 flex items-center justify-center p-8 overflow-auto">
	<div class="max-w-lg w-full space-y-8">
		<!-- Header -->
		<div class="text-center space-y-2">
			<DatabaseIcon class="size-12 mx-auto text-muted-foreground/50" />
			<h1 class="text-2xl font-semibold">{m.starter_getting_started_title()}</h1>
			<p class="text-muted-foreground">{m.starter_getting_started_description()}</p>
		</div>

		<!-- Quick Actions -->
		<div class="flex flex-col gap-3">
			{#if features.newConnections}
				<Button size="lg" class="w-full" onclick={() => connectionDialogStore.open()}>
					<PlusIcon class="size-4 me-2" />
					{m.starter_add_connection()}
				</Button>
			{/if}
			{#if isTauri()}
				<Button size="lg" variant="outline" class="w-full" onclick={handleImportDbeaver}>
					<DownloadIcon class="size-4 me-2" />
					{m.starter_import_dbeaver()}
				</Button>
			{/if}
		</div>

		<!-- Recent Connections -->
		{#if recentConnections.length > 0}
			<div class="space-y-3">
				<h2 class="text-sm font-medium text-muted-foreground">{m.starter_recent_connections()}</h2>
				<div class="grid gap-2">
					{#each recentConnections as connection (connection.id)}
						<Card
							class="cursor-pointer hover:bg-muted/50 transition-colors"
							onclick={() => handleConnectionClick(connection)}
						>
							<CardContent class="p-3 flex items-center gap-3">
								<span
									class={[
										"size-2 rounded-full shrink-0",
										(connection.database || connection.mssqlConnectionId || connection.providerConnectionId) ? "bg-green-500" : "bg-gray-400"
									]}
								></span>
								<div class="flex-1 min-w-0">
									<p class="font-medium truncate">{connection.name}</p>
									<p class="text-xs text-muted-foreground truncate">
										{connection.host}:{connection.port} / {connection.databaseName}
									</p>
								</div>
								<PlugIcon class="size-4 text-muted-foreground" />
							</CardContent>
						</Card>
					{/each}
				</div>
			</div>
		{/if}
	</div>
</div>
