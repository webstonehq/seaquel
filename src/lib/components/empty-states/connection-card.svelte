<script lang="ts">
	import * as Card from "$lib/components/ui/card/index.js";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import { Button } from "$lib/components/ui/button/index.js";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import MoreVerticalIcon from "@lucide/svelte/icons/more-vertical";
	import PencilIcon from "@lucide/svelte/icons/pencil";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import type { DatabaseConnection } from "$lib/types";
	import { formatRelativeTime } from "$lib/utils.js";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { m } from "$lib/paraglide/messages.js";
	import { isFeatureEnabled } from "$lib/features";

	interface Props {
		connection: DatabaseConnection;
	}

	const { connection }: Props = $props();
	const db = useDatabase();

	// Delete confirmation dialog state
	let showDeleteDialog = $state(false);

	const handleClick = async () => {
		if (connection.database || connection.providerConnectionId) {
			// Already connected, just activate
			db.connections.setActive(connection.id);
		} else {
			// Try auto-reconnect first if password is saved
			const autoReconnected = await db.connections.autoReconnect(connection.id);
			if (autoReconnected) {
				return; // Successfully reconnected
			}

			// Fall back to dialog if auto-reconnect fails or password not saved
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

	const handleEdit = (e: Event) => {
		e.stopPropagation();
		connectionDialogStore.open(
			{
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
			},
			"edit",
		);
	};

	const confirmDelete = (e: Event) => {
		e.stopPropagation();
		showDeleteDialog = true;
	};

	const handleDelete = () => {
		db.connections.remove(connection.id);
		showDeleteDialog = false;
	};

	const dbTypeLabels: Record<string, string> = {
		postgres: "PostgreSQL",
		mysql: "MySQL",
		mariadb: "MariaDB",
		sqlite: "SQLite",
		mongodb: "MongoDB",
		mssql: "SQL Server",
	};

	const canEdit = isFeatureEnabled("editConnections");
</script>

<div class="relative group">
	<button type="button" class="text-start w-full" onclick={handleClick}>
		<Card.Root class="hover:bg-muted/50 transition-colors cursor-pointer h-full">
			<Card.Header class="pb-2">
				<div class="flex items-center gap-2">
					<span
						class={[
							"size-2 rounded-full shrink-0",
							connection.database || connection.providerConnectionId
								? "bg-green-500"
								: "bg-gray-400",
						]}
						title={connection.database || connection.providerConnectionId
							? m.empty_states_connection_card_connected()
							: m.empty_states_connection_card_disconnected()}
					></span>
					<DatabaseIcon class="size-4 text-muted-foreground" />
					<Card.Title class="text-sm font-medium truncate line-clamp-1 text-ellipsis flex-1">
						{connection.name}
					</Card.Title>
				</div>
			</Card.Header>
			<Card.Content class="pt-0 space-y-2">
				<Badge variant="secondary" class="text-xs">
					{dbTypeLabels[connection.type] ?? connection.type}
				</Badge>
				<p class="text-xs text-muted-foreground truncate">
					{connection.host}:{connection.port}
				</p>
				<p class="text-xs text-muted-foreground truncate">
					{connection.databaseName}
				</p>
				{#if connection.lastConnected}
					<p class="text-xs text-muted-foreground">
						{m.empty_states_connection_card_last_connected({
							time: formatRelativeTime(connection.lastConnected),
						})}
					</p>
				{/if}
			</Card.Content>
		</Card.Root>
	</button>

	{#if canEdit}
		<div class="absolute top-2 right-2 opacity-0 group-hover:opacity-100 transition-opacity">
			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					{#snippet child({ props })}
						<Button
							{...props}
							variant="ghost"
							size="icon"
							class="size-7"
							onclick={(e: Event) => e.stopPropagation()}
						>
							<MoreVerticalIcon class="size-4" />
						</Button>
					{/snippet}
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end">
					<DropdownMenu.Item onclick={handleEdit}>
						<PencilIcon class="size-4 me-2" />
						{m.connection_card_edit()}
					</DropdownMenu.Item>
					<DropdownMenu.Separator />
					<DropdownMenu.Item class="text-destructive" onclick={confirmDelete}>
						<Trash2Icon class="size-4 me-2" />
						{m.connection_card_delete()}
					</DropdownMenu.Item>
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
	{/if}
</div>

<Dialog.Root bind:open={showDeleteDialog}>
	<Dialog.Content class="max-w-md">
		<Dialog.Header>
			<Dialog.Title>{m.header_delete_dialog_title()}</Dialog.Title>
			<Dialog.Description>
				{m.header_delete_dialog_description({ name: connection.name })}
			</Dialog.Description>
		</Dialog.Header>
		<Dialog.Footer class="gap-2">
			<Button variant="outline" onclick={() => (showDeleteDialog = false)}>
				{m.header_button_cancel()}
			</Button>
			<Button variant="destructive" onclick={handleDelete}>
				{m.connection_card_delete()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
