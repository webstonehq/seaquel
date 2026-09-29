<script lang="ts">
	import { Button, buttonVariants } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import { Badge } from "$lib/components/ui/badge";
	import ShortcutKeys from "$lib/components/shortcut-keys.svelte";
	import ConnectionSelector from "$lib/components/connection-selector.svelte";
	import { findShortcut } from "$lib/shortcuts/index.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import PlayIcon from "@lucide/svelte/icons/play";
	import SaveIcon from "@lucide/svelte/icons/save";
	import LoaderIcon from "@lucide/svelte/icons/loader";
	import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
	import WandSparklesIcon from "@lucide/svelte/icons/wand-sparkles";
	import XCircleIcon from "@lucide/svelte/icons/x-circle";
	import SearchIcon from "@lucide/svelte/icons/search";
	import ActivityIcon from "@lucide/svelte/icons/activity";
	import NetworkIcon from "@lucide/svelte/icons/network";
	import HistoryIcon from "@lucide/svelte/icons/history";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { m } from "$lib/paraglide/messages.js";
	import type { StatementResult, ResolvedQueryVersion } from "$lib/types";

	type Props = {
		isExecuting: boolean;
		hasQuery: boolean;
		activeResult: StatementResult | null | undefined;
		liveStatementCount: number;
		onExecute: () => void;
		onExecuteCurrent: () => void;
		onExplain: (analyze: boolean) => void;
		onVisualize: () => void;
		onFormat: () => void;
		onSave: () => void;
		onSaveAs?: () => void;
		queryId?: string;
		tabId?: string;
		versions?: ResolvedQueryVersion[];
		onDiffVersions?: (selected: ResolvedQueryVersion[]) => void;
	};

	let {
		isExecuting,
		hasQuery,
		activeResult,
		liveStatementCount,
		onExecute,
		onExecuteCurrent,
		onExplain,
		onVisualize,
		onFormat,
		onSave,
		onSaveAs,
		queryId,
		tabId,
		versions = [],
		onDiffVersions
	}: Props = $props();

	let checkedVersionIds = $state<string[]>([]);

	export function clearVersionSelection() {
		checkedVersionIds = [];
	}

	function toggleVersion(version: ResolvedQueryVersion) {
		const idx = checkedVersionIds.indexOf(version.id);
		if (idx >= 0) {
			checkedVersionIds = checkedVersionIds.filter((id) => id !== version.id);
		} else if (checkedVersionIds.length < 2) {
			checkedVersionIds = [...checkedVersionIds, version.id];
		} else {
			checkedVersionIds = [checkedVersionIds[1], version.id];
		}

		// Trigger diff callback (order preserved: first selected = left, second = right)
		const selected = checkedVersionIds
			.map((id) => versions.find((v) => v.id === id))
			.filter((v): v is ResolvedQueryVersion => v != null);
		onDiffVersions?.(selected);
	}

	function formatRelativeTime(date: Date): string {
		const now = Date.now();
		const diff = now - date.getTime();
		const minutes = Math.floor(diff / 60000);
		if (minutes < 1) return "just now";
		if (minutes < 60) return `${minutes}m ago`;
		const hours = Math.floor(minutes / 60);
		if (hours < 24) return `${hours}h ago`;
		const days = Math.floor(hours / 24);
		if (days < 30) return `${days}d ago`;
		return date.toLocaleDateString();
	}

	const db = useDatabase();
</script>

