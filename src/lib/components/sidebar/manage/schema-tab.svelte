<script lang="ts">
	import { SvelteSet } from "svelte/reactivity";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import TableIcon from "@lucide/svelte/icons/table";
	import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
	import FolderIcon from "@lucide/svelte/icons/folder";
	import SearchIcon from "@lucide/svelte/icons/search";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
	import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
	import EyeIcon from "@lucide/svelte/icons/eye";
	import LayoutGridIcon from "@lucide/svelte/icons/layout-grid";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
	import { m } from "$lib/paraglide/messages.js";

	const db = useDatabase();

	let expandedSchemas = new SvelteSet<string>();
	let schemaSearchQuery = $state("");
	let isRefreshingSchema = $state(false);

	// Drop/Truncate table state. `connectionId` is the connection whose
	// schema the menu was opened on, so the statement runs there even if
	// another connection becomes active before the dialog is confirmed.
	let dropTableTarget = $state<{ connectionId: string; schema: string; name: string; type: "table" | "view" | "materialized-view" } | null>(null);
	let showDropDialog = $state(false);
	let truncateTableTarget = $state<{ connectionId: string; schema: string; name: string } | null>(null);
	let showTruncateDialog = $state(false);

	/** What Core drops (phase 5c, Decision 11): it builds the statement with the dialect. */
	const dropKind = (type: "table" | "view" | "materialized-view") =>
		type === "materialized-view" ? "materializedView" : type;

	const dropKeyword = (type: "table" | "view" | "materialized-view") =>
		type === "materialized-view" ? "MATERIALIZED VIEW" : type === "view" ? "VIEW" : "TABLE";

	const dropLabel = (type: "table" | "view" | "materialized-view") =>
		type === "materialized-view" ? "Materialized View" : type === "view" ? "View" : "Table";

	const handleDropTable = async () => {
		if (!dropTableTarget) return;
		const { connectionId, schema, name, type } = dropTableTarget;
		const keyword = dropKeyword(type);
		showDropDialog = false;
		dropTableTarget = null;
		try {
			const result = await db.queries.dropObject(connectionId, { schema, name }, dropKind(type));
			if (result.queued) {
				const { toast } = await import("svelte-sonner");
				toast.info(`Drop ${keyword.toLowerCase()} "${name}" added to pending changes`);
				return;
			}
			// Close tabs referencing the dropped object
			for (const tab of db.state.schemaTabs) {
				if (tab.connectionId === connectionId && tab.table.name === name && tab.table.schema === schema) {
					db.schemaTabs.remove(tab.id);
				}
			}
			for (const tab of db.state.dataTabs) {
				if (tab.connectionId === connectionId && tab.tableName === name && tab.schemaName === schema) {
					db.dataTabs.remove(tab.id);
				}
			}
			for (const tab of db.state.createTableTabs) {
				const definition = tab.originalDefinition ?? tab.tableDefinition;
				if (
					tab.isEditMode &&
					tab.connectionId === connectionId &&
					tab.name === name &&
					definition.schemaName === schema
				) {
					db.createTableTabs.remove(tab.id);
				}
			}
			await db.connections.refreshSchema(connectionId);
			const { toast } = await import("svelte-sonner");
			toast.success(`${keyword} "${name}" dropped`);
		} catch (error) {
			const { errorToast } = await import("$lib/utils/toast");
			errorToast(`Failed to drop ${keyword.toLowerCase()}: ${error instanceof Error ? error.message : String(error)}`);
		}
	};

	const handleTruncateTable = async () => {
		if (!truncateTableTarget) return;
		const { connectionId, schema, name } = truncateTableTarget;
		showTruncateDialog = false;
		truncateTableTarget = null;
		try {
			// Core truncates SQLite with DELETE FROM.
			const result = await db.queries.truncateTable(connectionId, { schema, name });
			if (result.queued) {
				const { toast } = await import("svelte-sonner");
				toast.info(`Truncate table "${name}" added to pending changes`);
				return;
			}
			const { toast } = await import("svelte-sonner");
			toast.success(`Table "${name}" truncated`);
		} catch (error) {
			const { errorToast } = await import("$lib/utils/toast");
			errorToast(`Failed to truncate table: ${error instanceof Error ? error.message : String(error)}`);
		}
	};

	const toggleSchema = (schemaName: string) => {
		if (expandedSchemas.has(schemaName)) {
			expandedSchemas.delete(schemaName);
		} else {
			expandedSchemas.add(schemaName);
		}
	};

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

	const handleRefreshSchema = async () => {
		if (!db.state.activeConnectionId || isRefreshingSchema) return;
		isRefreshingSchema = true;
		try {
			await db.connections.refreshSchema(db.state.activeConnectionId);
		} catch (error) {
			console.error("Failed to refresh schema:", error);
		} finally {
			isRefreshingSchema = false;
		}
	};

	const handleTableClick = (table: (typeof db.state.activeSchema)[0]) => {
		db.dataTabs.add(table);
		db.ui.setActiveView("data");
	};
