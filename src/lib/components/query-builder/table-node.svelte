<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import type { TutorialTable, AggregateFunction } from "$lib/types";
	import { getTable } from "$lib/tutorial/schema";
	import { useQueryBuilder } from "$lib/hooks/query-builder.svelte.js";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { Button } from "$lib/components/ui/button";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import * as Select from "$lib/components/ui/select";
	import TableIcon from "@lucide/svelte/icons/table";
	import LinkIcon from "@lucide/svelte/icons/link";

	interface Props {
		id: string;
		data: {
			tableName: string;
			tableId: string;
			selectedColumns: Set<string>;
			columnAggregates: Map<string, { function: AggregateFunction; alias?: string }>;
		};
		isConnectable?: boolean;
	}

	let { id, data, isConnectable = true }: Props = $props();

	const queryBuilder = useQueryBuilder();

	const AGGREGATE_FUNCTIONS: { value: AggregateFunction | 'none'; label: string }[] = [
		{ value: 'none', label: 'None' },
		{ value: 'COUNT', label: 'COUNT' },
		{ value: 'SUM', label: 'SUM' },
		{ value: 'AVG', label: 'AVG' },
		{ value: 'MIN', label: 'MIN' },
		{ value: 'MAX', label: 'MAX' }
	];

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

	function handleAggregateChange(columnName: string, value: string) {
		if (value === 'none') {
			queryBuilder.clearColumnAggregate(data.tableId, columnName);
		} else {
			queryBuilder.setColumnAggregate(data.tableId, columnName, value as AggregateFunction);
		}
	}
</script>

<div class="bg-card border border-border rounded-lg shadow-md min-w-[220px]">
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
		<ScrollArea class="max-h-[240px] overflow-x-visible" orientation="vertical">
			<div class="text-xs divide-y divide-border">
				{#each tableSchema.columns as column (column.name)}
					{@const isSelected = data.selectedColumns.has(column.name)}
					{@const isForeignKey = Boolean(column.foreignKey)}
					{@const columnAgg = data.columnAggregates.get(column.name)}
					<div class="px-3 py-1.5 flex items-center gap-2 hover:bg-muted/30 relative">
						<!-- Left Handle (target) for receiving connections -->
						<Handle
							type="target"
							position={Position.Left}
							id="{column.name}-target"
							{isConnectable}
						/>

						<!-- Checkbox -->
						<Checkbox
							checked={isSelected}
							onCheckedChange={() => handleToggleColumn(column.name)}
						/>

						<!-- Aggregate dropdown (only for selected columns) -->
						{#if isSelected}
							<Select.Root
								type="single"
								value={columnAgg?.function ?? 'none'}
								onValueChange={(value) => {
									if (value) handleAggregateChange(column.name, value);
								}}
							>
								<Select.Trigger size="sm" class="h-5 w-14 text-[10px] px-1">
									{columnAgg?.function ?? 'col'}
								</Select.Trigger>
								<Select.Content>
									{#each AGGREGATE_FUNCTIONS as fn}
										<Select.Item value={fn.value} label={fn.label}>
											{fn.label}
										</Select.Item>
									{/each}
								</Select.Content>
							</Select.Root>
						{/if}

						<!-- FK Icon or spacer -->
						<div class="w-4 flex items-center justify-center shrink-0">
							{#if isForeignKey}
								<span title="FK -> {column.foreignKey?.table}.{column.foreignKey?.column}">
									<LinkIcon class="size-3 text-blue-500" />
								</span>
							{/if}
						</div>

						<!-- Column name -->
						<span class="flex-1 truncate font-mono" class:text-primary={columnAgg} title={column.name}>
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
