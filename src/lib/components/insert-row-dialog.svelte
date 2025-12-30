<script lang="ts">
	import { untrack } from "svelte";
	import { Button } from "$lib/components/ui/button";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
	import { LoaderIcon } from "@lucide/svelte";
	import type { SchemaColumn } from "$lib/types";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		open: boolean;
		sourceTable: { schema: string; name: string; primaryKeys: string[] };
		columns: SchemaColumn[];
		onClose: () => void;
		onSuccess: () => void;
	}

	let { open = $bindable(), sourceTable, columns, onClose, onSuccess }: Props = $props();

	const db = useDatabase();
	let isInserting = $state(false);
	let values = $state<Record<string, string>>({});
	let previousOpen = false;

	// Reset form when dialog opens (only on transition from closed to open)
	$effect(() => {
		const isOpening = open && !previousOpen;
		untrack(() => {
			previousOpen = open;
		});
		if (isOpening) {
			const newValues: Record<string, string> = {};
			columns.forEach(col => {
				newValues[col.name] = '';
			});
			values = newValues;
		}
	});

	async function handleInsert() {
		// Filter out empty values for nullable columns
		const insertValues: Record<string, unknown> = {};
		for (const col of columns) {
			const value = values[col.name];
			if (value !== '' && value !== undefined) {
				insertValues[col.name] = value;
			} else if (!col.nullable && !col.defaultValue) {
				toast.error(m.insert_row_error_required({ field: col.name }));
				return;
			}
		}

		if (Object.keys(insertValues).length === 0) {
			toast.error(m.insert_row_error_empty());
			return;
		}

		isInserting = true;

		const result = await db.queries.insertRow(sourceTable, insertValues);

		if (result.success) {
			toast.success(result.lastInsertId
				? m.insert_row_success_with_id({ id: String(result.lastInsertId) })
				: m.insert_row_success());
			open = false;
			onSuccess();
		} else {
			toast.error(m.insert_row_error_failed({ error: result.error || '' }));
		}

		isInserting = false;
	}

	function handleCancel() {
		open = false;
		onClose();
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content class="sm:max-w-lg max-h-[80vh] overflow-hidden flex flex-col">
		<Dialog.Header>
			<Dialog.Title>{m.insert_row_title()}</Dialog.Title>
			<Dialog.Description>
				{m.insert_row_description({ schema: sourceTable.schema, table: sourceTable.name })}
			</Dialog.Description>
		</Dialog.Header>
		<div class="overflow-y-auto flex-1 py-4">
			<div class="grid gap-4">
				{#each columns as col}
					<div class="grid gap-2">
						<Label for={col.name} class="flex items-center gap-2">
							{col.name}
							<span class="text-xs text-muted-foreground font-normal">
								{col.type}
								{#if col.nullable}
									{m.insert_row_optional()}
								{/if}
							</span>
						</Label>
						<Input
							id={col.name}
							bind:value={values[col.name]}
							placeholder={col.defaultValue ? `Default: ${col.defaultValue}` : undefined}
							disabled={isInserting}
						/>
					</div>
				{/each}
			</div>
		</div>
		<Dialog.Footer>
			<Button variant="outline" onclick={handleCancel} disabled={isInserting}>
				{m.insert_row_cancel()}
			</Button>
			<Button onclick={handleInsert} disabled={isInserting}>
				{#if isInserting}
					<LoaderIcon class="size-4 me-2 animate-spin" />
				{/if}
				{m.insert_row_insert()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
