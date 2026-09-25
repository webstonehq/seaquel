<script lang="ts">
	import { Input } from "$lib/components/ui/input";
	import { LoaderIcon } from "@lucide/svelte";
	import FormattedCell from "$lib/components/formatted-cell.svelte";
	import type { BinaryStringEncoding, CellType } from "$lib/utils/cell-type";
	import { binaryCellBytes, editedCellValue, inputTypeForCellType } from "$lib/utils/cell-type";
	import { SqlDecimal, cellText, jsonReplacer, toHex } from "$lib/values";

	interface Props {
		value: unknown;
		isEditable?: boolean;
		columnType?: CellType;
		onSave: (newValue: unknown) => Promise<void>;
		onTextareaToggle?: (active: boolean) => void;
		/** When true, render the idle state with green pending-change styling */
		pendingDisplay?: boolean;
		/** How string cells in a binary column hold their bytes (see `binaryCellBytes`). */
		binaryStrings?: BinaryStringEncoding;
	}

	let { value, isEditable = false, columnType = 'text', onSave, onTextareaToggle = () => {}, pendingDisplay = false, binaryStrings = 'text' }: Props = $props();

	const monoTypes = new Set(['integer', 'float', 'date', 'datetime', 'time', 'uuid', 'json']);
	const useMono = $derived(monoTypes.has(columnType));
	const htmlInputType = $derived(inputTypeForCellType(columnType));

	let isEditing = $state(false);
	let editValue = $state('');
	let isSaving = $state(false);
	let inputRef = $state<HTMLInputElement | null>(null);
	let textareaRef = $state<HTMLTextAreaElement | null>(null);
	let useTextarea = $state(false);

	function startEditing() {
		if (!isEditable) return;
		editValue = formatValue(value);
		useTextarea = false;
		isEditing = true;
		// Focus input after it renders, then check if it overflows
		setTimeout(() => {
			if (inputRef) {
				inputRef.focus();
				if (inputRef.scrollWidth > inputRef.clientWidth) {
					useTextarea = true;
					onTextareaToggle(true);
					setTimeout(() => textareaRef?.focus(), 0);
				}
			}
		}, 0);
	}

	function formatValue(val: unknown): string {
		if (val === null || val === undefined) return '';
		// Bytes are edited as \x hex, as the cell shows them; that includes a
		// MySQL VARBINARY string in a binary column.
		const bytes = columnType === 'binary' ? binaryCellBytes(val, binaryStrings) : null;
		if (bytes) return toHex(bytes);
		if (val instanceof Uint8Array) return toHex(val);
		if (val instanceof SqlDecimal) return val.value;
		// Compact on purpose: a single-line <input> would drop the newlines of
		// pretty-printed JSON and turn every edit into a change.
		if (typeof val === 'object') return JSON.stringify(val, jsonReplacer);
		return String(val);
	}

	function stopEditing() {
		isEditing = false;
		if (useTextarea) onTextareaToggle(false);
		useTextarea = false;
	}

	async function handleSave() {
		const originalValue = formatValue(value);
		if (editValue === originalValue) {
			stopEditing();
			return;
		}

		isSaving = true;
		try {
			await onSave(editedCellValue(value, columnType, editValue, binaryStrings));
			stopEditing();
		} finally {
			isSaving = false;
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			if (useTextarea && !e.metaKey && !e.ctrlKey) return; // allow newlines in textarea
			e.preventDefault();
			handleSave();
		} else if (e.key === 'Escape') {
			editValue = formatValue(value);
			stopEditing();
		}
	}

	function handleBlur() {
		if (!isSaving) {
			handleSave();
		}
	}
</script>

{#if isEditing && isEditable}
	<div class="flex items-center gap-1 w-full relative">
		{#if isSaving}
			<LoaderIcon class="size-3 animate-spin shrink-0" />
		{/if}
		{#if useTextarea}
			<textarea
				bind:this={textareaRef}
				bind:value={editValue}
				onblur={handleBlur}
				onkeydown={handleKeydown}
				disabled={isSaving}
				rows={4}
				class={["absolute -top-4 -left-4 min-w-[calc(100%+2rem)] w-max max-w-[80vw] z-20 text-sm px-4 py-1 border border-secondary-foreground bg-background shadow-lg focus-visible:ring-0 focus-visible:outline-none resize", useMono && "font-mono tabular-nums"]}
			></textarea>
		{:else}
			<Input
				bind:ref={inputRef}
				bind:value={editValue}
				type={htmlInputType}
				step={htmlInputType === "number" ? (columnType === "integer" ? "1" : "any") : undefined}
				onblur={handleBlur}
				onkeydown={handleKeydown}
				disabled={isSaving}
				class={["h-6 text-sm py-0 px-0 border-0 shadow-none focus-visible:ring-0 focus-visible:border-transparent", useMono && "font-mono tabular-nums"]}
			/>
		{/if}
	</div>
{:else}
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<span
		class={[
			isEditable ? "cursor-pointer hover:bg-muted rounded px-1 -mx-1 w-full min-w-0 min-h-5 block" : "block min-w-0",
			"truncate",
			(columnType === 'integer' || columnType === 'float') && "w-full",
			pendingDisplay && "w-full text-xs text-green-700 dark:text-green-400 bg-green-500/10 rounded px-1 leading-tight",
		]}
		ondblclick={startEditing}
		role={isEditable ? "button" : undefined}
		tabindex={isEditable ? 0 : undefined}
		onkeydown={(e) => e.key === 'Enter' && startEditing()}
	>
		{#if pendingDisplay}
			{value === null ? 'NULL' : value === undefined ? 'DEFAULT' : cellText(value)}
		{:else}
			<FormattedCell {value} {columnType} {isEditable} {onSave} {binaryStrings} />
		{/if}
	</span>
{/if}
