<script lang="ts">
	import type { Dashboard, DashboardVersion } from '$lib/types';
	import { Button } from '$lib/components/ui/button';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu/index.js';
	import ConnectionSelector from '$lib/components/connection-selector.svelte';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import Loader2Icon from '@lucide/svelte/icons/loader-2';
	import MaximizeIcon from '@lucide/svelte/icons/maximize';
	import MinimizeIcon from '@lucide/svelte/icons/minimize';
	import HistoryIcon from '@lucide/svelte/icons/history';

	interface Props {
		dashboard: Dashboard;
		isFullscreen?: boolean;
		/** The history, without snapshots: the view fetches the ones picked. */
		versions?: DashboardVersion[];
		onAddWidget: () => void;
		onRefreshAll: () => void;
		onToggleFullscreen?: () => void;
		onDiffVersions?: (selected: DashboardVersion[]) => void;
	}

	let {
		dashboard,
		isFullscreen = false,
		versions = [],
		onAddWidget,
		onRefreshAll,
		onToggleFullscreen,
		onDiffVersions,
	}: Props = $props();

	const isAnyWidgetLoading = $derived(
		dashboard.widgets.some((w) => w.isLoading)
	);

	let checkedVersionIds = $state<string[]>([]);

	export function clearVersionSelection() {
		checkedVersionIds = [];
	}

	function toggleVersion(version: DashboardVersion) {
		const idx = checkedVersionIds.indexOf(version.id);
		if (idx >= 0) {
			checkedVersionIds = checkedVersionIds.filter((id) => id !== version.id);
		} else if (checkedVersionIds.length < 2) {
			checkedVersionIds = [...checkedVersionIds, version.id];
		} else {
			checkedVersionIds = [checkedVersionIds[1], version.id];
		}

		const selected = checkedVersionIds
			.map((id) => versions.find((v) => v.id === id))
			.filter((v): v is DashboardVersion => v != null);
		onDiffVersions?.(selected);
	}

	function formatRelativeTime(date: Date): string {
		const now = Date.now();
		const diff = now - date.getTime();
		const seconds = Math.floor(diff / 1000);
		if (seconds < 60) return `${seconds}s ago`;
		const minutes = Math.floor(seconds / 60);
		if (minutes < 60) return `${minutes}m ago`;
		const hours = Math.floor(minutes / 60);
		if (hours < 24) return `${hours}h ago`;
		const days = Math.floor(hours / 24);
		return `${days}d ago`;
	}
</script>

<div class="flex items-center justify-between border-b bg-muted/30 px-2 py-2">
	<div class="flex items-center gap-2">
	    <ConnectionSelector />
		<span class="text-xs text-muted-foreground">
			{dashboard.widgets.length} widget{dashboard.widgets.length !== 1 ? 's' : ''}
		</span>
	</div>

	<div class="flex items-center gap-1">
		{#if !isFullscreen}
			<Button variant="outline" size="sm" class="h-7" onclick={onAddWidget}>
				<PlusIcon class="mr-1 size-3.5" />
				Add Widget
			</Button>
		{/if}

		<Button
			variant="outline"
			size="sm"
			class="h-7"
			onclick={onRefreshAll}
			disabled={isAnyWidgetLoading}
		>
			{#if isAnyWidgetLoading}
				<Loader2Icon class="mr-1 size-3.5 animate-spin" />
			{:else}
				<RefreshCwIcon class="mr-1 size-3.5" />
			{/if}
			Refresh
		</Button>

		{#if versions.length > 0}
			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					<Button variant="outline" size="sm" class="h-7 px-2">
						<HistoryIcon class="size-3" />
					</Button>
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="end" class="w-80 max-h-80 overflow-y-auto">
					<DropdownMenu.Label class="text-xs text-muted-foreground">Version History</DropdownMenu.Label>
					<DropdownMenu.Separator />
					{#each [...versions].reverse() as version (version.id)}
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
								{#if version.widgetCount !== null}
									<span class="text-xs text-muted-foreground truncate block">
										{version.widgetCount} widget{version.widgetCount !== 1 ? 's' : ''}
									</span>
								{/if}
							</div>
						</button>
					{/each}
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		{/if}

		{#if onToggleFullscreen}
			<Button variant="ghost" size="icon" class="size-7" onclick={onToggleFullscreen}>
				{#if isFullscreen}
					<MinimizeIcon class="size-3.5" />
				{:else}
					<MaximizeIcon class="size-3.5" />
				{/if}
			</Button>
		{/if}
	</div>
</div>
