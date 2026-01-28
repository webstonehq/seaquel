<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import type { QueryBuilderTable, AggregateFunction } from "$lib/types";
	import { useQueryBuilder } from "$lib/hooks/query-builder.svelte.js";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { Button } from "$lib/components/ui/button";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import * as Select from "$lib/components/ui/select";
	import TableIcon from "@lucide/svelte/icons/table";
	import LayersIcon from "@lucide/svelte/icons/layers";
	import LinkIcon from "@lucide/svelte/icons/link";
	import FunctionSquareIcon from "@lucide/svelte/icons/function-square";
	import HashIcon from "@lucide/svelte/icons/hash";
	import SigmaIcon from "@lucide/svelte/icons/sigma";
	import DivideIcon from "@lucide/svelte/icons/divide";
	import ArrowDownIcon from "@lucide/svelte/icons/arrow-down-narrow-wide";
	import ArrowUpIcon from "@lucide/svelte/icons/arrow-up-narrow-wide";

	const AGGREGATE_ICONS: Record<AggregateFunction, typeof HashIcon> = {
		COUNT: HashIcon,
		SUM: SigmaIcon,
		AVG: DivideIcon,
		MIN: ArrowDownIcon,
		MAX: ArrowUpIcon
	};

	interface Props {
		id: string;
		data: {
			tableName: string;
			tableId: string;
			selectedColumns: Set<string>;
			columnAggregates: Map<string, { function: AggregateFunction; alias?: string }>;
			subqueryId?: string; // Present when table is inside a subquery
			cteId?: string; // Present when this is a CTE reference table (in main query) or a table inside a CTE
		};
		isConnectable?: boolean;
	}

	let { id, data, isConnectable = true }: Props = $props();

	const queryBuilder = useQueryBuilder();

	// Check if this table is a CTE reference (not inside a CTE, but referencing one)
	const isCteReference = $derived.by(() => {
		// If cteId is passed directly in data (for subquery tables), use it
		// But only if this is NOT a table inside a CTE (checked by subqueryId presence or top-level check)
		if (data.cteId && data.subqueryId) {
			// This is a CTE reference inside a subquery
			return data.cteId;
		}

		// Find the table in qb.tables to check if it has cteId (top-level CTE references)
		const table = queryBuilder.tables.find((t) => t.id === data.tableId);
		return table?.cteId ? table.cteId : null;
	});

	const AGGREGATE_FUNCTIONS: { value: AggregateFunction | 'none'; label: string }[] = [
		{ value: 'none', label: 'None' },
		{ value: 'COUNT', label: 'COUNT' },
		{ value: 'SUM', label: 'SUM' },
		{ value: 'AVG', label: 'AVG' },
		{ value: 'MIN', label: 'MIN' },
		{ value: 'MAX', label: 'MAX' }
	];

	// Get the table schema - either from actual table or derived from CTE
	const tableSchema = $derived.by((): QueryBuilderTable | undefined => {
		// If this is a CTE reference table, get columns from the CTE
		if (isCteReference) {
			const cteColumns = queryBuilder.getCteColumns(isCteReference);
			if (cteColumns.length > 0) {
				return {
					name: data.tableName,
					columns: cteColumns.map((c) => ({
						name: c.name,
						type: c.type,
						primaryKey: false
					}))
				};
			}
		}

		// Regular table from schema
		return queryBuilder.getSchemaTable(data.tableName);
	});

	function handleToggleColumn(columnName: string) {
		// Check if this is a table inside a CTE (not a CTE reference)
		const isInsideCte = data.cteId && !isCteReference;

		if (isInsideCte) {
			queryBuilder.toggleCteColumn(data.cteId!, data.tableId, columnName);
		} else if (data.subqueryId) {
			queryBuilder.toggleSubqueryColumn(data.subqueryId, data.tableId, columnName);
		} else {
			queryBuilder.toggleColumn(data.tableId, columnName);
		}
	}

	function handleSelectAll() {
		// Check if this is a table inside a CTE (not a CTE reference)
		const isInsideCte = data.cteId && !isCteReference;

		if (isInsideCte) {
			// For CTE tables, toggle each column individually
			if (tableSchema) {
				for (const col of tableSchema.columns) {
					if (!data.selectedColumns.has(col.name)) {
						queryBuilder.toggleCteColumn(data.cteId!, data.tableId, col.name);
					}
				}
			}
		} else if (data.subqueryId) {
			// For subquery tables, toggle each column individually
			const schema = queryBuilder.getSchemaTable(data.tableName);
			if (schema) {
				for (const col of schema.columns) {
					if (!data.selectedColumns.has(col.name)) {
						queryBuilder.toggleSubqueryColumn(data.subqueryId, data.tableId, col.name);
					}
				}
			}
		} else {
			queryBuilder.selectAllColumns(data.tableId);
		}
	}

	function handleSelectNone() {
		// Check if this is a table inside a CTE (not a CTE reference)
		const isInsideCte = data.cteId && !isCteReference;

		if (isInsideCte) {
			// For CTE tables, toggle each selected column off
			for (const colName of data.selectedColumns) {
				queryBuilder.toggleCteColumn(data.cteId!, data.tableId, colName);
			}
		} else if (data.subqueryId) {
			// For subquery tables, toggle each selected column off
			for (const colName of data.selectedColumns) {
				queryBuilder.toggleSubqueryColumn(data.subqueryId, data.tableId, colName);
			}
		} else {
			queryBuilder.clearColumns(data.tableId);
		}
	}

	function handleAggregateChange(columnName: string, value: string) {
		// Check if this is a table inside a CTE (not a CTE reference)
		const isInsideCte = data.cteId && !isCteReference;

		if (isInsideCte) {
			// For CTE tables, update the aggregate in the CTE's inner state
			const cte = queryBuilder.getCte(data.cteId!);
			if (cte) {
				const table = cte.innerQuery.tables.find(t => t.id === data.tableId);
				if (table) {
					if (value === 'none') {
						table.columnAggregates.delete(columnName);
					} else {
						table.columnAggregates.set(columnName, { function: value as AggregateFunction });
					}
					// Trigger reactivity and sync with editor
					queryBuilder.ctes = [...queryBuilder.ctes];
					queryBuilder.customSql = null;
				}
			}
		} else if (data.subqueryId) {
			// For subquery tables, we need to update the aggregate in the subquery's inner state
			const subquery = queryBuilder.getSubquery(data.subqueryId);
			if (subquery) {
				const table = subquery.innerQuery.tables.find(t => t.id === data.tableId);
				if (table) {
					if (value === 'none') {
						table.columnAggregates.delete(columnName);
					} else {
						table.columnAggregates.set(columnName, { function: value as AggregateFunction });
					}
					// Trigger reactivity and sync with editor
					queryBuilder.subqueries = [...queryBuilder.subqueries];
					queryBuilder.customSql = null;
				}
			}
		} else {
			if (value === 'none') {
				queryBuilder.clearColumnAggregate(data.tableId, columnName);
			} else {
				queryBuilder.setColumnAggregate(data.tableId, columnName, value as AggregateFunction);
			}
		}
	}
