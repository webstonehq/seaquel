<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "$lib/components/ui/card";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Separator } from "$lib/components/ui/separator";
	import { KeyIcon, DatabaseIcon, ListIcon } from "@lucide/svelte";
	import { fly } from "svelte/transition";

	const db = useDatabase();

	const handleQueryTable = () => {
		if (!db.activeSchemaTab) return;
		db.addQueryTab(`Query ${db.activeSchemaTab.table.name}`, `SELECT * FROM ${db.activeSchemaTab.table.schema}.${db.activeSchemaTab.table.name} LIMIT 100;`);
		db.setActiveView("query");
	};
</script>

<div class="flex flex-col h-full">
	{#if db.activeSchemaTab}
		<div class="flex-1 overflow-auto p-4" transition:fly={{ x: 20, duration: 200 }}>
			<Card>
				<CardHeader>
					<div class="flex items-start justify-between">
						<div>
							<CardTitle class="flex items-center gap-2">
								<DatabaseIcon class="size-5" />
								{db.activeSchemaTab.table.name}
							</CardTitle>
							<CardDescription>
								{db.activeSchemaTab.table.type === "table" ? "Table" : "View"} • {db.activeSchemaTab.table.schema} schema • {db.activeSchemaTab.table.rowCount?.toLocaleString()} rows
							</CardDescription>
						</div>
						<Button size="sm" onclick={handleQueryTable}>Query Table</Button>
					</div>
				</CardHeader>
				<CardContent class="space-y-6">
					<div>
						<h3 class="text-sm font-semibold mb-3 flex items-center gap-2">
							<ListIcon class="size-4" />
							Columns ({db.activeSchemaTab.table.columns.length})
						</h3>
						<div class="border rounded-lg overflow-hidden">
							<table class="w-full text-sm">
								<thead class="bg-muted">
									<tr>
										<th class="px-4 py-2 text-left font-medium">Name</th>
										<th class="px-4 py-2 text-left font-medium">Type</th>
										<th class="px-4 py-2 text-left font-medium">Nullable</th>
										<th class="px-4 py-2 text-left font-medium">Default</th>
										<th class="px-4 py-2 text-left font-medium">Keys</th>
									</tr>
								</thead>
								<tbody>
									{#each db.activeSchemaTab.table.columns as column, i}
										<tr class={["border-t hover:bg-muted/50", i % 2 === 0 && "bg-muted/20"]}>
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
												<div class="flex gap-1">
													{#if column.isPrimaryKey}
														<Badge variant="default" class="text-xs">PK</Badge>
													{/if}
													{#if column.isForeignKey}
														<Badge variant="secondary" class="text-xs">FK</Badge>
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
							Indexes ({db.activeSchemaTab.table.indexes.length})
						</h3>
						<div class="border rounded-lg overflow-hidden">
							<table class="w-full text-sm">
								<thead class="bg-muted">
									<tr>
										<th class="px-4 py-2 text-left font-medium">Name</th>
										<th class="px-4 py-2 text-left font-medium">Columns</th>
										<th class="px-4 py-2 text-left font-medium">Type</th>
										<th class="px-4 py-2 text-left font-medium">Unique</th>
									</tr>
								</thead>
								<tbody>
									{#each db.activeSchemaTab.table.indexes as index, i}
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
				<p class="text-sm">Select a table from the sidebar to view its schema</p>
			</div>
		</div>
	{/if}
</div>
