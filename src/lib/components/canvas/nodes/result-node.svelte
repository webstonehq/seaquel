<script lang="ts">
	import { Handle, Position, NodeResizer } from "@xyflow/svelte";
	import type { CanvasResultNodeData } from "$lib/types/canvas";
	import { useCanvasNode } from "./use-canvas-node.svelte.js";
	import SuggestiveHandle from "../suggestive-handle.svelte";
	import VirtualResultsTable from "$lib/components/virtual-results-table.svelte";
	import TableIcon from "@lucide/svelte/icons/table-2";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import AlertCircleIcon from "@lucide/svelte/icons/alert-circle";
	import CheckCircleIcon from "@lucide/svelte/icons/check-circle";
	import ChartBarIcon from "@lucide/svelte/icons/chart-bar";
	import ChevronLeftIcon from "@lucide/svelte/icons/chevron-left";
	import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
	import { Button } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
	import MoreHorizontalIcon from "@lucide/svelte/icons/more-horizontal";
	import { copyCell as clipboardCopyCell, copyRowAsJSON as clipboardCopyRowAsJSON, copyColumn as clipboardCopyColumn } from "$lib/utils/clipboard";

	interface Props {
		id: string;
		data: CanvasResultNodeData;
		isConnectable?: boolean;
		selected?: boolean;
	}

	let { id, data, isConnectable = true, selected = false }: Props = $props();

	const { db, handleRemove, handleResizeEnd } = useCanvasNode(() => id);

	// Pagination state
	const pageSize = 50;
	let currentPage = $state(1);
	const totalPages = $derived(Math.ceil(data.rows.length / pageSize));
	const paginatedRows = $derived(
		data.rows.slice((currentPage - 1) * pageSize, currentPage * pageSize)
	);

	// Context menu state for copying
	let contextCell = $state<{ value: unknown; column: string; row: Record<string, unknown> } | null>(null);

	const handleCellRightClick = (value: unknown, column: string, row: Record<string, unknown>) => {
		contextCell = { value, column, row };
	};

	const copyCell = async () => {
		if (!contextCell) return;
		await clipboardCopyCell(contextCell.value);
	};

	const copyRowAsJSON = async () => {
		if (!contextCell) return;
		await clipboardCopyRowAsJSON(contextCell.row);
	};

	const copyColumn = async () => {
		if (!contextCell) return;
		await clipboardCopyColumn(contextCell.column, paginatedRows);
	};

	function handleViewAsChart() {
		db.canvas.addChartNode(id, data.columns, data.rows);
	}

	function goToPage(page: number) {
		currentPage = Math.max(1, Math.min(page, totalPages));
	}

	// Suggestions for the output handle (only if we have data)
	const outputSuggestions = $derived(
		data.rows.length > 0 && !data.error
			? [
					{
						label: "Create Chart",
						icon: ChartBarIcon,
						action: handleViewAsChart,
					},
				]
			: []
	);
</script>

<div
	class="bg-card border border-border rounded-lg shadow-lg overflow-hidden h-full flex flex-col"
>
	<NodeResizer
		minWidth={280}
		minHeight={150}
		isVisible={selected}
		lineClass="!border-primary"
		handleClass="!bg-primary !border-primary"
		onResizeEnd={handleResizeEnd}
	/>

	<!-- Input handle -->
	<Handle
		type="target"
		position={Position.Left}
		{isConnectable}
		id="input"
		class="!w-3 !h-3 !bg-blue-500 !border-2 !border-background"
	/>

	<!-- Output handle for connecting to chart nodes -->
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
			<TableIcon class="size-4 text-muted-foreground shrink-0" />
			<span class="font-medium text-sm">Results</span>
			{#if !data.error}
				<CheckCircleIcon class="size-3.5 text-green-500" />
			{/if}
		</div>

		<div class="flex items-center gap-2 shrink-0">
			{#if data.executionTime !== undefined}
				<span class="text-xs text-muted-foreground">
					{data.executionTime.toFixed(0)}ms
				</span>
			{/if}
			{#if data.totalRows !== undefined}
				<span class="text-xs text-muted-foreground">
					{data.totalRows.toLocaleString()} rows
				</span>
			{/if}

			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					<Button variant="ghost" size="icon" class="size-7">
						<MoreHorizontalIcon class="size-4" />
					</Button>
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end">
					<DropdownMenu.Item onclick={handleViewAsChart} disabled={data.rows.length === 0}>
						<ChartBarIcon class="size-4 mr-2" />
						View as Chart
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

	<!-- Content - nowheel prevents canvas zoom when scrolling -->
	<div class="flex-1 min-h-0 overflow-auto nowheel">
		{#if data.error}
			<div class="p-3 flex items-start gap-2 bg-destructive/10">
				<AlertCircleIcon class="size-4 text-destructive shrink-0 mt-0.5" />
				<span class="text-sm text-destructive">{data.error}</span>
			</div>
		{:else if data.rows.length === 0}
			<div class="p-4 text-sm text-muted-foreground text-center">
				No results returned
			</div>
		{:else}
			<VirtualResultsTable
				columns={data.columns}
				rows={paginatedRows}
				compact
				onCopyCell={copyCell}
				onCopyRow={copyRowAsJSON}
				onCopyColumn={copyColumn}
				onCellRightClick={handleCellRightClick}
			/>
		{/if}
	</div>

	<!-- Pagination -->
	{#if totalPages > 1 && !data.error}
		<div class="flex items-center justify-between px-3 py-1.5 border-t border-border bg-muted/30 text-xs">
			<span class="text-muted-foreground">
				{((currentPage - 1) * pageSize + 1).toLocaleString()}-{Math.min(currentPage * pageSize, data.rows.length).toLocaleString()} of {data.rows.length.toLocaleString()}
			</span>
			<div class="flex items-center gap-1">
				<Button
					variant="ghost"
					size="icon"
					class="size-6"
					onclick={() => goToPage(currentPage - 1)}
					disabled={currentPage === 1}
				>
					<ChevronLeftIcon class="size-3" />
				</Button>
				<span class="px-1">{currentPage} / {totalPages}</span>
				<Button
					variant="ghost"
					size="icon"
					class="size-6"
					onclick={() => goToPage(currentPage + 1)}
					disabled={currentPage === totalPages}
				>
					<ChevronRightIcon class="size-3" />
				</Button>
			</div>
		</div>
	{/if}
</div>
