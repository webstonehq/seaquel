<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Sidebar, SidebarContent, SidebarFooter, SidebarHeader, SidebarRail, SidebarGroup, SidebarGroupLabel, SidebarGroupContent, SidebarMenu, SidebarMenuItem, SidebarMenuButton } from "$lib/components/ui/sidebar";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Tabs, TabsContent, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import { TableIcon, Columns3Icon, KeyIcon, ChevronRightIcon, FolderIcon, HistoryIcon, StarIcon, ClockIcon, BookmarkIcon, Trash2Icon, SearchIcon, DatabaseIcon, FileTextIcon } from "@lucide/svelte";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";

	const db = useDatabase();
	let sidebarTab = $state<"schema" | "queries">("schema");
	let expandedSchemas = $state<Set<string>>(new Set());
	let expandedTables = $state<Set<string>>(new Set());
	let historyExpanded = $state(true);
	let savedExpanded = $state(true);
	let searchQuery = $state("");

	const toggleSchema = (schemaName: string) => {
		const newExpanded = new Set(expandedSchemas);
		if (newExpanded.has(schemaName)) {
			newExpanded.delete(schemaName);
		} else {
			newExpanded.add(schemaName);
		}
		expandedSchemas = newExpanded;
	};

	const toggleTable = (tableName: string) => {
		const newExpanded = new Set(expandedTables);
		if (newExpanded.has(tableName)) {
			newExpanded.delete(tableName);
		} else {
			newExpanded.add(tableName);
		}
		expandedTables = newExpanded;
	};

	// Group tables by schema
	const tablesBySchema = $derived(() => {
		const grouped = new Map<string, typeof db.activeSchema>();
		db.activeSchema.forEach((table) => {
			const schema = table.schema || "default";
			if (!grouped.has(schema)) {
				grouped.set(schema, []);
			}
			grouped.get(schema)!.push(table);
		});
		return grouped;
	});

	const handleTableClick = (table: (typeof db.activeSchema)[0]) => {
		db.addSchemaTab(table);
		db.setActiveView("schema");
	};

	const filteredHistory = $derived(
		db.activeConnectionQueryHistory.filter((item) =>
			item.query.toLowerCase().includes(searchQuery.toLowerCase()),
		),
	);

	const filteredSavedQueries = $derived(
		db.activeConnectionSavedQueries.filter(
			(item) =>
				item.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				item.query.toLowerCase().includes(searchQuery.toLowerCase()),
		),
	);

	const formatTime = (date: Date) => {
		const now = new Date();
		const diff = now.getTime() - date.getTime();
		const minutes = Math.floor(diff / 60000);
		const hours = Math.floor(diff / 3600000);
		const days = Math.floor(diff / 86400000);

		if (minutes < 1) return "Just now";
		if (minutes < 60) return `${minutes}m ago`;
		if (hours < 24) return `${hours}h ago`;
		return `${days}d ago`;
	};
</script>

