<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Sidebar, SidebarContent, SidebarFooter, SidebarHeader, SidebarRail, SidebarGroup, SidebarGroupLabel, SidebarGroupContent, SidebarMenu, SidebarMenuItem, SidebarMenuButton } from "$lib/components/ui/sidebar";
	import { Badge } from "$lib/components/ui/badge";
	import { TableIcon, Columns3Icon, KeyIcon, ChevronRightIcon, FolderIcon } from "@lucide/svelte";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";

	const db = useDatabase();
	let expandedSchemas = $state<Set<string>>(new Set());
	let expandedTables = $state<Set<string>>(new Set());

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
</script>

<Sidebar class="top-(--header-height)" collapsible="offcanvas">
	<SidebarContent>
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
																	<!-- <ChevronRightIcon class={["size-4 transition-transform", expandedTables.has(table.name) && "rotate-90"]} /> -->
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
	</SidebarContent>

	<SidebarFooter class="border-t border-sidebar-border p-4">
		<div class="text-xs text-muted-foreground">
			{#if db.activeConnection}
				{db.activeSchema.length} tables
			{:else}
				No connection
			{/if}
		</div>
	</SidebarFooter>

	<SidebarRail />
</Sidebar>
