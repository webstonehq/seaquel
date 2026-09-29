<script lang="ts">
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import DatabaseIcon from '@lucide/svelte/icons/database';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import CheckIcon from '@lucide/svelte/icons/check';
	import LoaderIcon from '@lucide/svelte/icons/loader';
	import { useDatabase } from '$lib/hooks/database.svelte.js';
	import { m } from '$lib/paraglide/messages.js';

	const db = useDatabase();

	const getConnectionLabels = (connectionId: string) => {
		return db.labels.getConnectionLabelsById(connectionId);
	};

	const handleConnectionSelect = async (connection: typeof db.state.connections[0]) => {
		if (connection.providerConnectionId) {
			db.connections.setActive(connection.id);
		} else {
			const autoReconnected = await db.connections.autoReconnect(connection.id);
			if (autoReconnected) {
				return;
			}
			void db.connectionTabs.open({
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

	const isConnected = (connection: typeof db.state.connections[0]) => {
		return !!(connection.providerConnectionId);
	};
</script>

{#if db.state.projectConnections.length > 0}
	<DropdownMenu.Root>
		<DropdownMenu.Trigger class="flex items-center gap-2 px-2 h-7 text-xs rounded-md bg-background border hover:bg-muted transition-colors">
			<DatabaseIcon class="size-3 text-muted-foreground" />
			{#if db.state.activeConnection}
				<span
					class={[
						"size-2 rounded-full shrink-0",
						isConnected(db.state.activeConnection) ? "bg-green-500" : "bg-gray-400"
					]}
				></span>
				<span class="max-w-24 truncate">{db.state.activeConnection.name}</span>
				{#if getConnectionLabels(db.state.activeConnection.id).length > 0}
					<Tooltip.Root>
						<Tooltip.Trigger class="flex items-center">
							{#each getConnectionLabels(db.state.activeConnection.id) as label, i (label.id)}
								<span
									class="size-2.5 rounded-full shrink-0 ring-1 ring-background"
									style="background-color: {label.color}; {i > 0 ? 'margin-left: -4px;' : ''}"
								></span>
							{/each}
						</Tooltip.Trigger>
						<Tooltip.Content>
							<div class="flex flex-col gap-1">
								{#each getConnectionLabels(db.state.activeConnection.id) as label (label.id)}
									<div class="flex items-center gap-1.5 text-xs">
										<span
											class="size-2 rounded-full"
											style="background-color: {label.color};"
										></span>
										{label.name}
									</div>
								{/each}
							</div>
						</Tooltip.Content>
					</Tooltip.Root>
				{/if}
			{:else}
				<span class="text-muted-foreground">{m.query_select_connection()}</span>
			{/if}
			<ChevronDownIcon class="size-3 text-muted-foreground" />
		</DropdownMenu.Trigger>
		<DropdownMenu.Content class="w-56" align="start">
			{#each db.state.projectConnections as connection (connection.id)}
				<DropdownMenu.Item
					class="flex items-center gap-2 cursor-pointer"
					onclick={() => handleConnectionSelect(connection)}
				>
					<span class="w-4">
						{#if db.state.activeConnectionId === connection.id}
							<CheckIcon class="size-4" />
						{/if}
					</span>
					<span class="flex size-2 items-center justify-center shrink-0">
						{#if db.connections.connectingIds.has(connection.id)}
							<LoaderIcon class="size-3 animate-spin text-muted-foreground" />
						{:else}
							<span
								class={[
									"size-2 rounded-full",
									isConnected(connection) ? "bg-green-500" : "bg-gray-400"
								]}
							></span>
						{/if}
					</span>
					<span class="flex-1 truncate">{connection.name}</span>
					{#if getConnectionLabels(connection.id).length > 0}
						<div class="flex items-center" title={getConnectionLabels(connection.id).map(l => l.name).join(', ')}>
							{#each getConnectionLabels(connection.id) as label, i (label.id)}
								<span
									class="size-2.5 rounded-full shrink-0 ring-1 ring-popover"
									style="background-color: {label.color}; {i > 0 ? 'margin-left: -4px;' : ''}"
								></span>
							{/each}
						</div>
					{/if}
				</DropdownMenu.Item>
			{/each}
		</DropdownMenu.Content>
	</DropdownMenu.Root>
{/if}
