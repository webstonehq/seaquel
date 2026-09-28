<script lang="ts">
	import { Button, buttonVariants } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import {
		ChevronDownIcon,
		ChevronLeftIcon,
		ChevronRightIcon,
		ChevronsLeftIcon,
		ChevronsRightIcon,
		LoaderIcon,
		XIcon
	} from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";

	type Props = {
		page: number;
		pageSize: number;
		totalPages: number;
		totalRows: number;
		isExecuting: boolean;
		isStreaming?: boolean;
		allowStreamAll?: boolean;
		onGoToPage: (page: number) => void;
		onSetPageSize: (size: number) => void;
		onCancelStream?: () => void;
	};

	let {
		page,
		pageSize,
		totalPages,
		totalRows,
		isExecuting,
		isStreaming = false,
		allowStreamAll = true,
		onGoToPage,
		onSetPageSize,
		onCancelStream
	}: Props = $props();

	const isAllRows = $derived(pageSize === 0);
	// While streaming, show the full accumulated range instead of the page slice.
	// Streaming produces a single flat result, so page/pageSize math doesn't apply.
	const start = $derived(isStreaming || isAllRows ? (totalRows > 0 ? 1 : 0) : (page - 1) * pageSize + 1);
	const end = $derived(
		isStreaming || isAllRows ? totalRows : Math.min(page * pageSize, totalRows)
	);
</script>

<div class="flex items-center justify-between p-2 border-t bg-muted/30 shrink-0 text-xs">
	<div class="flex items-center gap-2 text-muted-foreground">
		<span class="tabular-nums">{m.query_showing_rows({ start, end, total: totalRows.toLocaleString() })}</span>
		{#if isStreaming}
			<span class="flex items-center gap-1 text-primary">
				<LoaderIcon class="size-3 animate-spin" />
				streaming…
			</span>
			{#if onCancelStream}
				<Button
					size="sm"
					variant="ghost"
					class="h-6 px-2 text-xs gap-1"
					onclick={onCancelStream}
					aria-label="Cancel stream"
				>
					<XIcon class="size-3" />
					Cancel
				</Button>
			{/if}
		{/if}
	</div>
	<div class="flex items-center gap-2">
		<DropdownMenu.Root>
			<DropdownMenu.Trigger
				disabled={isExecuting}
				class={buttonVariants({
					variant: "outline",
					size: "sm"
				}) + " h-7 gap-1 text-xs"}
			>
				{isAllRows ? "Stream all" : `${pageSize} rows`}
				<ChevronDownIcon class="size-3" />
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="end">
				{#each [25, 50, 100, 250, 500, 1000] as size (size)}
					<DropdownMenu.Item
						onclick={() => onSetPageSize(size)}
						class={pageSize === size ? "bg-accent" : ""}
					>
						{m.query_rows_count({ count: size })}
					</DropdownMenu.Item>
				{/each}
				{#if allowStreamAll}
					<DropdownMenu.Separator />
					<DropdownMenu.Item
						onclick={() => onSetPageSize(0)}
						class={isAllRows ? "bg-accent" : ""}
					>
						Stream all
					</DropdownMenu.Item>
				{/if}
			</DropdownMenu.Content>
		</DropdownMenu.Root>

		{#if !isStreaming && !isAllRows}
			<div class="flex items-center gap-1 tabular-nums">
				<Button
					size="icon"
					variant="outline"
					class="size-7"
					aria-label={m.query_first_page()}
					onclick={() => onGoToPage(1)}
					disabled={page === 1 || isExecuting}
				>
					<ChevronsLeftIcon class="size-3" />
				</Button>
				<Button
					size="icon"
					variant="outline"
					class="size-7"
					aria-label={m.query_previous_page()}
					onclick={() => onGoToPage(page - 1)}
					disabled={page === 1 || isExecuting}
				>
					<ChevronLeftIcon class="size-3" />
				</Button>
				<span class="px-2 text-muted-foreground">
					{m.query_page_of({ page, total: totalPages })}
				</span>
				<Button
					size="icon"
					variant="outline"
					class="size-7"
					aria-label={m.query_next_page()}
					onclick={() => onGoToPage(page + 1)}
					disabled={page === totalPages || isExecuting}
				>
					<ChevronRightIcon class="size-3" />
				</Button>
				<Button
					size="icon"
					variant="outline"
					class="size-7"
					aria-label={m.query_last_page()}
					onclick={() => onGoToPage(totalPages)}
					disabled={page === totalPages || isExecuting}
				>
					<ChevronsRightIcon class="size-3" />
				</Button>
			</div>
		{/if}
	</div>
</div>