</script>

<div class="px-4 py-2">
	<div class="flex items-center gap-1">
		<div class="relative flex-1">
			<SearchIcon class="absolute start-2 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
			<Input
				bind:value={schemaSearchQuery}
				placeholder={m.sidebar_search_tables()}
				class="ps-8 h-8 text-sm"
			/>
		</div>
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button {...props} variant="ghost" size="icon" class="size-8 shrink-0" onclick={handleRefreshSchema} disabled={isRefreshingSchema}>
						<RefreshCwIcon class={["size-4", isRefreshingSchema && "animate-spin"]} />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>{m.sidebar_refresh_schema()}</Tooltip.Content>
		</Tooltip.Root>
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button {...props} variant="ghost" size="icon" class="size-8 shrink-0" onclick={() => {
						db.createTableTabs.add();
						db.ui.setActiveView("createTable");
					}}>
						<PlusIcon class="size-4" />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>{m.sidebar_create_table()}</Tooltip.Content>
		</Tooltip.Root>
	</div>
</div>
<Sidebar.Group>
	<Sidebar.GroupContent class="px-2">
		<Sidebar.Menu>
			{#each [...tablesBySchema.entries()] as [schemaName, tables] (schemaName)}
				<Collapsible open={expandedSchemas.has(schemaName)} onOpenChange={() => toggleSchema(schemaName)}>
					<Sidebar.MenuItem>
						<CollapsibleTrigger>
							{#snippet child({ props })}
								<Sidebar.MenuButton {...props} class="pr-1">
									<ChevronRightIcon class={["size-4 transition-transform", expandedSchemas.has(schemaName) && "rotate-90"]} />
									<FolderIcon class="size-4" />
									<span class="flex-1">{schemaName}</span>
									<Badge variant="secondary" class="text-xs">{tables.length}</Badge>
								</Sidebar.MenuButton>
							{/snippet}
						</CollapsibleTrigger>
						<CollapsibleContent class="flex">
							<Sidebar.Menu class="ms-4 border-s border-sidebar-border ps-2">
								{#each tables as table (table.name)}
									<Sidebar.MenuItem class="group/table-row flex">
										<Sidebar.MenuButton onclick={() => handleTableClick(table)}>
											{#if table.type === "table"}
												<TableIcon class="size-4" />
											{:else}
												<EyeIcon class="size-4" />
											{/if}
											<!-- svelte-ignore a11y_no_static_element_interactions -->
											<span
												class="flex-1 truncate text-left"
												title=""
												onpointerenter={(e: PointerEvent) => {
													const el = e.currentTarget as HTMLElement;
													el.title = el.scrollWidth > el.clientWidth ? table.name : '';
												}}
											>{table.name}</span>
										<DropdownMenu.Root>
											<DropdownMenu.Trigger>
												{#snippet child({ props })}
													<button
														{...props}
														class="end-0 top-1.5 flex size-5 items-center justify-center rounded-md text-sidebar-foreground opacity-0 ring-sidebar-ring transition-opacity hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 focus-visible:outline-hidden group-hover/table-row:opacity-100 data-[state=open]:opacity-100"
													>
														<MoreHorizontalIcon class="size-4" />
													</button>
												{/snippet}
											</DropdownMenu.Trigger>
											<DropdownMenu.Content align="end">
												<DropdownMenu.Item onclick={() => {
													db.schemaTabs.add(table);
													db.ui.setActiveView("schema");
												}}>
													<TableIcon class="size-4 me-2" />
													{m.sidebar_view_schema()}
												</DropdownMenu.Item>
												{#if db.state.activeView === 'workflow' && db.state.activeWorkflowTab}
													<DropdownMenu.Separator />
													<DropdownMenu.Item onclick={() => db.workflow.addTableNode(table)}>
														<LayoutGridIcon class="size-4 me-2" />
														{m.sidebar_add_to_workflow()}
													</DropdownMenu.Item>
												{/if}
												{#if table.type === "table"}
													<DropdownMenu.Separator />
													<DropdownMenu.Item onclick={() => {
														if (!db.state.activeConnectionId) return;
														truncateTableTarget = { connectionId: db.state.activeConnectionId, schema: table.schema, name: table.name };
														showTruncateDialog = true;
													}}>
														<Trash2Icon class="size-4 me-2" />
														{m.sidebar_truncate_table()}
													</DropdownMenu.Item>
												{:else}
													<DropdownMenu.Separator />
												{/if}
												<DropdownMenu.Item class="text-destructive" onclick={() => {
													if (!db.state.activeConnectionId) return;
													dropTableTarget = { connectionId: db.state.activeConnectionId, schema: table.schema, name: table.name, type: table.type };
													showDropDialog = true;
												}}>
													<Trash2Icon class="size-4 me-2" />
													Drop {dropLabel(table.type)}
												</DropdownMenu.Item>
											</DropdownMenu.Content>
										</DropdownMenu.Root>
										</Sidebar.MenuButton>
									</Sidebar.MenuItem>
								{/each}
							</Sidebar.Menu>
						</CollapsibleContent>
					</Sidebar.MenuItem>
				</Collapsible>
			{:else}
				<div class="text-center py-4 text-muted-foreground px-2">
					{#if schemaSearchQuery}
						<p class="text-xs">{m.sidebar_no_schema_search()} <button class="text-foreground underline underline-offset-4 hover:text-primary" onclick={() => { schemaSearchQuery = ""; }}>{m.sidebar_no_schema_clear_search()}</button></p>
					{:else}
						<p class="text-xs">{m.sidebar_no_schema()} <button class="text-foreground underline underline-offset-4 hover:text-primary" onclick={() => {
							db.createTableTabs.add();
							db.ui.setActiveView("createTable");
						}}>{m.sidebar_no_schema_create_table()}</button></p>
					{/if}
				</div>
			{/each}
		</Sidebar.Menu>
	</Sidebar.GroupContent>
</Sidebar.Group>

<DeleteConfirmDialog
	bind:open={showDropDialog}
	title={`Drop ${dropLabel(dropTableTarget?.type ?? "table")}`}
	description={m.drop_table_confirm_description({ schema: dropTableTarget?.schema ?? "", table: dropTableTarget?.name ?? "" })}
	cancelText={m.theme_delete_cancel()}
	confirmText={`Drop ${dropLabel(dropTableTarget?.type ?? "table")}`}
	onconfirm={handleDropTable}
/>

<DeleteConfirmDialog
	bind:open={showTruncateDialog}
	title={m.truncate_table_confirm_title()}
	description={m.truncate_table_confirm_description({ schema: truncateTableTarget?.schema ?? "", table: truncateTableTarget?.name ?? "" })}
	cancelText={m.theme_delete_cancel()}
	confirmText={m.sidebar_truncate_table()}
	onconfirm={handleTruncateTable}
/>
