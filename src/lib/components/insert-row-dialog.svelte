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
				toast.error(`${col.name} is required`);
				return;
			}
		}

		if (Object.keys(insertValues).length === 0) {
			toast.error('Please fill in at least one field');
			return;
		}

		isInserting = true;

		const result = await db.insertRow(sourceTable, insertValues);

		if (result.success) {
			toast.success(result.lastInsertId
				? `Row inserted (ID: ${result.lastInsertId})`
				: 'Row inserted');
			open = false;
			onSuccess();
		} else {
			toast.error(`Failed to insert: ${result.error}`);
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
			<Dialog.Title>Insert Row</Dialog.Title>
			<Dialog.Description>
				Add a new row to {sourceTable.schema}.{sourceTable.name}
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
									(optional)
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
				Cancel
			</Button>
			<Button onclick={handleInsert} disabled={isInserting}>
				{#if isInserting}
					<LoaderIcon class="size-4 mr-2 animate-spin" />
				{/if}
				Insert
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
