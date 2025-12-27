<script lang="ts">
	import { Input } from "$lib/components/ui/input";
	import { LoaderIcon } from "@lucide/svelte";

	interface Props {
		value: unknown;
		isEditable?: boolean;
		onSave: (newValue: string) => Promise<void>;
	}

	let { value, isEditable = false, onSave }: Props = $props();

	let isEditing = $state(false);
	let editValue = $state('');
	let isSaving = $state(false);
	let inputRef = $state<HTMLInputElement | null>(null);

	function startEditing() {
		if (!isEditable) return;
		editValue = formatValue(value);
		isEditing = true;
		// Focus input after it renders
		setTimeout(() => inputRef?.focus(), 0);
	}

	function formatValue(val: unknown): string {
		if (val === null || val === undefined) return '';
		if (typeof val === 'object') return JSON.stringify(val);
		return String(val);
	}

	async function handleSave() {
		const originalValue = formatValue(value);
		if (editValue === originalValue) {
			isEditing = false;
			return;
		}

		isSaving = true;
		try {
			await onSave(editValue);
			isEditing = false;
		} finally {
			isSaving = false;
		}
	}

	function handleKeydown(e: KeyboardEvent) {
		if (e.key === 'Enter') {
			e.preventDefault();
			handleSave();
		} else if (e.key === 'Escape') {
			editValue = formatValue(value);
			isEditing = false;
		}
	}

	function handleBlur() {
		if (!isSaving) {
			handleSave();
		}
	}
</script>

{#if isEditing && isEditable}
	<div class="flex items-center gap-1">
		<Input
			bind:ref={inputRef}
			bind:value={editValue}
			onblur={handleBlur}
			onkeydown={handleKeydown}
			disabled={isSaving}
			class="h-6 text-xs py-0 px-1"
		/>
		{#if isSaving}
			<LoaderIcon class="size-3 animate-spin shrink-0" />
		{/if}
	</div>
{:else}
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<span
		class={isEditable ? "cursor-pointer hover:bg-muted rounded px-1 -mx-1" : ""}
		ondblclick={startEditing}
		role={isEditable ? "button" : undefined}
		tabindex={isEditable ? 0 : undefined}
		onkeydown={(e) => e.key === 'Enter' && startEditing()}
	>
		{#if value === null}
			<span class="text-muted-foreground italic">NULL</span>
		{:else if value === undefined || value === ''}
			<span class="text-muted-foreground">&nbsp;</span>
		{:else if typeof value === 'object'}
			{JSON.stringify(value)}
		{:else}
			{value}
		{/if}
	</span>
{/if}
