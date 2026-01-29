<script lang="ts">
	import { Button, buttonVariants } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import {
		ChevronDownIcon,
		ChevronLeftIcon,
		ChevronRightIcon,
		ChevronsLeftIcon,
		ChevronsRightIcon
	} from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";

	type Props = {
		page: number;
		pageSize: number;
		totalPages: number;
		totalRows: number;
		isExecuting: boolean;
		onGoToPage: (page: number) => void;
		onSetPageSize: (size: number) => void;
	};

	let { page, pageSize, totalPages, totalRows, isExecuting, onGoToPage, onSetPageSize }: Props =
		$props();

	const isAllRows = $derived(pageSize === 0);
	const start = $derived(isAllRows ? 1 : (page - 1) * pageSize + 1);
	const end = $derived(isAllRows ? totalRows : Math.min(page * pageSize, totalRows));
</script>

<div class="flex items-center justify-between p-2 border-t bg-muted/30 shrink-0 text-xs">
	<div class="text-muted-foreground">
		{m.query_showing_rows({ start, end, total: totalRows.toLocaleString() })}
	</div>
	<div class="flex items-center gap-2">
		<DropdownMenu.Root>
			<DropdownMenu.Trigger
				class={buttonVariants({
					variant: "outline",
					size: "sm"
				}) + " h-7 gap-1 text-xs"}
			>
				{isAllRows ? "All (YOLO)" : `${pageSize} rows`}
				<ChevronDownIcon class="size-3" />
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="end">
				{#each [25, 50, 100, 250, 500, 1000] as size}
					<DropdownMenu.Item
						onclick={() => onSetPageSize(size)}
						class={pageSize === size ? "bg-accent" : ""}
					>
						{m.query_rows_count({ count: size })}
					</DropdownMenu.Item>
				{/each}
				<DropdownMenu.Separator />
				<DropdownMenu.Item
					onclick={() => onSetPageSize(0)}
					class={isAllRows ? "bg-accent" : ""}
				>
					All (YOLO)
				</DropdownMenu.Item>
			</DropdownMenu.Content>
		</DropdownMenu.Root>

		<div class="flex items-center gap-1">
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
	</div>
</div>
