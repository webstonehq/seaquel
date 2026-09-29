<script lang="ts">
	import type { ExtensionsDuckdbTab, DuckDBExtension, CommunityExtension } from '$lib/types';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import Loader2Icon from '@lucide/svelte/icons/loader-2';
	import SearchIcon from '@lucide/svelte/icons/search';
	import DownloadIcon from '@lucide/svelte/icons/download';
	import PlayIcon from '@lucide/svelte/icons/play';
	import ArrowUpCircleIcon from '@lucide/svelte/icons/arrow-up-circle';
	import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as Table from '$lib/components/ui/table';
	import { useDatabase } from '$lib/hooks/database.svelte';
	import { isTauri } from '$lib/utils/environment';
	import { m } from '$lib/paraglide/messages.js';

	interface Props {
		tab: ExtensionsDuckdbTab;
	}

	let { tab }: Props = $props();

	const db = useDatabase();

	let search = $state('');
	let view = $state<'core' | 'community'>('core');
	let filter = $state<'all' | 'installed' | 'loaded' | 'not_installed'>('all');

	// Normalized row for unified table rendering
	interface ExtensionRow {
		name: string;
		description: string;
		version: string;
		docs_url: string;
		loaded: boolean;
		installed: boolean;
		canUpdate: boolean;
		source: 'core' | 'community';
	}

	// Lookup of installed extensions by name
	const installedExtensionMap = $derived.by(() => {
		const map = new Map<string, DuckDBExtension>();
		if (tab.extensions) {
			for (const ext of tab.extensions) map.set(ext.extension_name, ext);
		}
		return map;
	});

	// Set of community extension names
	const communityExtensionNames = $derived.by(() => {
		const names = new Set<string>();
		if (tab.communityExtensions) {
			for (const ext of tab.communityExtensions) names.add(ext.name);
		}
		return names;
	});

	// Normalize core extensions into rows
	function coreToRow(ext: DuckDBExtension): ExtensionRow {
		return {
			name: ext.extension_name,
			description: ext.description,
			version: ext.extension_version || '',
			docs_url: `https://duckdb.org/docs/lts/core_extensions/${ext.extension_name}`,
			loaded: ext.loaded,
			installed: ext.installed,
			canUpdate: ext.installed && ext.loaded && ext.install_mode === 'REPOSITORY',
			source: 'core',
		};
	}

	// Normalize community extensions into rows
	function communityToRow(ext: CommunityExtension): ExtensionRow {
		const status = installedExtensionMap.get(ext.name);
		return {
			name: ext.name,
			description: ext.description,
			version: status?.extension_version || '',
			docs_url: ext.github_url,
			loaded: status?.loaded ?? false,
			installed: status?.installed ?? false,
			canUpdate: false,
			source: 'community',
		};
	}

	// Filtered and sorted rows for the active view
	const rows = $derived.by(() => {
		let items: ExtensionRow[];

		if (view === 'core') {
			if (!tab.extensions) return [];
			items = tab.extensions
				.filter((e) => !communityExtensionNames.has(e.extension_name))
				.map(coreToRow);
		} else {
			if (!tab.communityExtensions) return [];
			items = [...tab.communityExtensions].map(communityToRow);
		}

		if (filter === 'installed') items = items.filter((e) => e.installed && !e.loaded);
		else if (filter === 'loaded') items = items.filter((e) => e.loaded);
		else if (filter === 'not_installed') items = items.filter((e) => !e.installed && !e.loaded);

		if (search.trim()) {
			const q = search.trim().toLowerCase();
			items = items.filter((e) => e.name.toLowerCase().includes(q) || e.description.toLowerCase().includes(q));
		}

		items.sort((a, b) => {
			const rankA = a.loaded ? 0 : a.installed ? 1 : 2;
			const rankB = b.loaded ? 0 : b.installed ? 1 : 2;
			if (rankA !== rankB) return rankA - rankB;
			return a.name.localeCompare(b.name);
		});

		return items;
	});

	// Group rows by status
	const groups = $derived.by(() => {
		const loaded = rows.filter((e) => e.loaded);
		const installed = rows.filter((e) => e.installed && !e.loaded);
		const rest = rows.filter((e) => !e.installed && !e.loaded);
		const restLabel = view === 'community' ? m.ext_group_available() : m.ext_filter_not_installed();
		return [
			{ key: 'loaded', label: m.ext_filter_loaded(), rows: loaded },
			{ key: 'installed', label: m.ext_filter_installed(), rows: installed },
			{ key: 'rest', label: restLabel, rows: rest },
		].filter((g) => g.rows.length > 0);
	});

	// Counts for toggle buttons
	const coreAll = $derived(
		tab.extensions ? tab.extensions.filter((e) => !communityExtensionNames.has(e.extension_name)) : []
	);
	const coreLoadedCount = $derived(coreAll.filter((e) => e.loaded).length);
	const coreInstalledCount = $derived(coreAll.filter((e) => e.installed).length);
	const coreTotalCount = $derived(coreAll.length);
	const communityTotalCount = $derived(tab.communityExtensions?.length ?? 0);
	const communityInstalledCount = $derived(
		tab.communityExtensions ? tab.communityExtensions.filter((e) => installedExtensionMap.has(e.name)).length : 0
	);
	const communityLoadedCount = $derived(
		tab.communityExtensions ? tab.communityExtensions.filter((e) => installedExtensionMap.get(e.name)?.loaded).length : 0
	);

	// Collapsible group state
	let collapsedGroups = $state<Record<string, boolean>>({});
	function toggleGroup(key: string) {
		collapsedGroups = { ...collapsedGroups, [key]: !collapsedGroups[key] };
	}

	// Actions
	function handleAction(row: ExtensionRow) {
		if (row.source === 'community' && !row.installed && !row.loaded) {
			db.extensionsDuckdbTabs.installCommunityExtension(tab.id, row.name);
		} else if (!row.installed && !row.loaded) {
			db.extensionsDuckdbTabs.installAndLoadExtension(tab.id, row.name);
		} else if (row.installed && !row.loaded) {
			db.extensionsDuckdbTabs.loadExtension(tab.id, row.name);
		} else if (row.canUpdate) {
			db.extensionsDuckdbTabs.updateExtension(tab.id, row.name);
		}
	}

	function getActionLabel(row: ExtensionRow): { icon: typeof DownloadIcon; label: string } | null {
		if (!row.installed && !row.loaded) return { icon: DownloadIcon, label: m.ext_action_install_and_load() };
		if (row.installed && !row.loaded) return { icon: PlayIcon, label: m.ext_action_load() };
		if (row.canUpdate) return { icon: ArrowUpCircleIcon, label: m.ext_action_update() };
		return null;
	}

	function openExternal(url: string) {
		if (isTauri()) {
			import('$lib/api/tauri').then(({ openPath }) => openPath(url));
		} else {
			window.open(url, '_blank');
		}
	}

	function handleRefresh() {
		db.extensionsDuckdbTabs.refresh(tab.id);
	}

	function formatLastRefreshed(date: Date | undefined): string {
		if (!date) return '';
		return new Intl.DateTimeFormat(undefined, {
			hour: 'numeric',
			minute: '2-digit',
			second: '2-digit'
		}).format(date);
	}

	const isLoading = $derived(view === 'core' ? tab.isLoading && !tab.extensions : !!tab.isCommunityLoading);
	const errorMsg = $derived(view === 'core' ? tab.error : tab.communityError);

	const filterOptions = [
		{ value: 'all' as const, label: m.ext_filter_all() },
		{ value: 'installed' as const, label: m.ext_filter_installed() },
		{ value: 'loaded' as const, label: m.ext_filter_loaded() },
		{ value: 'not_installed' as const, label: m.ext_filter_not_installed() }
	];

	const colCount = $derived(view === 'core' ? 4 : 3);
