<script lang="ts">
	import { getVersion } from "@tauri-apps/api/app";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { formatRelativeTime } from "$lib/utils.js";
	import { Sidebar, SidebarContent, SidebarFooter, SidebarHeader, SidebarRail, SidebarGroup, SidebarGroupLabel, SidebarGroupContent, SidebarMenu, SidebarMenuItem, SidebarMenuButton } from "$lib/components/ui/sidebar";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Tabs, TabsContent, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import { TableIcon, ChevronRightIcon, FolderIcon, HistoryIcon, StarIcon, ClockIcon, BookmarkIcon, Trash2Icon, SearchIcon, DatabaseIcon, FileTextIcon } from "@lucide/svelte";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import { m } from "$lib/paraglide/messages.js";

	const db = useDatabase();
	let sidebarTab = $state<"schema" | "queries">("schema");
	let version = $state("");

	$effect(() => {
		getVersion().then((v) => {
			version = v;
		});
	});
	let expandedSchemas = $state<Set<string>>(new Set());
	let historyExpanded = $state(true);
	let savedExpanded = $state(true);
	let searchQuery = $state("");
	let schemaSearchQuery = $state("");

	const toggleSchema = (schemaName: string) => {
		const newExpanded = new Set(expandedSchemas);
		if (newExpanded.has(schemaName)) {
			newExpanded.delete(schemaName);
		} else {
			newExpanded.add(schemaName);
		}
		expandedSchemas = newExpanded;
	};

	// Filter and group tables by schema
	const tablesBySchema = $derived(() => {
		const searchLower = schemaSearchQuery.toLowerCase();
		const filtered = schemaSearchQuery
			? db.activeSchema.filter(table =>
				table.name.toLowerCase().includes(searchLower) ||
				(table.schema || "").toLowerCase().includes(searchLower)
			)
			: db.activeSchema;

		const grouped = new Map<string, typeof db.activeSchema>();
		filtered.forEach((table) => {
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

</script>

<Sidebar class="top-(--header-height) h-[calc(100svh-var(--header-height))]" collapsible="offcanvas">
	<SidebarHeader class="p-0 py-1">
		{#if db.activeConnectionId}
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
		{#if sidebarTab === "schema"}
			{#if db.activeConnection && db.activeConnection.database}
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
											<SidebarMenu class="ms-4 border-s border-sidebar-border ps-2">
												{#each tables as table (table.name)}
													<SidebarMenuItem>
														<SidebarMenuButton onclick={() => handleTableClick(table)}>
															<TableIcon class="size-4" />
															<span class="flex-1">{table.name}</span>
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
			{:else}
				<div class="flex-1 flex items-center justify-center p-4 text-center text-muted-foreground">
					<div>
						<p class="text-sm">{m.sidebar_no_connection()}</p>
						<p class="text-xs mt-1">{m.sidebar_no_connection_schema()}</p>
					</div>
				</div>
			{/if}
		{:else if sidebarTab === "queries"}
			{#if db.activeConnection && db.activeConnection.database}
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
														onclick={() => db.loadQueryFromHistory(item.id)}
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
													<p class="text-xs">{m.sidebar_no_history()}</p>
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
						</SidebarMenu>
					</SidebarGroupContent>
				</SidebarGroup>
			{:else}
				<div class="flex-1 flex items-center justify-center p-4 text-center text-muted-foreground">
					<div>
						<p class="text-sm">{m.sidebar_no_connection()}</p>
						<p class="text-xs mt-1">{m.sidebar_no_connection_queries()}</p>
					</div>
				</div>
			{/if}
		{/if}
	</SidebarContent>

	<SidebarFooter class="p-4">
		<div class="text-xs text-muted-foreground flex justify-between">
			<span>
				{#if db.activeConnection}
					{#if sidebarTab === "schema"}
						{m.sidebar_tables_count({ count: db.activeSchema.length })}
					{:else}
						{m.sidebar_queries_stats({ executed: db.activeConnectionQueryHistory.length, saved: db.activeConnectionSavedQueries.length })}
					{/if}
				{:else}
					{m.sidebar_no_connection_footer()}
				{/if}
			</span>
			{#if version}
				<span>v{version}</span>
			{/if}
		</div>
	</SidebarFooter>

	<SidebarRail />
</Sidebar>
