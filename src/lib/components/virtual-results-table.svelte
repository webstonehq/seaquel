<script lang="ts">
	import EditableCell from "$lib/components/editable-cell.svelte";
	import RowActions from "$lib/components/row-actions.svelte";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import { CopyIcon } from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		columns: string[];
		rows: Record<string, unknown>[];
		isEditable?: boolean;
		onCellSave?: (rowIndex: number, column: string, newValue: string) => Promise<void>;
		onRowDelete?: (rowIndex: number, row: Record<string, unknown>) => void;
		deletingRowIndex?: number | null;
		onCopyCell?: () => void;
		onCopyRow?: () => void;
		onCopyColumn?: () => void;
		onCellRightClick?: (value: unknown, column: string, row: Record<string, unknown>) => void;
		/** Compact mode for canvas nodes - smaller row height */
		compact?: boolean;
	}

	let {
		columns,
		rows,
		isEditable = false,
		onCellSave = async () => {},
		onRowDelete = () => {},
		deletingRowIndex = null,
		onCopyCell = () => {},
		onCopyRow = () => {},
		onCopyColumn = () => {},
		onCellRightClick = () => {},
		compact = false,
	}: Props = $props();

	// Virtual scrolling state
	const ROW_HEIGHT = $derived(compact ? 28 : 37);
	const HEADER_HEIGHT = $derived(compact ? 28 : 37);
	const OVERSCAN = 10;
	const DEFAULT_COLUMN_WIDTH = $derived(compact ? 100 : 150);
	const MIN_COLUMN_WIDTH = 50;

	let scrollTop = $state(0);
	let containerHeight = $state(0);
	let scrollContainerRef: HTMLDivElement | undefined = $state();

	// Column widths state - initialize with default width for each column
	let columnWidths = $state<number[]>([]);

	// Initialize column widths when columns change
	$effect(() => {
		if (columns.length !== columnWidths.length) {
			columnWidths = columns.map(() => DEFAULT_COLUMN_WIDTH);
		}
	});

	// Resize state
	let resizingIndex = $state<number | null>(null);
	let resizeStartX = $state(0);
	let resizeStartWidth = $state(0);

	// Calculate visible range
	const totalHeight = $derived(rows.length * ROW_HEIGHT);
	const startIndex = $derived(Math.max(0, Math.floor(scrollTop / ROW_HEIGHT) - OVERSCAN));
	const visibleCount = $derived(Math.ceil(containerHeight / ROW_HEIGHT) + OVERSCAN * 2);
	const endIndex = $derived(Math.min(rows.length, startIndex + visibleCount));

	const visibleRows = $derived(
		rows.slice(startIndex, endIndex).map((row, i) => ({
			row,
			index: startIndex + i,
			offset: (startIndex + i) * ROW_HEIGHT
		}))
	);

	// Grid template for consistent column widths across all rows
	const gridTemplate = $derived(
		(isEditable ? '40px ' : '') + columnWidths.map(w => `${w}px`).join(' ')
	);

	function handleScroll(e: Event) {
		const target = e.target as HTMLDivElement;
		scrollTop = target.scrollTop;
	}

	// Track container height
	$effect(() => {
		if (scrollContainerRef) {
			const observer = new ResizeObserver((entries) => {
				containerHeight = entries[0]?.contentRect.height ?? 0;
			});
			observer.observe(scrollContainerRef);
			return () => observer.disconnect();
		}
	});

	// Column resize handlers
	function startResize(index: number, e: MouseEvent) {
		e.preventDefault();
		resizingIndex = index;
		resizeStartX = e.clientX;
		resizeStartWidth = columnWidths[index];

		document.addEventListener('mousemove', handleResize);
		document.addEventListener('mouseup', stopResize);
		document.body.style.cursor = 'col-resize';
		document.body.style.userSelect = 'none';
	}

	function handleResize(e: MouseEvent) {
		if (resizingIndex === null) return;

		const delta = e.clientX - resizeStartX;
		const newWidth = Math.max(MIN_COLUMN_WIDTH, resizeStartWidth + delta);
		columnWidths[resizingIndex] = newWidth;
	}

	function stopResize() {
		resizingIndex = null;
		document.removeEventListener('mousemove', handleResize);
		document.removeEventListener('mouseup', stopResize);
		document.body.style.cursor = '';
		document.body.style.userSelect = '';
	}
