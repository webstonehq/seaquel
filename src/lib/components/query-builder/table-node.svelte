<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import type { TutorialTable } from "$lib/types";
	import { getTable } from "$lib/tutorial/schema";
	import { useQueryBuilder } from "$lib/hooks/query-builder.svelte.js";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { Button } from "$lib/components/ui/button";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import TableIcon from "@lucide/svelte/icons/table";
	import LinkIcon from "@lucide/svelte/icons/link";

	interface Props {
		id: string;
		data: {
			tableName: string;
			tableId: string;
			selectedColumns: Set<string>;
		};
		isConnectable?: boolean;
	}

	let { id, data, isConnectable = true }: Props = $props();

	const queryBuilder = useQueryBuilder();

	// Get the table schema
	const tableSchema: TutorialTable | undefined = $derived(getTable(data.tableName));

	function handleToggleColumn(columnName: string) {
		queryBuilder.toggleColumn(data.tableId, columnName);
	}

	function handleSelectAll() {
		queryBuilder.selectAllColumns(data.tableId);
	}

	function handleSelectNone() {
		queryBuilder.clearColumns(data.tableId);
	}
</script>

<div class="bg-card border border-border rounded-lg shadow-md overflow-hidden min-w-[220px]">
	<!-- Table Header -->
	<div class="bg-muted/50 border-b border-border px-3 py-2 flex items-center justify-between gap-2">
		<div class="flex items-center gap-2 min-w-0">
			<TableIcon class="size-4 shrink-0 text-muted-foreground" />
			<span class="font-medium text-sm truncate" title={data.tableName}>
				{data.tableName}
			</span>
		</div>

		<div class="flex items-center gap-1 shrink-0">
			<Button
				variant="ghost"
				size="sm"
				class="h-6 px-2 text-xs"
				onclick={handleSelectAll}
			>
				All
			</Button>
			<Button
				variant="ghost"
				size="sm"
				class="h-6 px-2 text-xs"
				onclick={handleSelectNone}
			>
				None
			</Button>
		</div>
	</div>

	<!-- Columns List -->
	{#if tableSchema}
		<ScrollArea class="max-h-[240px]">
			<div class="text-xs divide-y divide-border">
				{#each tableSchema.columns as column (column.name)}
					{@const isSelected = data.selectedColumns.has(column.name)}
					{@const isForeignKey = Boolean(column.foreignKey)}
					<div class="px-3 py-1.5 flex items-center gap-2 hover:bg-muted/30 relative">
						<!-- Left Handle (target) for receiving connections -->
						<Handle
							type="target"
							position={Position.Left}
							id="{column.name}-target"
							{isConnectable}
							class="!size-2 !bg-primary !border-background !-left-1"
						/>

						<!-- Checkbox -->
						<Checkbox
							checked={isSelected}
							onCheckedChange={() => handleToggleColumn(column.name)}
						/>

						<!-- FK Icon or spacer -->
						<div class="w-4 flex items-center justify-center shrink-0">
							{#if isForeignKey}
								<span title="FK -> {column.foreignKey?.table}.{column.foreignKey?.column}">
									<LinkIcon class="size-3 text-blue-500" />
								</span>
							{/if}
						</div>

						<!-- Column name -->
						<span class="flex-1 truncate font-mono" title={column.name}>
							{column.name}
						</span>

						<!-- Column type -->
						<span class="text-muted-foreground shrink-0 font-mono">
							{column.type}
						</span>

						<!-- Right Handle (source) for creating connections -->
						<Handle
							type="source"
							position={Position.Right}
							id="{column.name}-source"
							{isConnectable}
							class="!size-2 !bg-primary !border-background !-right-1"
						/>
					</div>
				{/each}
			</div>
		</ScrollArea>
	{:else}
		<div class="px-3 py-4 text-sm text-muted-foreground text-center">
			Table not found in schema
		</div>
	{/if}
</div>