</script>

<div class="flex h-full flex-col overflow-hidden">
	<div class="flex items-center justify-between border-b pl-6 pr-2 py-2">
		<div class="flex items-center gap-2">
			<h2 class="font-medium">{tab.name}</h2>
			{#if tab.lastRefreshed}
				<span class="text-sm text-muted-foreground">
					{m.ext_updated({ time: formatLastRefreshed(tab.lastRefreshed) })}
				</span>
			{/if}
		</div>
		<div class="flex items-center gap-2">
			<div class="relative">
				<SearchIcon class="absolute left-2 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" />
				<Input
					bind:value={search}
					placeholder={m.ext_search_placeholder()}
					class="h-7 w-48 pl-7 text-xs"
				/>
			</div>
			<Button variant="outline" size="sm" class="h-7" onclick={handleRefresh} disabled={tab.isLoading}>
				{#if tab.isLoading}
					<Loader2Icon class="mr-2 size-4 animate-spin" />
					{m.ext_loading()}
				{:else}
					<RefreshCwIcon class="mr-2 size-4" />
					{m.ext_refresh()}
				{/if}
			</Button>
		</div>
	</div>

	<div class="flex items-center gap-1 border-b px-6 py-1.5">
		<Button variant={view === 'core' ? 'secondary' : 'ghost'} size="sm" class="h-6 text-xs" onclick={() => view = 'core'}>
			{m.ext_core()} ({coreLoadedCount}/{coreInstalledCount}/{coreTotalCount})
		</Button>
		<Button variant={view === 'community' ? 'secondary' : 'ghost'} size="sm" class="h-6 text-xs" onclick={() => view = 'community'}>
			{m.ext_community()} ({communityLoadedCount}/{communityInstalledCount}/{communityTotalCount})
		</Button>
		<span class="mx-1 text-muted-foreground">|</span>
		{#each filterOptions as opt (opt.value)}
			<Button variant={filter === opt.value ? 'secondary' : 'ghost'} size="sm" class="h-6 text-xs" onclick={() => filter = opt.value}>
				{opt.label}
			</Button>
		{/each}
	</div>

	<div class="flex-1 overflow-auto">
		{#if isLoading}
			<div class="flex h-full items-center justify-center">
				<Loader2Icon class="size-8 animate-spin text-muted-foreground" />
			</div>
		{:else if errorMsg}
			<div class="flex h-full flex-col items-center justify-center gap-2 text-destructive">
				<p class="font-medium">{m.ext_failed_to_load()}</p>
				<p class="text-sm">{errorMsg}</p>
				<Button variant="outline" size="sm" onclick={view === 'core' ? handleRefresh : () => db.extensionsDuckdbTabs.refreshCommunityExtensions(tab.id)}>
					<RefreshCwIcon class="mr-2 size-4" />
					{m.ext_retry()}
				</Button>
			</div>
		{:else if rows.length === 0}
			<div class="flex h-full items-center justify-center text-muted-foreground">
				<p class="text-sm">{m.ext_no_match()}</p>
			</div>
		{:else}
			<Table.Root class="table-fixed">
				<Table.Header>
					<Table.Row>
						<Table.Head class="w-48 pl-6">{m.ext_col_name()}</Table.Head>
						<Table.Head>{m.ext_col_description()}</Table.Head>
						{#if view === 'core'}
							<Table.Head class="w-28">{m.ext_col_version()}</Table.Head>
						{/if}
						<Table.Head class="w-36">{m.ext_col_actions()}</Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#each groups as group (group.key)}
						<Table.Row class="cursor-pointer hover:bg-muted/50" onclick={() => toggleGroup(group.key)}>
							<Table.Cell colspan={colCount} class="pl-6 py-1.5">
								<div class="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
									<ChevronRightIcon class="size-3.5 transition-transform {collapsedGroups[group.key] ? '' : 'rotate-90'}" />
									{group.label} ({group.rows.length})
								</div>
							</Table.Cell>
						</Table.Row>
						{#if !collapsedGroups[group.key]}
							{#each group.rows as row (row.name)}
								{@const actionInProgress = tab.actionInProgress?.[row.name]}
								{@const action = getActionLabel(row)}
								<Table.Row>
									<Table.Cell class="pl-6">
										<span class="font-mono text-sm">{row.name}</span>
										{#if row.docs_url}
											<button
												class="ml-1.5 inline-flex cursor-pointer text-muted-foreground hover:text-foreground"
												onclick={(e) => { e.stopPropagation(); openExternal(row.docs_url); }}
											>
												<ExternalLinkIcon class="size-3" />
											</button>
										{/if}
									</Table.Cell>
									<Table.Cell class="truncate text-sm text-muted-foreground">{row.description}</Table.Cell>
									{#if view === 'core'}
										<Table.Cell class="font-mono text-xs text-muted-foreground">
											{row.version || '—'}
										</Table.Cell>
									{/if}
									<Table.Cell>
										{#if actionInProgress}
											<Button variant="outline" size="sm" class="h-7 text-xs" disabled>
												<Loader2Icon class="mr-1.5 size-3 animate-spin" />
												{actionInProgress}...
											</Button>
										{:else if action}
											{@const ActionIcon = action.icon}
											<Button variant="outline" size="sm" class="h-7 text-xs" onclick={() => handleAction(row)}>
												<ActionIcon class="mr-1.5 size-3" />
												{action.label}
											</Button>
										{/if}
									</Table.Cell>
								</Table.Row>
							{/each}
						{/if}
					{/each}
				</Table.Body>
			</Table.Root>
		{/if}
	</div>
</div>
