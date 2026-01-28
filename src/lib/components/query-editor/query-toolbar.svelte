<script lang="ts">
	import { Button, buttonVariants } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { Badge } from "$lib/components/ui/badge";
	import ShortcutKeys from "$lib/components/shortcut-keys.svelte";
	import { findShortcut } from "$lib/shortcuts/index.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
	import {
		PlayIcon,
		SaveIcon,
		LoaderIcon,
		ChevronDownIcon,
		WandSparklesIcon,
		XCircleIcon,
		SearchIcon,
		ActivityIcon,
		DatabaseIcon,
		CheckIcon,
		NetworkIcon,
		GitBranchIcon
	} from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";
	import type { StatementResult } from "$lib/types";

	type Props = {
		isExecuting: boolean;
		hasQuery: boolean;
		activeResult: StatementResult | null | undefined;
		liveStatementCount: number;
		onExecute: () => void;
		onExecuteCurrent: () => void;
		onExplain: (analyze: boolean) => void;
		onVisualize: () => void;
		onFormat: () => void;
		onSave: () => void;
		onShare?: () => void;
	};

	let {
		isExecuting,
		hasQuery,
		activeResult,
		liveStatementCount,
		onExecute,
		onExecuteCurrent,
		onExplain,
		onVisualize,
		onFormat,
		onSave,
		onShare
	}: Props = $props();

	const db = useDatabase();

	// Check if shared repos exist
	const hasSharedRepos = $derived(db.state.sharedRepos.length > 0);

	// Get labels for a connection
	const getConnectionLabels = (connectionId: string) => {
		return db.labels.getConnectionLabelsById(connectionId);
	};

	// Handle connection selection
	const handleConnectionSelect = async (connection: typeof db.state.connections[0]) => {
		// If connection has a database instance, just activate it
		if (connection.database || connection.mssqlConnectionId || connection.providerConnectionId) {
			db.connections.setActive(connection.id);
		} else {
			// Try auto-reconnect first if password is saved
			const autoReconnected = await db.connections.autoReconnect(connection.id);
			if (autoReconnected) {
				return;
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

	// Check if connection is connected
	const isConnected = (connection: typeof db.state.connections[0]) => {
		return !!(connection.database || connection.mssqlConnectionId || connection.providerConnectionId);
	};
</script>

<div class="flex items-center justify-between p-2 shrink-0">
	<div class="flex items-center gap-3 text-xs text-muted-foreground">
		<!-- Connection Selector -->
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
							<span
								class={[
									"size-2 rounded-full shrink-0",
									isConnected(connection) ? "bg-green-500" : "bg-gray-400"
								]}
							></span>
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

		{#if liveStatementCount > 1}
			<span class="flex items-center gap-1">
				<Badge variant="outline" class="text-xs"
					>{m.query_statements_count({ count: liveStatementCount })}</Badge
				>
			</span>
		{/if}
		{#if activeResult}
			{#if activeResult.isError}
				<span class="flex items-center gap-1 text-destructive">
					<XCircleIcon class="size-3" />
					{m.query_error()}
				</span>
			{:else if activeResult.queryType && ['insert', 'update', 'delete'].includes(activeResult.queryType)}
				<span class="flex items-center gap-1">
					<Badge variant="secondary" class="text-xs"
						>{activeResult.affectedRows ?? 0}</Badge
					>
					{m.query_rows_affected()}
					{#if activeResult.lastInsertId}
						<Badge variant="outline" class="text-xs ms-1"
							>ID: {activeResult.lastInsertId}</Badge
						>
					{/if}
				</span>
			{/if}
		{/if}
	</div>
	<div class="flex items-center gap-2">
		<div class="flex">
			<Button
				size="sm"
				class="h-7 gap-1 rounded-r-none border-r-0"
				onclick={onExecuteCurrent}
				disabled={isExecuting}
			>
				{#if isExecuting}
					<LoaderIcon class="animate-spin size-3" />
				{:else}
					<PlayIcon class="size-3" />
				{/if}
				{m.query_execute()}
			</Button>
			<DropdownMenu.Root>
				<DropdownMenu.Trigger
					class={buttonVariants({ size: "sm", variant: "default" }) +
						" !h-7 px-1.5 rounded-l-none border-l border-primary-foreground/20"}
					disabled={isExecuting}
				>
					<ChevronDownIcon class="size-3" />
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end">
					<DropdownMenu.Item onclick={onExecuteCurrent}>
						<PlayIcon class="size-4 me-2" />
						{m.query_execute_current()}
						{#if findShortcut('executeQuery')}
							<ShortcutKeys keys={findShortcut('executeQuery')!.keys} class="ms-auto" />
						{/if}
					</DropdownMenu.Item>
					{#if liveStatementCount > 1}
						<DropdownMenu.Item onclick={onExecute}>
							<PlayIcon class="size-4 me-2" />
							{m.query_execute_all()}
						</DropdownMenu.Item>
					{/if}
					<DropdownMenu.Separator />
					<DropdownMenu.Item onclick={() => onExplain(false)}>
						<SearchIcon class="size-4 me-2" />
						{m.query_explain()}
					</DropdownMenu.Item>
					<DropdownMenu.Item onclick={() => onExplain(true)}>
						<ActivityIcon class="size-4 me-2" />
						{m.query_explain_analyze()}
					</DropdownMenu.Item>
					<DropdownMenu.Separator />
					<DropdownMenu.Item onclick={onVisualize}>
						<NetworkIcon class="size-4 me-2" />
						Visualize Query
					</DropdownMenu.Item>
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
		<Button
			size="sm"
			variant="outline"
			class="h-7 gap-1"
			onclick={onFormat}
			disabled={!hasQuery}
		>
			<WandSparklesIcon class="size-3" />
			{m.query_format()}
			{#if findShortcut('formatSql')}
				<ShortcutKeys keys={findShortcut('formatSql')!.keys} class="ms-1" />
			{/if}
		</Button>
		<div class="flex">
			<Button
				size="sm"
				variant="outline"
				class="h-7 gap-1 rounded-r-none border-r-0"
				onclick={onSave}
				disabled={!hasQuery}
			>
				<SaveIcon class="size-3" />
				{m.query_save()}
				{#if findShortcut('saveQuery')}
					<ShortcutKeys keys={findShortcut('saveQuery')!.keys} class="ms-1" />
				{/if}
			</Button>
			<DropdownMenu.Root>
				<DropdownMenu.Trigger
					class={buttonVariants({ size: "sm", variant: "outline" }) +
						" !h-7 px-1.5 rounded-l-none"}
					disabled={!hasQuery}
				>
					<ChevronDownIcon class="size-3" />
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end">
					<DropdownMenu.Item onclick={onSave}>
						<SaveIcon class="size-4 me-2" />
						{m.query_save()}
						{#if findShortcut('saveQuery')}
							<ShortcutKeys keys={findShortcut('saveQuery')!.keys} class="ms-auto" />
						{/if}
					</DropdownMenu.Item>
					{#if onShare}
						<DropdownMenu.Item onclick={onShare} disabled={!hasSharedRepos}>
							<GitBranchIcon class="size-4 me-2" />
							Share to Repository
						</DropdownMenu.Item>
					{/if}
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
	</div>
</div>
