<script lang="ts">
	import { SvelteSet } from "svelte/reactivity";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { formatRelativeTime } from "$lib/utils.js";
	import { Sidebar, SidebarContent, SidebarFooter, SidebarHeader, SidebarRail, SidebarGroup, SidebarGroupLabel, SidebarGroupContent, SidebarMenu, SidebarMenuItem, SidebarMenuButton } from "$lib/components/ui/sidebar";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Tabs, TabsContent, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import { TableIcon, ChevronRightIcon, FolderIcon, HistoryIcon, StarIcon, ClockIcon, BookmarkIcon, Trash2Icon, SearchIcon, DatabaseIcon, FileTextIcon, PlusIcon, PlugIcon, UnplugIcon, TagIcon, BarChart3Icon, NetworkIcon, LayoutGridIcon, MoreHorizontalIcon } from "@lucide/svelte";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { m } from "$lib/paraglide/messages.js";
	import { isTauri } from "$lib/utils/environment";
	import { isDemo, getFeatures } from "$lib/features";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
	import ConnectionLabelPicker from "$lib/components/connection-label-picker.svelte";

	const db = useDatabase();
	const features = getFeatures();
	let sidebarTab = $state<"schema" | "queries">("schema");
	let version = $state("");
	let connectionsExpanded = $state(true);

	// Remove connection confirmation dialog state
	let showRemoveDialog = $state(false);
	let connectionToRemove = $state<string | null>(null);
	let connectionToRemoveName = $state("");

	// Labels dialog state
	let showLabelsDialog = $state(false);
	let connectionToEditLabels = $state<string | null>(null);
	let connectionToEditLabelsName = $state("");

	$effect(() => {
		if (isTauri()) {
			import("@tauri-apps/api/app").then(({ getVersion }) => {
				getVersion().then((v) => {
					version = v;
				});
			});
		}
	});
	let expandedSchemas = new SvelteSet<string>();
	let historyExpanded = $state(true);
	let savedExpanded = $state(true);
	let searchQuery = $state("");
	let schemaSearchQuery = $state("");

	const toggleSchema = (schemaName: string) => {
		if (expandedSchemas.has(schemaName)) {
			expandedSchemas.delete(schemaName);
		} else {
			expandedSchemas.add(schemaName);
		}
	};

	// Filter and group tables by schema
	const tablesBySchema = $derived.by(() => {
		const searchLower = schemaSearchQuery.toLowerCase();
		const filtered = schemaSearchQuery
			? db.state.activeSchema.filter(table =>
				table.name.toLowerCase().includes(searchLower) ||
				(table.schema || "").toLowerCase().includes(searchLower)
			)
			: db.state.activeSchema;

		const grouped = new Map<string, typeof db.state.activeSchema>();
		filtered.forEach((table) => {
			const schema = table.schema || "default";
			if (!grouped.has(schema)) {
				grouped.set(schema, []);
			}
			grouped.get(schema)!.push(table);
		});
		return grouped;
	});

	const handleTableClick = (table: (typeof db.state.activeSchema)[0]) => {
		db.schemaTabs.add(table);
		db.ui.setActiveView("schema");
	};

	const filteredHistory = $derived(
		db.state.activeConnectionQueryHistory.filter((item) =>
			item.query.toLowerCase().includes(searchQuery.toLowerCase()),
		),
	);

	const filteredSavedQueries = $derived(
		db.state.activeConnectionSavedQueries.filter(
			(item) =>
				item.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				item.query.toLowerCase().includes(searchQuery.toLowerCase()),
		),
	);

	const handleConnectionClick = async (connection: typeof db.state.connections[0]) => {
		// If connection has a database instance, just activate it
		if (connection.database || connection.mssqlConnectionId || connection.providerConnectionId) {
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

	const confirmRemoveConnection = (connectionId: string, name: string) => {
		connectionToRemove = connectionId;
		connectionToRemoveName = name;
		showRemoveDialog = true;
	};

	const handleRemoveConnection = () => {
		if (connectionToRemove) {
			db.connections.remove(connectionToRemove);
			connectionToRemove = null;
			connectionToRemoveName = "";
		}
		showRemoveDialog = false;
	};

	// Get labels for a connection
	const getConnectionLabels = (connection: typeof db.state.connections[0]) => {
		return db.labels.getConnectionLabelsById(connection.id);
	};

	const openLabelsDialog = (connectionId: string, name: string) => {
		connectionToEditLabels = connectionId;
		connectionToEditLabelsName = name;
		showLabelsDialog = true;
	};
</script>

<Sidebar class="top-(--header-height) h-[calc(100svh-var(--header-height))]" collapsible="offcanvas">
	<SidebarHeader class="p-0 py-1">
		<!-- Connections section -->
		<SidebarGroup class="py-2">
			<Collapsible bind:open={connectionsExpanded}>
				<div class="flex items-center justify-between px-3 py-1">
					<CollapsibleTrigger class="flex items-center gap-1 text-xs font-medium text-sidebar-foreground/70 hover:text-sidebar-foreground">
						<ChevronRightIcon class={["size-3 transition-transform", connectionsExpanded && "rotate-90"]} />
						{m.sidebar_connections()}
					</CollapsibleTrigger>
					{#if features.newConnections}
						<Button
							size="icon"
							variant="ghost"
							class="size-5 [&_svg:not([class*='size-'])]:size-3"
							onclick={() => connectionDialogStore.open()}
							title={m.sidebar_connections_add()}
						>
							<PlusIcon />
						</Button>
					{/if}
				</div>
				<CollapsibleContent>
					<SidebarGroupContent>
						<SidebarMenu class="px-2">
							{#each db.state.projectConnections as connection (connection.id)}
								<ContextMenu.Root>
									<ContextMenu.Trigger class="w-full">
										<SidebarMenuItem>
											<SidebarMenuButton
												class={[
													"flex items-center gap-2 cursor-pointer",
													db.state.activeConnectionId === connection.id && "bg-sidebar-accent"
												]}
												onclick={() => handleConnectionClick(connection)}
											>
												<span
													class={[
														"size-2 rounded-full shrink-0",
														(connection.database || connection.mssqlConnectionId || connection.providerConnectionId) ? "bg-green-500" : "bg-gray-400"
													]}
												></span>
												<span class="flex-1 truncate text-sm">{connection.name}</span>
												{#if getConnectionLabels(connection).length > 0}
													<Tooltip.Root>
														<Tooltip.Trigger class="flex items-center">
															{#each getConnectionLabels(connection) as label, i (label.id)}
																<span
																	class="size-2.5 rounded-full shrink-0 ring-1 ring-sidebar-background"
																	style="background-color: {label.color}; {i > 0 ? 'margin-left: -4px;' : ''}"
																></span>
															{/each}
														</Tooltip.Trigger>
														<Tooltip.Content side="right">
															<div class="flex flex-col gap-1">
																{#each getConnectionLabels(connection) as label (label.id)}
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
											</SidebarMenuButton>
										</SidebarMenuItem>
									</ContextMenu.Trigger>
									<ContextMenu.Content class="w-48">
										{#if connection.database || connection.mssqlConnectionId || connection.providerConnectionId}
											<ContextMenu.Item onclick={() => db.connections.toggle(connection.id)}>
												<UnplugIcon class="size-4 me-2" />
												{m.sidebar_connection_disconnect()}
											</ContextMenu.Item>
											<ContextMenu.Separator />
											<ContextMenu.Item onclick={() => {
												db.connections.setActive(connection.id);
												db.statisticsTabs.add();
											}}>
												<BarChart3Icon class="size-4 me-2" />
												Database Statistics
											</ContextMenu.Item>
											<ContextMenu.Item onclick={() => {
												db.connections.setActive(connection.id);
												db.erdTabs.add();
											}}>
												<NetworkIcon class="size-4 me-2" />
												Entity Relationship Diagram
											</ContextMenu.Item>
											<ContextMenu.Item onclick={() => {
												db.connections.setActive(connection.id);
												db.canvasTabs.add();
											}}>
												<LayoutGridIcon class="size-4 me-2" />
												Canvas Workspace
											</ContextMenu.Item>
										{:else}
											<ContextMenu.Item onclick={() => handleConnectionClick(connection)}>
												<PlugIcon class="size-4 me-2" />
												{m.sidebar_connection_connect()}
											</ContextMenu.Item>
										{/if}
										<ContextMenu.Separator />
										<ContextMenu.Item onclick={() => openLabelsDialog(connection.id, connection.name)}>
											<TagIcon class="size-4 me-2" />
											{m.sidebar_connection_labels()}
										</ContextMenu.Item>
										{#if !(isDemo() && connection.id === "demo-connection")}
											<ContextMenu.Separator />
											<ContextMenu.Item
												class="text-destructive focus:text-destructive"
												onclick={() => confirmRemoveConnection(connection.id, connection.name)}
											>
												<Trash2Icon class="size-4 me-2" />
												{m.sidebar_connection_delete()}
											</ContextMenu.Item>
										{/if}
									</ContextMenu.Content>
								</ContextMenu.Root>
							{/each}
							{#if db.state.projectConnections.length === 0}
								<div class="text-center py-2 text-muted-foreground">
									<p class="text-xs">{m.sidebar_no_connection()}</p>
								</div>
							{/if}
						</SidebarMenu>
					</SidebarGroupContent>
				</CollapsibleContent>
			</Collapsible>
		</SidebarGroup>

		<!-- Schema/Queries tabs - only show when connected -->
		{#if db.state.activeConnectionId}
			<Tabs bind:value={sidebarTab} class="w-full">
				<TabsList class="w-full justify-start rounded-none h-10 bg-transparent px-2">
					<TabsTrigger value="schema" class="text-xs data-[state=active]:bg-background">
						<DatabaseIcon class="size-3 me-1" />
						{m.sidebar_tab_schema()}
					</TabsTrigger>
					<TabsTrigger value="queries" class="text-xs data-[state=active]:bg-background">
						<FileTextIcon class="size-3 me-1" />
						{m.sidebar_tab_queries()}
					</TabsTrigger>
				</TabsList>
			</Tabs>
		{/if}
	</SidebarHeader>

	<SidebarContent>
		{#if db.state.activeConnectionId && db.state.activeConnection && (db.state.activeConnection.database || db.state.activeConnection.mssqlConnectionId || db.state.activeConnection.providerConnectionId)}
			{#if sidebarTab === "schema"}
				<div class="p-3 pb-2">
					<div class="relative">
						<SearchIcon class="absolute start-2 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
						<Input
							bind:value={schemaSearchQuery}
							placeholder={m.sidebar_search_tables()}
							class="ps-8 h-8 text-sm"
						/>
					</div>
				</div>
				<SidebarGroup>
					<SidebarGroupLabel>{db.state.activeConnection.name}</SidebarGroupLabel>
					<SidebarGroupContent>
						<SidebarMenu>
							{#each [...tablesBySchema.entries()] as [schemaName, tables] (schemaName)}
								<Collapsible open={expandedSchemas.has(schemaName)} onOpenChange={() => toggleSchema(schemaName)}>
									<SidebarMenuItem>
										<CollapsibleTrigger>
											{#snippet child({ props })}
												<SidebarMenuButton {...props}>
													<ChevronRightIcon class={["size-4 transition-transform", expandedSchemas.has(schemaName) && "rotate-90"]} />
													<FolderIcon class="size-4" />
													<span class="flex-1">{schemaName}</span>
													<Badge variant="secondary" class="text-xs">{tables.length}</Badge>
												</SidebarMenuButton>
											{/snippet}
										</CollapsibleTrigger>
										<CollapsibleContent>
											<SidebarMenu class="ms-4 border-s border-sidebar-border ps-2">
												{#each tables as table (table.name)}
													<SidebarMenuItem class="group/table-row flex pr-2">
														<SidebarMenuButton onclick={() => handleTableClick(table)}>
															<TableIcon class="size-4" />
															<span class="flex-1">{table.name}</span>
														<DropdownMenu.Root>
															<DropdownMenu.Trigger>
																{#snippet child({ props })}
																	<button
																		{...props}
																		class="TTTabsolute end-0 top-1.5 flex size-5 items-center justify-center rounded-md text-sidebar-foreground opacity-0 ring-sidebar-ring transition-opacity hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 focus-visible:outline-hidden group-hover/table-row:opacity-100 data-[state=open]:opacity-100"
																	>
																		<MoreHorizontalIcon class="size-4" />
																	</button>
																{/snippet}
															</DropdownMenu.Trigger>
															<DropdownMenu.Content align="end">
																{#if db.state.activeView === 'canvas' && db.state.activeCanvasTabId}
																	<DropdownMenu.Item onclick={() => db.canvas.addTableNode(table)}>
																		<LayoutGridIcon class="size-4 me-2" />
																		Add to canvas
																	</DropdownMenu.Item>
																{:else}
																	<Tooltip.Root>
																		<Tooltip.Trigger class="w-full">
																			<DropdownMenu.Item disabled class="w-full">
																				<LayoutGridIcon class="size-4 me-2" />
																				Add to canvas
																			</DropdownMenu.Item>
																		</Tooltip.Trigger>
																		<Tooltip.Content side="right">
																			Open a canvas view
																		</Tooltip.Content>
																	</Tooltip.Root>
																{/if}
															</DropdownMenu.Content>
														</DropdownMenu.Root>
														</SidebarMenuButton>
													</SidebarMenuItem>
												{/each}
											</SidebarMenu>
										</CollapsibleContent>
									</SidebarMenuItem>
								</Collapsible>
							{/each}
						</SidebarMenu>
					</SidebarGroupContent>
				</SidebarGroup>
		{:else if sidebarTab === "queries"}
				<div class="p-3 pb-2">
					<div class="relative">
						<SearchIcon class="absolute start-2 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
						<Input
							bind:value={searchQuery}
							placeholder={m.sidebar_search_queries()}
							class="ps-8 h-8 text-sm"
						/>
					</div>
				</div>

				<SidebarGroup>
					<SidebarGroupContent>
						<SidebarMenu>
							<!-- Saved folder -->
							<Collapsible bind:open={savedExpanded}>
								<SidebarMenuItem>
									<CollapsibleTrigger>
										{#snippet child({ props })}
											<SidebarMenuButton {...props}>
												<ChevronRightIcon class={["size-4 transition-transform", savedExpanded && "rotate-90"]} />
												<BookmarkIcon class="size-4" />
												<span class="flex-1">{m.sidebar_saved()}</span>
												<Badge variant="secondary" class="text-xs">{filteredSavedQueries.length}</Badge>
											</SidebarMenuButton>
										{/snippet}
									</CollapsibleTrigger>
									<CollapsibleContent>
										<SidebarMenu class="ms-4 border-s border-sidebar-border ps-2">
											{#each filteredSavedQueries as item (item.id)}
												<SidebarMenuItem>
													<SidebarMenuButton
														class="h-auto py-2 flex-col items-start gap-1 group"
														onclick={() => db.queryTabs.loadSaved(item.id)}
													>
														<div class="flex items-center justify-between w-full gap-2">
															<div class="flex items-center gap-2 flex-1 min-w-0">
																<BookmarkIcon class="size-3 text-primary shrink-0" />
																<span class="text-sm font-medium truncate">{item.name}</span>
															</div>
															<Button
																size="icon"
																variant="ghost"
																class="size-5 shrink-0 [&_svg:not([class*='size-'])]:size-3 opacity-0 group-hover:opacity-100 transition-opacity hover:text-destructive"
																aria-label={m.history_delete_saved()}
																onclick={(e) => {
																	e.stopPropagation();
																	db.savedQueries.deleteSavedQuery(item.id);
																}}
															>
																<Trash2Icon />
															</Button>
														</div>
														<p class="text-xs text-muted-foreground w-full text-start">
															{m.sidebar_updated({ time: formatRelativeTime(item.updatedAt) })}
														</p>
														<p class="text-xs font-mono line-clamp-2 text-muted-foreground w-full text-left">
															{item.query}
														</p>
													</SidebarMenuButton>
												</SidebarMenuItem>
											{/each}
											{#if filteredSavedQueries.length === 0}
												<div class="text-center py-4 text-muted-foreground px-2">
													<p class="text-xs">{m.sidebar_no_saved()}</p>
												</div>
											{/if}
										</SidebarMenu>
									</CollapsibleContent>
								</SidebarMenuItem>
							</Collapsible>
							
							<!-- History folder -->
							<Collapsible bind:open={historyExpanded}>
								<SidebarMenuItem>
									<CollapsibleTrigger>
										{#snippet child({ props })}
											<SidebarMenuButton {...props}>
												<ChevronRightIcon class={["size-4 transition-transform", historyExpanded && "rotate-90"]} />
												<HistoryIcon class="size-4" />
												<span class="flex-1">{m.sidebar_history()}</span>
												<Badge variant="secondary" class="text-xs">{filteredHistory.length}</Badge>
											</SidebarMenuButton>
										{/snippet}
									</CollapsibleTrigger>
									<CollapsibleContent>
										<SidebarMenu class="ms-4 border-s border-sidebar-border ps-2">
											{#each filteredHistory as item (item.id)}
												<SidebarMenuItem>
													<SidebarMenuButton
														class="h-auto py-2 flex-col items-start gap-1 group"
														onclick={() => db.queryTabs.loadFromHistory(item.id)}
													>
														<div class="flex items-center justify-between w-full gap-2">
															<div class="flex items-center gap-2 flex-1 min-w-0">
																<ClockIcon class="size-3 text-muted-foreground shrink-0" />
																<span class="text-xs text-muted-foreground">{formatRelativeTime(item.timestamp)}</span>
																<Badge variant="secondary" class="text-xs">{item.executionTime}ms</Badge>
															</div>
															<Button
																size="icon"
																variant="ghost"
																class={[
																	"size-5 shrink-0 [&_svg:not([class*='size-'])]:size-3",
																	item.favorite && "text-yellow-500",
																]}
																aria-label={item.favorite ? m.history_remove_favorite() : m.history_add_favorite()}
																onclick={(e) => {
																	e.stopPropagation();
																	db.history.toggleQueryFavorite(item.id);
																}}
															>
																<StarIcon class={[item.favorite && "fill-current"]} />
															</Button>
														</div>
														<p class="text-xs font-mono line-clamp-2 text-muted-foreground w-full text-left">
															{item.query}
														</p>
													</SidebarMenuButton>
												</SidebarMenuItem>
											{/each}
											{#if filteredHistory.length === 0}
												<div class="text-center py-4 text-muted-foreground px-2">
													<p class="text-xs">{m.sidebar_no_history()}</p>
												</div>
											{/if}
										</SidebarMenu>
									</CollapsibleContent>
								</SidebarMenuItem>
							</Collapsible>


						</SidebarMenu>
					</SidebarGroupContent>
				</SidebarGroup>
		{/if}
		{/if}
	</SidebarContent>

	<SidebarFooter class="p-4">
		<div class="text-xs text-muted-foreground flex justify-between">
			<span>
				{#if db.state.activeConnection}
					{#if sidebarTab === "schema"}
						{m.sidebar_tables_count({ count: db.state.activeSchema.length })}
					{:else}
						{m.sidebar_queries_stats({ executed: db.state.activeConnectionQueryHistory.length, saved: db.state.activeConnectionSavedQueries.length })}
					{/if}
				{:else}
					{m.sidebar_no_connection_footer()}
				{/if}
			</span>
			{#if isDemo()}
				<Badge variant="secondary" class="text-xs">Demo</Badge>
			{:else if version}
				<span>v{version}</span>
			{/if}
		</div>
	</SidebarFooter>

	<SidebarRail />
</Sidebar>

<!-- Delete Connection Dialog -->
<Dialog.Root bind:open={showRemoveDialog}>
	<Dialog.Content class="max-w-md">
		<Dialog.Header>
			<Dialog.Title>{m.header_delete_dialog_title()}</Dialog.Title>
			<Dialog.Description>
				{m.header_delete_dialog_description({ name: connectionToRemoveName })}
			</Dialog.Description>
		</Dialog.Header>
		<Dialog.Footer class="gap-2">
			<Button variant="outline" onclick={() => showRemoveDialog = false}>
				{m.header_button_cancel()}
			</Button>
			<Button variant="destructive" onclick={handleRemoveConnection}>
				{m.header_button_remove()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>

<!-- Labels Dialog -->
<Dialog.Root bind:open={showLabelsDialog}>
	<Dialog.Content class="max-w-md">
		<Dialog.Header>
			<Dialog.Title>{m.labels_dialog_title({ name: connectionToEditLabelsName })}</Dialog.Title>
		</Dialog.Header>
		<div class="py-4">
			{#if connectionToEditLabels}
				<ConnectionLabelPicker connectionId={connectionToEditLabels} />
			{/if}
		</div>
		<Dialog.Footer>
			<Button onclick={() => showLabelsDialog = false}>
				{m.labels_done()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
