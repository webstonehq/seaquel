<script lang="ts">
	import { Position, NodeResizer } from "@xyflow/svelte";
	import type { CanvasTableNodeData } from "$lib/types/canvas";
	import { useCanvasNode } from "./use-canvas-node.svelte.js";
	import SuggestiveHandle from "../suggestive-handle.svelte";
	import TableIcon from "@lucide/svelte/icons/table";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import KeyIcon from "@lucide/svelte/icons/key";
	import LinkIcon from "@lucide/svelte/icons/link";
	import CodeIcon from "@lucide/svelte/icons/code";
	import { Button } from "$lib/components/ui/button";
	import { Badge } from "$lib/components/ui/badge";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
	import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";

	interface Props {
		id: string;
		data: CanvasTableNodeData;
		isConnectable?: boolean;
		selected?: boolean;
	}

	let { id, data, isConnectable = true, selected = false }: Props = $props();

	const { db, handleRemove, handleResizeEnd } = useCanvasNode(() => id);

	const maxVisibleColumns = 12;
	const visibleColumns = $derived(data.columns.slice(0, maxVisibleColumns));
	const hiddenColumnCount = $derived(data.columns.length - maxVisibleColumns);

	function handleQueryTable() {
		const type = db.state.activeConnection?.type;
		let query: string;

		if (type === 'mssql') {
			// MS SQL Server uses TOP and bracket identifiers
			query = `SELECT TOP 100 * FROM [${data.schemaName}].[${data.tableName}]`;
		} else {
			// PostgreSQL, MySQL, MariaDB, SQLite, DuckDB use LIMIT
			query = `SELECT * FROM "${data.schemaName}"."${data.tableName}" LIMIT 100`;
		}

		const queryNodeId = db.canvas.addQueryNode(query);
		// Connect table node to query node
		db.canvas.connect(id, queryNodeId, "output", "input");
	}

	// Suggestions for the output handle
	const outputSuggestions = $derived([
		{
			label: "Query Table",
			icon: CodeIcon,
			action: handleQueryTable,
		},
	]);
</script>

<div
	class="bg-card border border-border rounded-lg shadow-lg overflow-hidden h-full flex flex-col"
>
	<NodeResizer
		minWidth={220}
		minHeight={120}
		isVisible={selected}
		lineClass="!border-primary"
		handleClass="!bg-primary !border-primary"
		onResizeEnd={handleResizeEnd}
	/>

	<!-- Output handle for connecting to query nodes -->
	<SuggestiveHandle
		nodeId={id}
		type="source"
		position={Position.Right}
		{isConnectable}
		id="output"
		suggestions={outputSuggestions}
		class="!w-3 !h-3 !bg-primary !border-2 !border-background"
	/>

	<!-- Header -->
	<div class="bg-muted/50 border-b border-border px-3 py-2 flex items-center justify-between">
		<div class="flex items-center gap-2 min-w-0">
			<TableIcon class="size-4 shrink-0 text-muted-foreground" />
			<span class="font-medium text-sm truncate" title="{data.schemaName}.{data.tableName}">
				{data.tableName}
			</span>
		</div>

		<div class="flex items-center gap-1 shrink-0">
			<span class="text-xs text-muted-foreground">
				{data.schemaName}
			</span>

			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					<Button variant="ghost" size="icon" class="size-6">
						<MoreHorizontalIcon class="size-4" />
					</Button>
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end">
					<DropdownMenu.Item onclick={handleQueryTable}>
						<CodeIcon class="size-4 mr-2" />
						Query Table
					</DropdownMenu.Item>
					<DropdownMenu.Separator />
					<DropdownMenu.Item onclick={handleRemove} class="text-destructive">
						<Trash2Icon class="size-4 mr-2" />
						Remove
					</DropdownMenu.Item>
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
	</div>

	<!-- Metadata bar -->
	<div class="px-3 py-1.5 bg-muted/50 border-b border-border flex items-center gap-3 text-xs text-muted-foreground">
		<span>{data.tableType === "view" ? "View" : "Table"}</span>
		<span>{data.columns.length} columns</span>
		{#if data.rowCount !== undefined}
			<span>{data.rowCount.toLocaleString()} rows</span>
		{/if}
	</div>

	<!-- Columns list - nowheel prevents canvas zoom when scrolling -->
	<div class="flex-1 min-h-0 overflow-auto nowheel">
		<div class="text-xs divide-y divide-border">
			{#each visibleColumns as column}
				<div class="px-3 py-1.5 flex items-center gap-2 hover:bg-muted/30">
					<!-- Key indicators -->
					<div class="flex items-center gap-0.5 shrink-0 w-6">
						{#if column.isPrimaryKey}
							<KeyIcon class="size-3 text-amber-500" />
						{/if}
						{#if column.isForeignKey}
							<span
								title={column.foreignKeyRef
									? `FK → ${column.foreignKeyRef.referencedSchema}.${column.foreignKeyRef.referencedTable}.${column.foreignKeyRef.referencedColumn}`
									: "Foreign Key"}
							>
								<LinkIcon class="size-3 text-blue-500" />
							</span>
						{/if}
					</div>

					<!-- Column name -->
					<span class="flex-1 truncate font-mono" title={column.name}>
						{column.name}
					</span>

					<!-- Column type -->
					<Badge variant="outline" class="font-mono text-[10px] h-5 shrink-0">
						{column.type.length > 15 ? column.type.slice(0, 12) + "..." : column.type}
					</Badge>

					<!-- Nullable indicator -->
					{#if column.nullable}
						<span class="text-muted-foreground/60 shrink-0 text-[10px]">NULL</span>
					{/if}
				</div>
			{/each}

			{#if hiddenColumnCount > 0}
				<div class="px-3 py-1.5 text-center text-muted-foreground italic">
					+{hiddenColumnCount} more columns
				</div>
			{/if}
		</div>
	</div>

	<!-- Indexes section (if any) -->
	{#if data.indexes.length > 0}
		<div class="border-t border-border">
			<div class="px-3 py-1.5 bg-muted/30 flex items-center gap-2 text-xs text-muted-foreground">
				<KeyIcon class="size-3" />
				<span>{data.indexes.length} indexes</span>
			</div>
			<div class="text-xs divide-y divide-border max-h-[80px] overflow-auto nowheel">
				{#each data.indexes.slice(0, 3) as index}
					<div class="px-3 py-1 flex items-center gap-2 hover:bg-muted/30">
						<span class="truncate font-mono flex-1" title={index.name}>{index.name}</span>
						{#if index.unique}
							<Badge variant="secondary" class="text-[10px] h-4">UNIQUE</Badge>
						{/if}
					</div>
				{/each}
				{#if data.indexes.length > 3}
					<div class="px-3 py-1 text-center text-muted-foreground italic">
						+{data.indexes.length - 3} more
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>