</script>

{#snippet tableContent()}
	<!-- Single scroll container for both header and body -->
	<div
		bind:this={scrollContainerRef}
		class={["h-full overflow-auto", compact && "nowheel"]}
		onscroll={handleScroll}
	>
		<!-- Inner wrapper that can expand horizontally -->
		<div class="min-w-fit">
			<!-- Sticky Header -->
			<div
				class={["grid bg-muted border-b sticky top-0 z-10", compact ? "text-xs" : "text-sm"]}
				style="height: {HEADER_HEIGHT}px; grid-template-columns: {gridTemplate};"
			>
				{#if isEditable}
					<div class={compact ? "px-1 py-1" : "px-2 py-2"} class:flex={true} class:items-center={true}></div>
				{/if}
				{#each columns as column, i}
					<div class="relative flex items-center group">
						<div class={["font-medium flex-1 truncate", compact ? "px-2 py-1" : "px-4 py-2"]}>{column}</div>
						<!-- Resize handle -->
						<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
						<div
							class="absolute right-0 top-0 bottom-0 w-1 cursor-col-resize hover:bg-primary/50 group-hover:bg-border transition-colors"
							onmousedown={(e) => startResize(i, e)}
							role="separator"
							aria-orientation="vertical"
						></div>
					</div>
				{/each}
			</div>

			<!-- Virtual Body -->
			<div style="height: {totalHeight}px; position: relative;">
				{#each visibleRows as { row, index: rowIndex, offset } (rowIndex)}
					<div
						class={["border-b hover:bg-muted/50 grid", compact ? "text-xs" : "text-sm", rowIndex % 2 === 0 && "bg-muted/20"]}
						style="position: absolute; top: {offset}px; left: 0; right: 0; height: {ROW_HEIGHT}px; grid-template-columns: {gridTemplate};"
					>
						{#if isEditable}
							<div class={["flex items-center", compact ? "px-1 py-0.5" : "px-2 py-1"]}>
								<RowActions
									onDelete={async () => onRowDelete(rowIndex, row)}
									isDeleting={deletingRowIndex === rowIndex}
								/>
							</div>
						{/if}
						{#each columns as column}
							<!-- svelte-ignore a11y_no_static_element_interactions -->
							<div
								class={["flex items-center overflow-hidden", compact ? "px-2 py-1" : "px-4 py-2"]}
								oncontextmenu={() => onCellRightClick(row[column], column, row)}
							>
								<EditableCell
									value={row[column]}
									{isEditable}
									onSave={(newValue) => onCellSave(rowIndex, column, newValue)}
								/>
							</div>
						{/each}
					</div>
				{/each}
			</div>
		</div>
	</div>
{/snippet}

<ContextMenu.Root>
	<ContextMenu.Trigger class="flex-1 overflow-auto min-h-0 block">
		{@render tableContent()}
	</ContextMenu.Trigger>
	<ContextMenu.Portal>
		<ContextMenu.Content class="w-48">
			<ContextMenu.Item onclick={onCopyCell}>
				<CopyIcon class="size-4 me-2" />
				{m.query_copy_cell()}
			</ContextMenu.Item>
			<ContextMenu.Item onclick={onCopyRow}>
				<CopyIcon class="size-4 me-2" />
				{m.query_copy_row()}
			</ContextMenu.Item>
			<ContextMenu.Item onclick={onCopyColumn}>
				<CopyIcon class="size-4 me-2" />
				{m.query_copy_column()}
			</ContextMenu.Item>
		</ContextMenu.Content>
	</ContextMenu.Portal>
</ContextMenu.Root>
