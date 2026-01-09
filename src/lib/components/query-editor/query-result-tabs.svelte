<script lang="ts">
	import { XCircleIcon, TableIcon, ZapIcon } from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { cn } from "$lib/utils.js";
	import type { StatementResult } from "$lib/types";

	type Props = {
		results: StatementResult[];
		activeIndex: number;
		onSelectResult: (index: number) => void;
	};

	let { results, activeIndex, onSelectResult }: Props = $props();
</script>

<div class="flex items-center gap-1 p-2 border-b bg-muted/20 overflow-x-auto shrink-0">
	{#each results as result, i}
		<button
			class={cn(
				"px-3 py-1.5 text-xs rounded-md shrink-0 flex items-center gap-1.5 transition-colors",
				activeIndex === i
					? "bg-primary text-primary-foreground"
					: "bg-muted hover:bg-muted/80",
				result.isError && activeIndex !== i && "text-destructive"
			)}
			onclick={() => onSelectResult(i)}
		>
			{#if result.isError}
				<XCircleIcon class="size-3" />
			{:else if result.queryType === 'select'}
				<TableIcon class="size-3" />
			{:else}
				<ZapIcon class="size-3" />
			{/if}
			{m.query_statement_n({ n: i + 1 })}
			{#if result.isError}
				<span class="opacity-70">{m.query_result_error()}</span>
			{:else if result.queryType === 'select'}
				<span class="opacity-70"
					>{m.query_result_rows_time({ rows: result.totalRows, time: result.executionTime })}</span
				>
			{:else if result.affectedRows !== undefined}
				<span class="opacity-70"
					>{m.query_result_affected_time({
						affected: result.affectedRows,
						time: result.executionTime
					})}</span
				>
			{:else}
				<span class="opacity-70">{m.query_result_time({ time: result.executionTime })}</span>
			{/if}
		</button>
	{/each}
</div>
