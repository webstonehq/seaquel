<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import * as Select from "$lib/components/ui/select/index.js";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import TrashIcon from "@lucide/svelte/icons/trash";
	import { m } from "$lib/paraglide/messages.js";
	import type { DataFilter, DataFilterOperator, SchemaColumn } from "$lib/types";

	let {
		columns,
		filters,
		filterLogic,
		onFiltersChange,
	}: {
		columns: SchemaColumn[];
		filters: DataFilter[];
		filterLogic: "AND" | "OR";
		onFiltersChange: (filters: DataFilter[], logic: "AND" | "OR") => void;
	} = $props();

	const operators: { value: DataFilterOperator; label: string }[] = [
		{ value: "=", label: "=" },
		{ value: "!=", label: "!=" },
		{ value: ">", label: ">" },
		{ value: "<", label: "<" },
		{ value: ">=", label: ">=" },
		{ value: "<=", label: "<=" },
		{ value: "LIKE", label: "LIKE" },
		{ value: "NOT LIKE", label: "NOT LIKE" },
		{ value: "IN", label: "IN" },
		{ value: "NOT IN", label: "NOT IN" },
		{ value: "IS NULL", label: "IS NULL" },
		{ value: "IS NOT NULL", label: "IS NOT NULL" },
	];

	const nullOperators = new Set<DataFilterOperator>(["IS NULL", "IS NOT NULL"]);

	function addFilter() {
		const newFilter: DataFilter = {
			id: crypto.randomUUID(),
			column: columns[0]?.name ?? "",
			operator: "=",
			value: "",
			enabled: true,
		};
		onFiltersChange([...filters, newFilter], filterLogic);
	}

	function removeFilter(id: string) {
		onFiltersChange(
			filters.filter((f) => f.id !== id),
			filterLogic,
		);
	}

	// Track uncommitted value edits locally; commit on blur/Enter
	let pendingValues = $state<Record<string, string>>({});

	function updateFilter(id: string, updates: Partial<DataFilter>) {
		onFiltersChange(
			filters.map((f) => (f.id === id ? { ...f, ...updates } : f)),
			filterLogic,
		);
	}

	function commitPendingValue(id: string) {
		if (id in pendingValues) {
			const value = pendingValues[id];
			delete pendingValues[id];
			pendingValues = { ...pendingValues };
			updateFilter(id, { value });
		}
	}

	function toggleLogic() {
		onFiltersChange(filters, filterLogic === "AND" ? "OR" : "AND");
	}
</script>

<div class="flex flex-wrap items-center gap-2 px-4 py-2 border-b bg-muted/20">
	{#each filters as filter (filter.id)}
		<div class="flex items-center gap-1 border rounded-md bg-background px-1.5 py-0.5">
			<Checkbox
				checked={filter.enabled}
				onCheckedChange={(v) => updateFilter(filter.id, { enabled: v === true })}
				class="size-3.5"
			/>
			<Select.Root
				type="single"
				value={filter.column}
				onValueChange={(v) => updateFilter(filter.id, { column: v })}
			>
				<Select.Trigger class="h-6 text-xs border-0 bg-transparent px-1 min-w-[80px]">
					{filter.column || "column"}
				</Select.Trigger>
				<Select.Content class="max-h-60">
					{#each columns as col}
						<Select.Item value={col.name}>{col.name}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			<Select.Root
				type="single"
				value={filter.operator}
				onValueChange={(v) => updateFilter(filter.id, { operator: v as DataFilterOperator })}
			>
				<Select.Trigger class="h-6 text-xs border-0 bg-transparent px-1 min-w-[50px]">
					{filter.operator}
				</Select.Trigger>
				<Select.Content>
					{#each operators as op}
						<Select.Item value={op.value}>{op.label}</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
			{#if !nullOperators.has(filter.operator)}
				<Input
					value={pendingValues[filter.id] ?? filter.value}
					placeholder="value"
					class="h-6 text-xs border-0 bg-transparent px-1 w-24"
					oninput={(e) => {
						pendingValues = { ...pendingValues, [filter.id]: (e.target as HTMLInputElement).value };
					}}
					onblur={() => commitPendingValue(filter.id)}
					onkeydown={(e) => { if (e.key === "Enter") commitPendingValue(filter.id); }}
				/>
			{/if}
			<Button
				size="icon"
				variant="ghost"
				class="size-5"
				onclick={() => removeFilter(filter.id)}
			>
				<TrashIcon class="size-3" />
			</Button>
		</div>
		{#if filter !== filters[filters.length - 1]}
			<button
				class="text-xs text-muted-foreground hover:text-foreground font-medium px-1"
				onclick={toggleLogic}
			>
				{filterLogic}
			</button>
		{/if}
	{/each}
	<Button size="sm" variant="ghost" class="h-6 text-xs" onclick={addFilter}>
		<PlusIcon class="size-3 me-1" />
		{m.data_viewer_add_filter()}
	</Button>
</div>