</script>

<div class="bg-card border rounded-lg shadow-md min-w-[220px] {isCteReference ? 'border-violet-500/30' : 'border-border'}">
	<!-- Table Header -->
	<div class="border-b border-border px-3 py-2 flex items-center justify-between gap-2 {isCteReference ? 'bg-violet-500/10' : 'bg-muted/50'}">
		<div class="flex items-center gap-2 min-w-0">
			{#if isCteReference}
				<LayersIcon class="size-4 shrink-0 text-violet-500" />
			{:else}
				<TableIcon class="size-4 shrink-0 text-muted-foreground" />
			{/if}
			<span class="font-medium text-sm truncate {isCteReference ? 'text-violet-600 dark:text-violet-400' : ''}" title={data.tableName}>
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

						<!-- Aggregate dropdown (fixed size to prevent layout shift) -->
						<div class="w-3.5 h-3.5 shrink-0 flex items-center justify-center">
							{#if isSelected}
								<Select.Root
									type="single"
									value={columnAgg?.function ?? 'none'}
									onValueChange={(value) => {
										if (value) handleAggregateChange(column.name, value);
									}}
								>
									<Select.Trigger
										size="sm"
										class="!h-3.5 !w-3.5 !min-h-0 min-w-0 border-0 shadow-none p-0 rounded-sm [&>svg:last-child]:hidden {columnAgg ? 'bg-primary/15 hover:bg-primary/25' : 'bg-transparent hover:bg-muted'}"
									>
										{#if columnAgg}
											{@const AggIcon = AGGREGATE_ICONS[columnAgg.function]}
											<AggIcon class="size-3 text-primary" />
										{:else}
											<FunctionSquareIcon class="size-3 text-muted-foreground/50" />
										{/if}
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
						</div>

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
