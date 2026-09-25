<script lang="ts">
	import type { BinaryStringEncoding, CellType } from "$lib/utils/cell-type";
	import {
		binaryCellBytes,
		displayCellType,
		formatCellNumber,
		formatBinaryPreview,
		formatDate,
		formatDateTime,
		formatTime,
		formatByteSize,
		truncateText,
	} from "$lib/utils/cell-type";
	import { cellText, jsonReplacer } from "$lib/values";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { Badge } from "$lib/components/ui/badge";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import * as Popover from "$lib/components/ui/popover/index.js";

	interface Props {
		value: unknown;
		columnType: CellType;
		isEditable?: boolean;
		onSave?: (newValue: unknown) => Promise<void>;
		/** How string cells in a binary column hold their bytes (see `binaryCellBytes`). */
		binaryStrings?: BinaryStringEncoding;
	}

	let { value, columnType: declaredType, isEditable = false, onSave, binaryStrings = "text" }: Props = $props();

	// A string in a binary column is text on engines that don't send bytes as strings.
	const columnType = $derived(displayCellType(value, declaredType, binaryStrings));
</script>

{#if value === null || value === undefined}
	<span class="text-muted-foreground italic">NULL</span>
{:else if columnType === 'boolean'}
	<Checkbox
		checked={Boolean(value)}
		disabled={!isEditable}
		onCheckedChange={async (checked) => {
			if (onSave) await onSave(Boolean(checked));
		}}
	/>
{:else if columnType === 'integer' || columnType === 'float'}
	<span class="font-mono tabular-nums">{formatCellNumber(value)}</span>
{:else if columnType === 'date'}
	<Tooltip.Root>
		<Tooltip.Trigger class="cursor-default">{formatDate(String(value))}</Tooltip.Trigger>
		<Tooltip.Portal>
			<Tooltip.Content>
				<span class="font-mono text-xs">{String(value)}</span>
			</Tooltip.Content>
		</Tooltip.Portal>
	</Tooltip.Root>
{:else if columnType === 'datetime'}
	<Tooltip.Root>
		<Tooltip.Trigger class="cursor-default">{formatDateTime(String(value))}</Tooltip.Trigger>
		<Tooltip.Portal>
			<Tooltip.Content>
				<span class="font-mono text-xs">{String(value)}</span>
			</Tooltip.Content>
		</Tooltip.Portal>
	</Tooltip.Root>
{:else if columnType === 'time'}
	<Tooltip.Root>
		<Tooltip.Trigger class="cursor-default">{formatTime(String(value))}</Tooltip.Trigger>
		<Tooltip.Portal>
			<Tooltip.Content>
				<span class="font-mono text-xs">{String(value)}</span>
			</Tooltip.Content>
		</Tooltip.Portal>
	</Tooltip.Root>
{:else if columnType === 'uuid'}
	<span class="font-mono text-xs">{#each String(value).split('-') as part, i (i)}{#if i > 0}<span class="text-muted-foreground">-</span>{/if}{part}{/each}</span>
{:else if columnType === 'json'}
	<Popover.Root>
		<Popover.Trigger class="cursor-pointer hover:underline text-left">
			{truncateText(typeof value === 'object' ? JSON.stringify(value, jsonReplacer) : String(value))}
		</Popover.Trigger>
		<Popover.Portal>
			<Popover.Content class="w-96 max-h-64 overflow-auto p-3">
				<pre class="text-xs font-mono whitespace-pre-wrap break-all">{typeof value === 'object' ? JSON.stringify(value, jsonReplacer, 2) : String(value)}</pre>
			</Popover.Content>
		</Popover.Portal>
	</Popover.Root>
{:else if columnType === 'array'}
	{#if Array.isArray(value)}
		<div class="flex items-center gap-1 overflow-hidden">
			{#each value.slice(0, 3) as item, i (i)}
				<Badge variant="secondary">{cellText(item)}</Badge>
			{/each}
			{#if value.length > 3}
				<Badge variant="outline">+{value.length - 3}</Badge>
			{/if}
		</div>
	{:else}
		{String(value)}
	{/if}
{:else if columnType === 'binary'}
	{@const bytes = binaryCellBytes(value, binaryStrings)}
	{#if bytes}
		<span class="flex items-center gap-1 overflow-hidden">
			<span class="font-mono text-xs truncate">{formatBinaryPreview(bytes)}</span>
			<Badge variant="secondary">{formatByteSize(bytes)}</Badge>
		</span>
	{:else}
		<Badge variant="secondary">{formatByteSize(String(value))}</Badge>
	{/if}
{:else if columnType === 'long_text'}
	<Popover.Root>
		<Popover.Trigger class="cursor-pointer hover:underline text-left truncate">
			{truncateText(String(value), 80)}
		</Popover.Trigger>
		<Popover.Portal>
			<Popover.Content class="w-96 max-h-64 overflow-auto p-3">
				<p class="text-xs whitespace-pre-wrap break-all">{String(value)}</p>
			</Popover.Content>
		</Popover.Portal>
	</Popover.Root>
{:else}
	{@const str = cellText(value)}
	{@const trailingSpaces = str.length - str.trimEnd().length}
	{#if trailingSpaces > 0}
		{str.trimEnd()}{#each { length: trailingSpaces } as _, i (i)}<span title="Trailing space" class="trailing-space">&nbsp;</span>{/each}
	{:else}
		{str === '' ? '\u00A0' : str}
	{/if}
{/if}

<style>
	.trailing-space {
		display: inline-block;
		width: 0.5em;
		margin-left: 1px;
		background-color: color-mix(in srgb, currentColor 10%, transparent);
		border: 1px solid color-mix(in srgb, currentColor 15%, transparent);
		border-radius: 3px;
	}
</style>
