<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Separator } from "$lib/components/ui/separator";
	import KeyIcon from "@lucide/svelte/icons/key";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import ListIcon from "@lucide/svelte/icons/list";
	import PencilIcon from "@lucide/svelte/icons/pencil";
	import ArrowUpRightIcon from "@lucide/svelte/icons/arrow-up-right";
	import { m } from "$lib/paraglide/messages.js";

	let { tabId: propTabId = undefined }: { tabId?: string } = $props();

	const db = useDatabase();
	const activeSchemaTab = $derived(
		propTabId
			? db.state.schemaTabs.find(t => t.id === propTabId) ?? null
			: db.state.activeSchemaTab
	);

	const handleViewData = () => {
		if (!activeSchemaTab?.table) return;
		db.dataTabs.add(activeSchemaTab.table);
		db.ui.setActiveView("data");
	};

	const handleEditTable = () => {
		if (!activeSchemaTab?.table) return;
		db.createTableTabs.addFromTable(activeSchemaTab.table);
		db.ui.setActiveView("createTable");
	};

	const handleForeignKeyNavigate = (ref: import("$lib/types").ForeignKeyRef) => {
		const connectionId = db.state.activeConnectionId;
		if (!connectionId) return;
		const schemas = db.state.schemas[connectionId] ?? [];
		const refTable = schemas.find(
			(t) => t.name === ref.referencedTable && t.schema === ref.referencedSchema,
		);
		if (refTable) {
			db.schemaTabs.add(refTable);
		}
	};
</script>

<div class="flex flex-col h-full">
	{#if activeSchemaTab?.table}
		<div class="flex-1 overflow-auto p-4">
			<Card>
				<CardHeader>
					<div class="flex items-start justify-between">
						<div>
							<CardTitle class="flex items-center gap-2">
								<DatabaseIcon class="size-5" />
								{activeSchemaTab.table.name}
							</CardTitle>
							<CardDescription>
								{activeSchemaTab.table.type === "table" ? m.table_viewer_table() : m.table_viewer_view()} • {m.table_viewer_schema({ schema: activeSchemaTab.table.schema })} • {m.table_viewer_rows({ count: activeSchemaTab.table.rowCount?.toLocaleString() ?? "0" })}
							</CardDescription>
						</div>
						<div class="flex items-center gap-2">
							{#if activeSchemaTab.table.type === "table"}
								<Button size="sm" variant="outline" onclick={handleEditTable}>
									<PencilIcon class="size-3 me-1" />
									{m.table_viewer_edit_table()}
								</Button>
							{/if}
							<Button size="sm" onclick={handleViewData}>{m.table_viewer_view_data()}</Button>
						</div>
					</div>
				</CardHeader>
				<CardContent class="space-y-6">
					<div>
						<h3 class="text-sm font-semibold mb-3 flex items-center gap-2">
							<ListIcon class="size-4" />
							{m.table_viewer_columns({ count: activeSchemaTab.table.columns.length })}
						</h3>
						<div class="border rounded-lg overflow-hidden">
							<table class="w-full text-sm">
								<thead class="bg-muted">
									<tr>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_column_name()}</th>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_column_type()}</th>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_column_nullable()}</th>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_column_default()}</th>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_column_keys()}</th>
									</tr>
								</thead>
								<tbody>
									{#each activeSchemaTab.table.columns as column, i}
										<tr class={["border-t hover:bg-muted/50 group/fkrow", i % 2 === 0 && "bg-muted/20"]}>
											<td class="px-4 py-2 font-mono">{column.name}</td>
											<td class="px-4 py-2">
												<Badge variant="outline" class="font-mono text-xs">{column.type}</Badge>
											</td>
											<td class="px-4 py-2">
												{#if column.nullable}
													<Badge variant="secondary" class="text-xs">NULL</Badge>
												{:else}
													<Badge variant="outline" class="text-xs">NOT NULL</Badge>
												{/if}
											</td>
											<td class="px-4 py-2 text-muted-foreground font-mono text-xs">
												{column.defaultValue || "-"}
											</td>
											<td class="px-4 py-2">
												<div class="flex gap-1 items-center">
													{#if column.isPrimaryKey}
														<Badge variant="default" class="text-xs">PK</Badge>
													{/if}
													{#if column.isForeignKey}
														<Badge variant="secondary" class="text-xs">FK</Badge>
														{#if column.foreignKeyRef}
															<button
																class="flex items-center justify-center shrink-0 size-5 rounded hover:bg-muted text-muted-foreground hover:text-foreground transition-colors opacity-0 group-hover/fkrow:opacity-100"
																onclick={() => handleForeignKeyNavigate(column.foreignKeyRef!)}
																title="{column.foreignKeyRef.referencedSchema}.{column.foreignKeyRef.referencedTable}.{column.foreignKeyRef.referencedColumn}"
															>
																<ArrowUpRightIcon class="size-3" />
															</button>
														{/if}
													{/if}
												</div>
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					</div>

					<Separator />

					<div>
						<h3 class="text-sm font-semibold mb-3 flex items-center gap-2">
							<KeyIcon class="size-4" />
							{m.table_viewer_indexes({ count: activeSchemaTab.table.indexes.length })}
						</h3>
						<div class="border rounded-lg overflow-hidden">
							<table class="w-full text-sm">
								<thead class="bg-muted">
									<tr>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_index_name()}</th>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_index_columns()}</th>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_index_type()}</th>
										<th class="px-4 py-2 text-start font-medium">{m.table_viewer_index_unique()}</th>
									</tr>
								</thead>
								<tbody>
									{#each activeSchemaTab.table.indexes as index, i}
										<tr class={["border-t hover:bg-muted/50", i % 2 === 0 && "bg-muted/20"]}>
											<td class="px-4 py-2 font-mono">{index.name}</td>
											<td class="px-4 py-2 font-mono text-xs">{index.columns.join(", ")}</td>
											<td class="px-4 py-2">
												<Badge variant="outline" class="text-xs">{index.type}</Badge>
											</td>
											<td class="px-4 py-2">
												{#if index.unique}
													<Badge variant="default" class="text-xs">UNIQUE</Badge>
												{:else}
													<span class="text-muted-foreground">-</span>
												{/if}
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					</div>
				</CardContent>
			</Card>
		</div>
	{:else}
		<div class="flex-1 flex items-center justify-center text-muted-foreground">
			<div class="text-center">
				<DatabaseIcon class="size-12 mx-auto mb-2 opacity-20" />
				<p class="text-sm">{m.table_viewer_no_selection()}</p>
			</div>
		</div>
	{/if}
</div>