<div class="flex items-center justify-between p-2 shrink-0">
	<div class="flex items-center gap-3 text-xs">
		<ConnectionSelector />

		{#if liveStatementCount > 1}
			<span class="flex items-center gap-1">
				<Badge variant="outline" class="text-xs text-muted-foreground"
					>{m.query_statements_count({ count: liveStatementCount })}</Badge
				>
			</span>
		{/if}
		{#if activeResult}
			{#if activeResult.isError}
				<span class="flex items-center gap-1 text-destructive">
					<XCircleIcon class="size-3" />
					{m.query_error()}
				</span>
			{:else if activeResult.queryType && ['insert', 'update', 'delete'].includes(activeResult.queryType)}
				<span class="flex items-center gap-1">
					<Badge variant="secondary" class="text-xs text-muted-foreground"
						>{activeResult.affectedRows ?? 0}</Badge
					>
					{m.query_rows_affected()}
					{#if activeResult.lastInsertId}
						<Badge variant="outline" class="text-xs ms-1"
							>ID: {activeResult.lastInsertId}</Badge
						>
					{/if}
				</span>
			{/if}
		{/if}
	</div>
	<div class="flex items-center gap-2">
		<div class="flex">
			<Button
				size="sm"
				class="h-7 gap-1 rounded-e-none border-e-0"
				onclick={onExecuteCurrent}
				disabled={isExecuting}
			>
				{#if isExecuting}
					<LoaderIcon class="animate-spin size-3" />
				{:else}
					<PlayIcon class="size-3" />
				{/if}
				{m.query_execute()}
			</Button>
			<DropdownMenu.Root>
				<DropdownMenu.Trigger
					class={buttonVariants({ size: "sm", variant: "default" }) +
						" !h-7 px-1.5 rounded-s-none border-s border-primary-foreground/20"}
					disabled={isExecuting}
				>
					<ChevronDownIcon class="size-3" />
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end">
					<DropdownMenu.Item onclick={onExecuteCurrent}>
						<PlayIcon class="size-4 me-2" />
						{m.query_execute_current()}
						{#if findShortcut('executeQuery')}
							<ShortcutKeys keys={findShortcut('executeQuery')!.keys} class="ms-auto" />
						{/if}
					</DropdownMenu.Item>
					{#if liveStatementCount > 1}
						<DropdownMenu.Item onclick={onExecute}>
							<PlayIcon class="size-4 me-2" />
							{m.query_execute_all()}
						</DropdownMenu.Item>
					{/if}
					<DropdownMenu.Separator />
					<DropdownMenu.Item onclick={() => onExplain(false)}>
						<SearchIcon class="size-4 me-2" />
						{m.query_explain()}
					</DropdownMenu.Item>
					<DropdownMenu.Item onclick={() => onExplain(true)}>
						<ActivityIcon class="size-4 me-2" />
						{m.query_explain_analyze()}
					</DropdownMenu.Item>
					<DropdownMenu.Separator />
					<DropdownMenu.Item onclick={onVisualize}>
						<NetworkIcon class="size-4 me-2" />
						Visualize Query
					</DropdownMenu.Item>
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		</div>
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button
						{...props}
						size="sm"
						variant="outline"
						class="h-7 px-2"
						onclick={onFormat}
						disabled={!hasQuery}
					>
						<WandSparklesIcon class="size-3" />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>
				<span class="flex items-center gap-1.5">
					{m.query_format()}
					{#if findShortcut('formatSql')}
						<ShortcutKeys keys={findShortcut('formatSql')!.keys} />
					{/if}
				</span>
			</Tooltip.Content>
		</Tooltip.Root>
		{#if queryId && onSaveAs}
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props })}
						<div class="flex" {...props}>
							<Button
								size="sm"
								variant="outline"
								class="h-7 px-2 rounded-e-none border-e-0"
								onclick={onSave}
								disabled={!hasQuery || (tabId != null && !db.queryTabs.hasUnsavedChanges(tabId))}
							>
								<SaveIcon class="size-3" />
							</Button>
							<DropdownMenu.Root>
								<DropdownMenu.Trigger
									class={buttonVariants({ size: "sm", variant: "outline" }) +
										" !h-7 px-1.5 rounded-s-none"}
									disabled={!hasQuery}
								>
									<ChevronDownIcon class="size-3" />
								</DropdownMenu.Trigger>
								<DropdownMenu.Content align="end">
									<DropdownMenu.Item onclick={onSaveAs}>
										<SaveIcon class="size-4 me-2" />
										{m.query_save_as()}
									</DropdownMenu.Item>
								</DropdownMenu.Content>
							</DropdownMenu.Root>
						</div>
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content>
					<span class="flex items-center gap-1.5">
						{m.query_save()}
						{#if findShortcut('saveQuery')}
							<ShortcutKeys keys={findShortcut('saveQuery')!.keys} />
						{/if}
					</span>
				</Tooltip.Content>
			</Tooltip.Root>
		{:else}
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props })}
						<Button
							{...props}
							size="sm"
							variant="outline"
							class="h-7 px-2"
							onclick={onSave}
							disabled={!hasQuery}
						>
							<SaveIcon class="size-3" />
						</Button>
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content>
					<span class="flex items-center gap-1.5">
						{m.query_save()}
						{#if findShortcut('saveQuery')}
							<ShortcutKeys keys={findShortcut('saveQuery')!.keys} />
						{/if}
					</span>
				</Tooltip.Content>
			</Tooltip.Root>
		{/if}
		{#if queryId && versions.length > 0}
			<DropdownMenu.Root>
				<DropdownMenu.Trigger
					class={buttonVariants({ size: "sm", variant: "outline" }) + " !h-7 px-2"}
				>
					<HistoryIcon class="size-3" />
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end" class="w-80 max-h-80 overflow-y-auto">
					<DropdownMenu.Label class="text-xs text-muted-foreground">Version History</DropdownMenu.Label>
					<DropdownMenu.Separator />
					{#each [...versions].reverse() as version}
						<button
							class="relative flex w-full cursor-default select-none items-start gap-2 rounded-sm px-2 py-1.5 text-left outline-none hover:bg-accent hover:text-accent-foreground"
							onclick={() => toggleVersion(version)}
						>
							<div class="flex size-4 shrink-0 items-center justify-center rounded-sm border border-input mt-0.5 {checkedVersionIds.includes(version.id) ? 'bg-primary border-primary' : ''}">
								{#if checkedVersionIds.includes(version.id)}
									<svg class="size-3 text-primary-foreground" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12" /></svg>
								{/if}
							</div>
							<div class="flex flex-col gap-0.5 flex-1 min-w-0">
								<div class="flex items-center justify-between">
									<span class="font-medium text-xs">Version {version.version}</span>
									<span class="text-xs text-muted-foreground">{formatRelativeTime(version.createdAt)}</span>
								</div>
								<span class="text-xs text-muted-foreground truncate block">{version.query.slice(0, 80)}</span>
							</div>
						</button>
					{/each}
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		{/if}
	</div>
</div>
