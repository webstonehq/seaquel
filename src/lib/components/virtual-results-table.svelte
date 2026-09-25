<script lang="ts">
	import EditableCell from "$lib/components/editable-cell.svelte";
	import RowActions from "$lib/components/row-actions.svelte";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import { CopyIcon, CircleOffIcon, RotateCcwIcon, ArrowUpIcon, ArrowDownIcon, PlusIcon, XIcon, KeyRoundIcon, ArrowUpRightIcon } from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { detectColumnTypes, getFormattedCellText, type BinaryStringEncoding } from "$lib/utils/cell-type";
	import { cellText } from "$lib/values";
	import type { ForeignKeyRef, SchemaTable } from "$lib/types";

	interface Props {
		columns: string[];
		/** Columnar rows — `rows[i][j]` is the value in column `columns[j]`. */
		rows: unknown[][];
		isEditable?: boolean;
		onCellSave?: (rowIndex: number, column: string, newValue: unknown) => Promise<void>;
		onRowDelete?: (rowIndex: number, row: unknown[]) => void;
		deletingRowIndex?: number | null;
		onCopyCell?: () => void;
		onCopyRow?: () => void;
		onCopyColumn?: () => void;
		onCellRightClick?: (value: unknown, column: string, row: unknown[], rowIndex: number) => void;
		onSetNull?: () => Promise<void>;
		onSetDefault?: () => Promise<void>;
		/** Compact mode for canvas nodes - smaller row height */
		compact?: boolean;
		/** Called when a column header is clicked (for sorting) */
		onColumnHeaderClick?: (column: string) => void;
		/** Currently sorted column name */
		sortColumn?: string | null;
		/** Current sort direction */
		sortDirection?: "ASC" | "DESC" | null;
		/** Called when the "Add Row" element is clicked */
		onAddRow?: () => void;
		/** Snippet rendered inline between data rows and the "Add Row" link. Receives gridTemplate string. */
		pendingRowsContent?: import('svelte').Snippet<[string]>;
		/** Map of "rowIndex:column" keys to new values for pending cell edits */
		pendingCellEdits?: Map<string, unknown>;
		/** Set of row indices with pending deletes */
		pendingRowDeletes?: Set<number>;
		/** Pending insert rows to display at the bottom of the table */
		pendingInsertRows?: { id: string; values: Record<string, unknown> }[];
		/** Called to remove a pending insert row by its change ID */
		onRemovePendingInsert?: (changeId: string) => void;
		/** Map of column names to their FK reference and resolved target table */
		foreignKeyColumns?: Map<string, { ref: ForeignKeyRef; table: SchemaTable }>;
		/** Called when a FK cell's navigation icon is clicked */
		onForeignKeyClick?: (ref: ForeignKeyRef, table: SchemaTable, value: string) => void;
		/** Map of column name → declared SQL type (e.g. "BOOLEAN", "varchar(255)"). Used to classify cells even when value sampling would misidentify them. */
		declaredColumnTypes?: Record<string, string>;
		/** How string cells in binary columns hold their bytes (`binaryStringEncoding(connection type)`). */
		binaryStrings?: BinaryStringEncoding;
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
		onSetNull = async () => {},
		onSetDefault = async () => {},
		compact = false,
		onColumnHeaderClick,
		sortColumn = null,
		sortDirection = null,
		onAddRow,
		pendingRowsContent,
		pendingCellEdits,
		pendingRowDeletes,
		pendingInsertRows = [],
		onRemovePendingInsert,
		foreignKeyColumns,
		onForeignKeyClick,
		declaredColumnTypes,
		binaryStrings = "text",
	}: Props = $props();

	// Column type detection for formatted cells. The query editor passes no
	// declared types, so there a VARBINARY value that is clean UTF-8 (MySQL
	// sends those as text) is classified by its value and shows as text; only
	// real bytes (Uint8Array) show as binary. The data viewer passes the table's
	// declared types and shows it as bytes.
	const columnTypes = $derived(detectColumnTypes(columns, rows, declaredColumnTypes));

	// Virtual scrolling state
	const ROW_HEIGHT = $derived(compact ? 28 : 37);
	const HEADER_HEIGHT = $derived(compact ? 28 : 37);
	const OVERSCAN = 10;
	const DEFAULT_COLUMN_WIDTH = $derived(compact ? 100 : 150);
	const MIN_COLUMN_WIDTH = $derived(onAddRow ? 120 : 50);

	// WebKit clamps rendered element heights at 2^25 = 33,554,432 pixels. At
	// ROW_HEIGHT=37 that's only 906,876 rows before the spacer div gets silently
	// truncated and you can't scroll further. We cap the rendered spacer just
	// under that and use a scale factor below to stretch the scrollable range
	// across arbitrarily large row counts.
	const MAX_SAFE_HEIGHT = 33_000_000;

	let scrollTop = $state(0);
	let containerHeight = $state(0);

	// Column widths state - initialize with auto-calculated widths
	let columnWidths = $state<number[]>([]);

	const MAX_COLUMN_WIDTH = 400;
	// Approximate character width in pixels (text-sm = 14px, ~8px per char for sans-serif, ~8.4px for mono)
	const CHAR_WIDTH = $derived(compact ? 7 : 8);
	const MONO_CHAR_WIDTH = $derived(compact ? 7.5 : 8.4);
	const CELL_PADDING = $derived(compact ? 16 : 32); // px-2*2 or px-4*2

	function autoSizeColumn(colIdx: number): number {
		const col = columns[colIdx];
		const type = columnTypes[col];
		const isMono = type === 'integer' || type === 'float' || type === 'uuid';
		const charW = isMono ? MONO_CHAR_WIDTH : CHAR_WIDTH;

		let maxLen = col.length;
		const sampleRows = rows.slice(0, 100);
		for (const row of sampleRows) {
			const text = getFormattedCellText(row[colIdx], type, binaryStrings);
			if (text.length > maxLen) maxLen = text.length;
		}

		const width = Math.ceil(maxLen * charW) + CELL_PADDING;
		return Math.max(MIN_COLUMN_WIDTH, Math.min(MAX_COLUMN_WIDTH, width));
	}

	// Initialize column widths when columns change, then re-measure as the
	// rows array grows (streaming queries fill in rows over time). We use a
	// geometric backoff so a 10M-row stream only triggers ~log₄(10M) ≈ 12
	// re-measurements total. Re-measures grow columns only — never shrink —
	// so the user's manual resizes aren't clobbered.
	let lastSizedRowCount = $state(0);
	$effect(() => {
		if (columns.length !== columnWidths.length) {
			columnWidths = columns.map((_col, i) => autoSizeColumn(i));
			lastSizedRowCount = rows.length;
			return;
		}
		if (rows.length >= Math.max(100, lastSizedRowCount * 4)) {
			const fresh = columns.map((_col, i) => autoSizeColumn(i));
			columnWidths = columnWidths.map((w, i) => Math.max(w, fresh[i]));
			lastSizedRowCount = rows.length;
		}
	});

	// Track which cell has an active textarea overlay
	let textareaCell = $state<string | null>(null);

	// Resize state
	let resizingIndex = $state<number | null>(null);
	let resizeStartX = $state(0);
	let resizeStartWidth = $state(0);

	// Calculate visible range.
	//
	// Two coordinate systems are in play:
	//   * "virtual" — what the list logically looks like. Any row i sits at
	//     virtual y = i * ROW_HEIGHT. Can grow to hundreds of millions of px.
	//   * "display" — what actually lives in the DOM. The spacer div is
	//     capped at MAX_SAFE_HEIGHT so WebKit won't clamp it.
	//
	// When the logical list is small enough to fit under the cap, the two
	// spaces are identical (scaleFactor = 1) and this behaves exactly like
	// the previous implementation. When the list exceeds the cap we compress
	// the display scroll range and multiply incoming scrollTop values up to
	// recover the virtual position.
	const actualTotalHeight = $derived(rows.length * ROW_HEIGHT);
	const totalHeight = $derived(Math.min(actualTotalHeight, MAX_SAFE_HEIGHT));
	const scaleFactor = $derived.by(() => {
		if (actualTotalHeight <= MAX_SAFE_HEIGHT) return 1;
		const displayScrollable = totalHeight - containerHeight;
		if (displayScrollable <= 0) return 1;
		const virtualScrollable = actualTotalHeight - containerHeight;
		return Math.max(1, virtualScrollable / displayScrollable);
	});
	const virtualScrollTop = $derived(scrollTop * scaleFactor);

	const startIndex = $derived(
		Math.max(0, Math.floor(virtualScrollTop / ROW_HEIGHT) - OVERSCAN)
	);
	const visibleCount = $derived(Math.ceil(containerHeight / ROW_HEIGHT) + OVERSCAN * 2);
	const endIndex = $derived(Math.min(rows.length, startIndex + visibleCount));

	// Row positions are computed in track-relative coordinates (the spacer
	// div's frame). For each visible row we want it rendered at viewport y
	// `index * ROW_HEIGHT - virtualScrollTop`, which in track coordinates is
	// `scrollTop + viewport y`. When scaleFactor === 1 this collapses to the
	// standard `index * ROW_HEIGHT` formula.
	const visibleRows = $derived(
		rows.slice(startIndex, endIndex).map((row, i) => {
			const index = startIndex + i;
			return {
				row,
				index,
				offset: scrollTop + index * ROW_HEIGHT - virtualScrollTop
			};
		})
	);

	// Grid template for consistent column widths across all rows
	const gridTemplate = $derived(
		(isEditable ? '40px ' : '') + columnWidths.map(w => `${w}px`).join(' ')
	);

	function handleScroll(e: Event) {
		const target = e.target as HTMLDivElement;
		scrollTop = target.scrollTop;
	}


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
		class={["h-full overflow-auto isolate", compact && "nowheel"]}
		onscroll={handleScroll}
		{@attach (el) => {
			const observer = new ResizeObserver((entries) => {
				containerHeight = entries[0]?.contentRect.height ?? 0;
			});
			observer.observe(el);
			return () => observer.disconnect();
		}}
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
						<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
						<div
							class={["font-medium flex-1 truncate flex items-center gap-1", compact ? "px-2 py-1" : "px-4 py-2", onColumnHeaderClick && "cursor-pointer hover:text-foreground select-none"]}
							onclick={() => onColumnHeaderClick?.(column)}
						>
							{column}
							{#if foreignKeyColumns?.has(column)}
								<KeyRoundIcon class="size-3 shrink-0 text-muted-foreground" />
							{/if}
							{#if sortColumn === column}
								{#if sortDirection === "ASC"}
									<ArrowUpIcon class="size-3 shrink-0 text-primary" />
								{:else if sortDirection === "DESC"}
									<ArrowDownIcon class="size-3 shrink-0 text-primary" />
								{/if}
							{/if}
						</div>
						<!-- Resize handle -->
						<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
						<div
							class="absolute right-0 top-0 bottom-0 w-1 cursor-col-resize hover:bg-primary/50 group-hover:bg-border transition-colors"
							onmousedown={(e) => startResize(i, e)}
							ondblclick={() => { columnWidths[i] = autoSizeColumn(i); }}
							role="separator"
							aria-orientation="vertical"
						></div>
					</div>
				{/each}
			</div>

			<!-- Virtual Body -->
			<div style="height: {totalHeight}px; position: relative;">
				{#each visibleRows as { row, index: rowIndex, offset } (rowIndex)}
					{@const rowHasTextarea = textareaCell?.startsWith(`${rowIndex}:`) ?? false}
					{@const isRowPendingDelete = pendingRowDeletes?.has(rowIndex) ?? false}
					<div
						class={[
							"border-b hover:bg-muted/50 grid",
							compact ? "text-xs" : "text-sm",
							rowIndex % 2 === 0 && "bg-muted/20",
							rowHasTextarea && "overflow-visible z-20",
							isRowPendingDelete && "bg-destructive/10 line-through opacity-60",
						]}
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
						{#each columns as column, colIdx}
							{@const cellKey = `${rowIndex}:${column}`}
							{@const isCellPendingEdit = pendingCellEdits?.has(cellKey) ?? false}
							{@const pendingNewValue = isCellPendingEdit ? pendingCellEdits?.get(cellKey) : undefined}
							{@const fkInfo = foreignKeyColumns?.get(column)}
							{@const cellValue = row[colIdx]}
							{@const showFkIcon = fkInfo && cellValue != null && onForeignKeyClick}
							<!-- svelte-ignore a11y_no_static_element_interactions -->
							<div
								class={[
									"flex group/fk",
									isCellPendingEdit ? "flex-col justify-center" : "items-center",
									textareaCell === cellKey ? "overflow-visible relative" : "overflow-hidden",
									compact ? "px-2 py-0.5" : "px-4 py-0.5",
									isCellPendingEdit && "bg-amber-500/10 border-l-2 border-l-amber-500",
								]}
								oncontextmenu={() => onCellRightClick(cellValue, column, row, rowIndex)}
							>
								{#if isCellPendingEdit}
									<span class="w-full -mx-1 truncate text-xs line-through text-amber-700 dark:text-amber-400 bg-amber-500/10 rounded px-1 leading-tight">{cellValue === null ? 'NULL' : cellText(cellValue)}</span>
									<EditableCell
										value={pendingNewValue}
										{isEditable}
										columnType={columnTypes[column]}
										onSave={(newValue) => onCellSave(rowIndex, column, newValue)}
										onTextareaToggle={(active) => { textareaCell = active ? cellKey : null; }}
										pendingDisplay
										{binaryStrings}
									/>
								{:else}
									<EditableCell
										value={cellValue}
										{isEditable}
										columnType={columnTypes[column]}
										onSave={(newValue) => onCellSave(rowIndex, column, newValue)}
										onTextareaToggle={(active) => { textareaCell = active ? cellKey : null; }}
										{binaryStrings}
									/>
								{/if}
								{#if showFkIcon}
									<button
										class="hidden group-hover/fk:flex items-center justify-center shrink-0 size-5 rounded hover:bg-muted text-muted-foreground hover:text-foreground transition-colors"
										onclick={(e) => { e.stopPropagation(); onForeignKeyClick(fkInfo.ref, fkInfo.table, cellText(cellValue)); }}
										title="{fkInfo.ref.referencedSchema}.{fkInfo.ref.referencedTable}.{fkInfo.ref.referencedColumn}"
									>
										<ArrowUpRightIcon class="size-3" />
									</button>
								{/if}
							</div>
						{/each}
					</div>
				{/each}
			</div>

			<!-- Pending new rows (inline) -->
			{#if pendingRowsContent}
				{@render pendingRowsContent(gridTemplate)}
			{/if}

			<!-- Pending insert rows -->
			{#each pendingInsertRows as insertRow (insertRow.id)}
				<div
					class={["border-b grid bg-green-500/15", compact ? "text-xs" : "text-sm"]}
					style="height: {ROW_HEIGHT}px; grid-template-columns: {gridTemplate};"
				>
					{#if isEditable}
						<div class={["flex items-center justify-center", compact ? "px-1 py-0.5" : "px-2 py-1"]}>
							{#if onRemovePendingInsert}
								<button
									class="size-6 flex items-center justify-center rounded-sm text-muted-foreground hover:text-destructive hover:bg-destructive/10 transition-colors"
									onclick={() => onRemovePendingInsert(insertRow.id)}
									title="Remove pending insert"
								>
									<XIcon class="size-3.5" />
								</button>
							{/if}
						</div>
					{/if}
					{#each columns as column}
						<div class={["flex items-center overflow-hidden", compact ? "px-2 py-1" : "px-4 py-2"]}>
							<span class="truncate text-muted-foreground">{cellText(insertRow.values[column])}</span>
						</div>
					{/each}
				</div>
			{/each}

			<!-- Add Row -->
			{#if onAddRow}
				<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
				<div
					class="flex items-center gap-2 px-2.5 py-2 border-b border-dashed text-muted-foreground hover:text-foreground hover:bg-muted/50 cursor-pointer transition-colors"
					onclick={onAddRow}
				>
					<PlusIcon class="size-3.5" />
					<span class="text-xs">{m.data_viewer_add_row()}</span>
				</div>
			{/if}
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
			{#if isEditable}
				<ContextMenu.Separator />
				<ContextMenu.Item onclick={onSetNull}>
					<CircleOffIcon class="size-4 me-2" />
					{m.query_set_null()}
				</ContextMenu.Item>
				<ContextMenu.Item onclick={onSetDefault}>
					<RotateCcwIcon class="size-4 me-2" />
					{m.query_set_default()}
				</ContextMenu.Item>
			{/if}
		</ContextMenu.Content>
	</ContextMenu.Portal>
</ContextMenu.Root>