<Sidebar class="top-(--header-height)" collapsible="offcanvas">
	<SidebarHeader class="border-b border-sidebar-border p-0">
		<Tabs bind:value={sidebarTab} class="w-full">
			<TabsList class="w-full justify-start rounded-none h-10 bg-transparent px-2">
				<TabsTrigger value="schema" class="text-xs data-[state=active]:bg-background">
					<DatabaseIcon class="size-3 mr-1" />
					Schema
				</TabsTrigger>
				<TabsTrigger value="queries" class="text-xs data-[state=active]:bg-background">
					<FileTextIcon class="size-3 mr-1" />
					Queries
				</TabsTrigger>
			</TabsList>
		</Tabs>
	</SidebarHeader>

	<SidebarContent>
		{#if sidebarTab === "schema"}
			{#if db.activeConnection && db.activeConnection.database}
				<SidebarGroup>
					<SidebarGroupLabel>{db.activeConnection.name}</SidebarGroupLabel>
					<SidebarGroupContent>
						<SidebarMenu>
							{#each [...tablesBySchema().entries()] as [schemaName, tables] (schemaName)}
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
											<SidebarMenu class="ml-4 border-l border-sidebar-border pl-2">
												{#each tables as table (table.name)}
													<Collapsible open={expandedTables.has(table.name)} onOpenChange={() => toggleTable(table.name)}>
														<SidebarMenuItem>
															<CollapsibleTrigger>
																{#snippet child({ props })}
																	<SidebarMenuButton {...props} onclick={() => handleTableClick(table)}>
																		<TableIcon class="size-4" />
																		<span class="flex-1">{table.name}</span>
																		<Badge variant="secondary" class="text-xs">{table.rowCount?.toLocaleString()}</Badge>
																	</SidebarMenuButton>
																{/snippet}
															</CollapsibleTrigger>
															<CollapsibleContent>
																<SidebarMenu class="ml-4 border-l border-sidebar-border pl-2">
																	<SidebarMenuItem>
																		<SidebarMenuButton class="text-xs">
																			<Columns3Icon class="size-3" />
																			<span>{table.columns.length} columns</span>
																		</SidebarMenuButton>
																	</SidebarMenuItem>
																	<SidebarMenuItem>
																		<SidebarMenuButton class="text-xs">
																			<KeyIcon class="size-3" />
																			<span>{table.indexes.length} indexes</span>
																		</SidebarMenuButton>
																	</SidebarMenuItem>
																</SidebarMenu>
															</CollapsibleContent>
														</SidebarMenuItem>
													</Collapsible>
												{/each}
											</SidebarMenu>
										</CollapsibleContent>
									</SidebarMenuItem>
								</Collapsible>
							{/each}
						</SidebarMenu>
					</SidebarGroupContent>
				</SidebarGroup>
			{:else}
				<div class="flex-1 flex items-center justify-center p-4 text-center text-muted-foreground">
					<div>
						<p class="text-sm">No active connection</p>
						<p class="text-xs mt-1">Connect to a database to browse its schema</p>
					</div>
				</div>
			{/if}
		{:else if sidebarTab === "queries"}
			{#if db.activeConnection && db.activeConnection.database}
				<div class="p-3 pb-2">
					<div class="relative">
						<SearchIcon class="absolute left-2 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
						<Input
							bind:value={searchQuery}
							placeholder="Search queries..."
							class="pl-8 h-8 text-sm"
						/>
					</div>
				</div>

				<SidebarGroup>
					<SidebarGroupContent>
						<SidebarMenu>
							<!-- History folder -->
							<Collapsible bind:open={historyExpanded}>
								<SidebarMenuItem>
									<CollapsibleTrigger>
										{#snippet child({ props })}
											<SidebarMenuButton {...props}>
												<ChevronRightIcon class={["size-4 transition-transform", historyExpanded && "rotate-90"]} />
												<HistoryIcon class="size-4" />
												<span class="flex-1">History</span>
												<Badge variant="secondary" class="text-xs">{filteredHistory.length}</Badge>
											</SidebarMenuButton>
										{/snippet}
									</CollapsibleTrigger>
									<CollapsibleContent>
										<SidebarMenu class="ml-4 border-l border-sidebar-border pl-2">
											{#each filteredHistory as item (item.id)}
												<SidebarMenuItem>
													<SidebarMenuButton
														class="h-auto py-2 flex-col items-start gap-1 group"
														onclick={() => db.loadQueryFromHistory(item.id)}
													>
														<div class="flex items-center justify-between w-full gap-2">
															<div class="flex items-center gap-2 flex-1 min-w-0">
																<ClockIcon class="size-3 text-muted-foreground shrink-0" />
																<span class="text-xs text-muted-foreground">{formatTime(item.timestamp)}</span>
																<Badge variant="secondary" class="text-xs">{item.executionTime}ms</Badge>
															</div>
															<Button
																size="icon"
																variant="ghost"
																class={[
																	"size-5 shrink-0 [&_svg:not([class*='size-'])]:size-3",
																	item.favorite && "text-yellow-500",
																]}
																onclick={(e) => {
																	e.stopPropagation();
																	db.toggleQueryFavorite(item.id);
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
													<p class="text-xs">No history found</p>
												</div>
											{/if}
										</SidebarMenu>
									</CollapsibleContent>
								</SidebarMenuItem>
							</Collapsible>

							<!-- Saved folder -->
							<Collapsible bind:open={savedExpanded}>
								<SidebarMenuItem>
									<CollapsibleTrigger>
										{#snippet child({ props })}
											<SidebarMenuButton {...props}>
												<ChevronRightIcon class={["size-4 transition-transform", savedExpanded && "rotate-90"]} />
												<BookmarkIcon class="size-4" />
												<span class="flex-1">Saved</span>
												<Badge variant="secondary" class="text-xs">{filteredSavedQueries.length}</Badge>
											</SidebarMenuButton>
										{/snippet}
									</CollapsibleTrigger>
									<CollapsibleContent>
										<SidebarMenu class="ml-4 border-l border-sidebar-border pl-2">
											{#each filteredSavedQueries as item (item.id)}
												<SidebarMenuItem>
													<SidebarMenuButton
														class="h-auto py-2 flex-col items-start gap-1 group"
														onclick={() => db.loadSavedQuery(item.id)}
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
																onclick={(e) => {
																	e.stopPropagation();
																	db.deleteSavedQuery(item.id);
																}}
															>
																<Trash2Icon />
															</Button>
														</div>
														<p class="text-xs text-muted-foreground w-full text-left">
															Updated {formatTime(item.updatedAt)}
														</p>
														<p class="text-xs font-mono line-clamp-2 text-muted-foreground w-full text-left">
															{item.query}
														</p>
													</SidebarMenuButton>
												</SidebarMenuItem>
											{/each}
											{#if filteredSavedQueries.length === 0}
												<div class="text-center py-4 text-muted-foreground px-2">
													<p class="text-xs">No saved queries</p>
												</div>
											{/if}
										</SidebarMenu>
									</CollapsibleContent>
								</SidebarMenuItem>
							</Collapsible>
						</SidebarMenu>
					</SidebarGroupContent>
				</SidebarGroup>
			{:else}
				<div class="flex-1 flex items-center justify-center p-4 text-center text-muted-foreground">
					<div>
						<p class="text-sm">No active connection</p>
						<p class="text-xs mt-1">Connect to a database to see queries</p>
					</div>
				</div>
			{/if}
		{/if}
	</SidebarContent>

	<SidebarFooter class="border-t border-sidebar-border p-4">
		<div class="text-xs text-muted-foreground">
			{#if db.activeConnection}
				{#if sidebarTab === "schema"}
					{db.activeSchema.length} tables
				{:else}
					{db.activeConnectionQueryHistory.length} executed, {db.activeConnectionSavedQueries.length} saved
				{/if}
			{:else}
				No connection
			{/if}
		</div>
	</SidebarFooter>

	<SidebarRail />
</Sidebar>
