<script lang="ts">
	import * as Card from "$lib/components/ui/card/index.js";
	import { Badge } from "$lib/components/ui/badge/index.js";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import type { DatabaseConnection } from "$lib/types";
	import { formatRelativeTime } from "$lib/utils.js";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		connection: DatabaseConnection;
	}

	const { connection }: Props = $props();
	const db = useDatabase();

	const handleClick = () => {
		if (connection.database) {
			// Already connected, just activate
			db.connections.setActive(connection.id);
		} else {
			// Need to reconnect - open dialog with prefill
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
			});
		}
	};

	const dbTypeLabels: Record<string, string> = {
		postgres: "PostgreSQL",
		mysql: "MySQL",
		mariadb: "MariaDB",
		sqlite: "SQLite",
		mongodb: "MongoDB",
		mssql: "SQL Server",
	};
</script>

<button type="button" class="text-start w-full" onclick={handleClick}>
	<Card.Root class="hover:bg-muted/50 transition-colors cursor-pointer h-full">
		<Card.Header class="pb-2">
			<div class="flex items-center gap-2">
				<span
					class={[
						"size-2 rounded-full shrink-0",
						connection.database ? "bg-green-500" : "bg-gray-400",
					]}
					title={connection.database ? m.empty_states_connection_card_connected() : m.empty_states_connection_card_disconnected()}
				></span>
				<DatabaseIcon class="size-4 text-muted-foreground" />
				<Card.Title class="text-sm font-medium truncate">{connection.name}</Card.Title>
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
					{m.empty_states_connection_card_last_connected({ time: formatRelativeTime(connection.lastConnected) })}
				</p>
			{/if}
		</Card.Content>
	</Card.Root>
</button>
