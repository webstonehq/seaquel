<script lang="ts">
	import EditableCell from "$lib/components/editable-cell.svelte";
	import RowActions from "$lib/components/row-actions.svelte";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import { CopyIcon } from "@lucide/svelte";

	interface Props {
		columns: string[];
		rows: Record<string, unknown>[];
		isEditable: boolean;
		onCellSave: (rowIndex: number, column: string, newValue: string) => Promise<void>;
		onRowDelete: (rowIndex: number, row: Record<string, unknown>) => void;
		deletingRowIndex: number | null;
		onCopyCell: () => void;
		onCopyRow: () => void;
		onCopyColumn: () => void;
		onCellRightClick: (value: unknown, column: string, row: Record<string, unknown>) => void;
	}

	let {
		columns,
		rows,
		isEditable,
		onCellSave,
		onRowDelete,
		deletingRowIndex,
		onCopyCell,
		onCopyRow,
		onCopyColumn,
		onCellRightClick,
	}: Props = $props();

	// Virtual scrolling state
	const ROW_HEIGHT = 37;
	const OVERSCAN = 10;

	let scrollTop = $state(0);
	let containerHeight = $state(0);
	let scrollContainerRef: HTMLDivElement | undefined = $state();

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
</script>

<ContextMenu.Root>
	<ContextMenu.Trigger class="flex-1 overflow-hidden min-h-0 block">
		<div class="flex flex-col h-full">
			<!-- Fixed Header -->
			<div class="shrink-0 bg-muted border-b">
				<table class="w-full text-sm">
					<thead>
						<tr>
							{#if isEditable}
								<th class="px-2 py-2 w-8"></th>
							{/if}
							{#each columns as column}
								<th class="px-4 py-2 text-left font-medium">{column}</th>
							{/each}
						</tr>
					</thead>
				</table>
			</div>

			<!-- Scrollable Virtualized Body -->
			<div
				bind:this={scrollContainerRef}
				class="flex-1 overflow-auto min-h-0"
				onscroll={handleScroll}
			>
				<div style="height: {totalHeight}px; position: relative;">
					{#each visibleRows as { row, index: rowIndex, offset } (rowIndex)}
						<div
							class={["border-b hover:bg-muted/50 flex", rowIndex % 2 === 0 && "bg-muted/20"]}
							style="position: absolute; top: {offset}px; left: 0; right: 0; height: {ROW_HEIGHT}px;"
						>
							{#if isEditable}
								<div class="px-2 py-1 w-8 shrink-0 flex items-center">
									<RowActions
										onDelete={async () => onRowDelete(rowIndex, row)}
										isDeleting={deletingRowIndex === rowIndex}
									/>
								</div>
							{/if}
							{#each columns as column}
								<div
									class="px-4 py-2 flex-1 min-w-0 flex items-center"
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
	</ContextMenu.Trigger>
	<ContextMenu.Portal>
		<ContextMenu.Content class="w-48">
			<ContextMenu.Item onclick={onCopyCell}>
				<CopyIcon class="size-4 mr-2" />
				Copy Cell Value
			</ContextMenu.Item>
			<ContextMenu.Item onclick={onCopyRow}>
				<CopyIcon class="size-4 mr-2" />
				Copy Row as JSON
			</ContextMenu.Item>
			<ContextMenu.Item onclick={onCopyColumn}>
				<CopyIcon class="size-4 mr-2" />
				Copy Column Values
			</ContextMenu.Item>
		</ContextMenu.Content>
	</ContextMenu.Portal>
</ContextMenu.Root>
