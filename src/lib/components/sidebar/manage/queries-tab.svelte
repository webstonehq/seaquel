<script lang="ts">
	import type { ShareResource } from "./share-link.js";
	import { showErrorUnlessShown } from "$lib/errors";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { formatRelativeTime } from "$lib/utils.js";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import StarIcon from "@lucide/svelte/icons/star";
	import ClockIcon from "@lucide/svelte/icons/clock";
	import BookmarkIcon from "@lucide/svelte/icons/bookmark";
	import SearchIcon from "@lucide/svelte/icons/search";
	import FileTextIcon from "@lucide/svelte/icons/file-text";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import HistoryIcon from "@lucide/svelte/icons/history";
	import GitBranchIcon from "@lucide/svelte/icons/git-branch";
	import LinkIcon from "@lucide/svelte/icons/link";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
	import { m } from "$lib/paraglide/messages.js";
	import { formatWireBindValues } from "$lib/utils/bind-values";
	import ResourceFolder from "./resource-folder.svelte";
	import ResourceItem from "./resource-item.svelte";

	interface Props {
		oncopyShareLink: (resource: ShareResource, resourceType: "query") => Promise<void>;
	}

	let { oncopyShareLink }: Props = $props();

	const db = useDatabase();

	let searchQuery = $state("");
	let starredExpanded = $state(true);
	let localExpanded = $state(true);
	let sharedExpanded = $state(true);
	let historyExpanded = $state(true);

	// Delete query confirmation dialog state
	let showDeleteQueryDialog = $state(false);
	let queryToDelete = $state<{ id: string; name: string; type: "saved" | "shared" } | null>(null);

	const confirmDeleteQuery = () => {
		if (!queryToDelete) return;
		void db.savedQueries.deleteQuery(queryToDelete.id).catch(showErrorUnlessShown);
		showDeleteQueryDialog = false;
		queryToDelete = null;
	};

	// Filtering
	const filteredHistory = $derived(
		db.state.activeConnectionQueryHistory.filter((item) =>
			item.query.toLowerCase().includes(searchQuery.toLowerCase()),
		),
	);

	const filteredQueries = $derived(
		db.state.projectQueries.filter(
			(item) =>
				item.name.toLowerCase().includes(searchQuery.toLowerCase()) ||
				item.query.toLowerCase().includes(searchQuery.toLowerCase()),
		),
	);

	const filteredSharedQueries = $derived(filteredQueries.filter((q) => q.shared));
	const filteredSavedQueries = $derived(filteredQueries.filter((q) => !q.shared));

	const starredSavedQueries = $derived(filteredSavedQueries.filter((item) => item.starred));
	const starredSharedQueries = $derived(filteredSharedQueries.filter((item) => item.starred));
	const totalStarredCount = $derived(starredSavedQueries.length + starredSharedQueries.length);

	const localQueries = $derived(filteredSavedQueries.filter((item) => !item.starred));
	const nonStarredSharedQueries = $derived(filteredSharedQueries.filter((item) => !item.starred));

	// Handlers
	const handleQueryClick = (query: typeof db.state.projectQueries[0]) => {
		db.queryTabs.loadQuery(query.id, () => db.ui.setActiveView("query"));
	};

	const handleShareQuery = async (item: typeof db.state.projectQueries[0]) => {
		try {
			await db.savedQueries.shareQuery(item.id);
		} catch (error) {
			showErrorUnlessShown(error);
		}
	};

	const handleUnshareQuery = async (queryId: string) => {
		try {
			await db.savedQueries.unshareQuery(queryId);
		} catch (error) {
			showErrorUnlessShown(error);
		}
	};

	const deleteQuery = (item: typeof db.state.projectQueries[0], type: "saved" | "shared") => {
		queryToDelete = { id: item.id, name: item.name, type };
		showDeleteQueryDialog = true;
	};
</script>

<div class="px-4 py-2 flex">
	<div class="flex items-center gap-1">
		<div class="relative">
			<SearchIcon class="absolute start-2 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
			<Input
				bind:value={searchQuery}
				placeholder={m.sidebar_search_queries()}
				class="ps-8 h-8 text-sm"
			/>
		</div>
		<Tooltip.Root>
			<Tooltip.Trigger>
				{#snippet child({ props })}
					<Button {...props}
						variant="ghost"
						size="icon"
						class="size-8 shrink-0"
						onclick={() => {
							db.queryTabs.add();
							db.ui.setActiveView("query");
						}}
					>
						<PlusIcon class="size-4" />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>{m.sidebar_new_query()}</Tooltip.Content>
		</Tooltip.Root>
	</div>
</div>

<Sidebar.Group>
	<Sidebar.GroupContent class="px-2">
		<Sidebar.Menu>
			<!-- Starred folder -->
			<ResourceFolder bind:expanded={starredExpanded} icon={StarIcon} label={m.sidebar_starred()} count={totalStarredCount} emptyMessage={m.sidebar_no_starred()}>
				{#each starredSharedQueries as item (`starred-shared:${item.id}`)}
					<ResourceItem
						icon={FileTextIcon}
						name={item.name}
						starred={true}
						shared={true}
						updatedAt={item.updatedAt}
						onclick={() => handleQueryClick(item)}
						ondelete={() => deleteQuery(item, "shared")}
						ontogglestar={() => void db.savedQueries.toggleQueryStarred(item.id).catch(showErrorUnlessShown)}
						onunshare={() => handleUnshareQuery(item.id)}
					/>
				{/each}
				{#each starredSavedQueries as item (`starred-saved:${item.id}`)}
					<ResourceItem
						icon={BookmarkIcon}
						name={item.name}
						starred={true}
						shared={false}
						updatedAt={item.updatedAt}
						onclick={() => db.queryTabs.loadQuery(item.id, () => db.ui.setActiveView("query"))}
						ondelete={() => deleteQuery(item, "saved")}
						ontogglestar={() => void db.savedQueries.toggleQueryStarred(item.id).catch(showErrorUnlessShown)}
						onshare={() => handleShareQuery(item)}
					/>
				{/each}
			</ResourceFolder>

			<!-- Local folder -->
			<ResourceFolder bind:expanded={localExpanded} icon={BookmarkIcon} label={m.sidebar_local()} count={localQueries.length} emptyMessage={m.sidebar_no_local()}>
				{#each localQueries as item (`local:${item.id}`)}
					<ResourceItem
						icon={BookmarkIcon}
						name={item.name}
						starred={false}
						shared={false}
						updatedAt={item.updatedAt}
						onclick={() => db.queryTabs.loadQuery(item.id, () => db.ui.setActiveView("query"))}
						ondelete={() => deleteQuery(item, "saved")}
						ontogglestar={() => void db.savedQueries.toggleQueryStarred(item.id).catch(showErrorUnlessShown)}
						onshare={() => handleShareQuery(item)}
					/>
				{/each}
			</ResourceFolder>

			<!-- Shared folder -->
			{#if db.state.activeProjectHasGit}
				<ResourceFolder bind:expanded={sharedExpanded} icon={GitBranchIcon} label={m.sidebar_shared()} count={nonStarredSharedQueries.length}>
					{#each nonStarredSharedQueries as item (`shared:${item.id}`)}
						<Sidebar.MenuItem>
							<ContextMenu.Root>
								<ContextMenu.Trigger>
									<ResourceItem
										icon={FileTextIcon}
										name={item.name}
										starred={false}
										shared={true}
										updatedAt={item.updatedAt}
										onclick={() => handleQueryClick(item)}
										ondelete={() => deleteQuery(item, "shared")}
										ontogglestar={() => void db.savedQueries.toggleQueryStarred(item.id).catch(showErrorUnlessShown)}
										onunshare={() => handleUnshareQuery(item.id)}
									/>
								</ContextMenu.Trigger>
								<ContextMenu.Content class="w-44">
									<ContextMenu.Item onclick={() => oncopyShareLink(item, "query")}>
										<LinkIcon class="size-4 me-2" />
										{m.share_query()}
									</ContextMenu.Item>
									<ContextMenu.Separator />
									<ContextMenu.Item onclick={() => handleUnshareQuery(item.id)}>
										<GitBranchIcon class="size-4 me-2" />
										{m.connection_mark_local_only()}
									</ContextMenu.Item>
									<ContextMenu.Item class="text-destructive" onclick={() => deleteQuery(item, "shared")}>
										<Trash2Icon class="size-4 me-2" />
										{m.history_delete_saved()}
									</ContextMenu.Item>
								</ContextMenu.Content>
							</ContextMenu.Root>
						</Sidebar.MenuItem>
					{/each}
					{#if nonStarredSharedQueries.length === 0}
						<div class="text-center py-4 text-muted-foreground px-2">
							{#if !db.state.activeProjectHasGit}
								<button
									class="text-xs text-primary hover:underline cursor-pointer"
									onclick={() => { db.settingsTabs.open("project"); }}
								>
									{m.sidebar_share_queries()}
								</button>
							{:else}
								<p class="text-xs">{m.sidebar_no_shared()}</p>
							{/if}
						</div>
					{/if}
				</ResourceFolder>
			{/if}

			<!-- History folder -->
			{#if db.state.activeConnectionId}
				<ResourceFolder bind:expanded={historyExpanded} icon={HistoryIcon} label={m.sidebar_history()} count={filteredHistory.length} emptyMessage={m.sidebar_no_history()}>
					{#each filteredHistory as item (item.id)}
						<Sidebar.MenuItem>
							<Sidebar.MenuButton
								class="h-auto py-2 flex-col items-start gap-1 group pr-1"
								onclick={() => db.queryTabs.loadFromHistory(item.id, () => db.ui.setActiveView("query"))}
							>
								<div class="flex items-center justify-between w-full gap-2">
									<div class="flex items-center gap-2 flex-1 min-w-0">
										<ClockIcon class="size-3 text-muted-foreground shrink-0" />
										<span class="text-xs text-muted-foreground">{formatRelativeTime(item.timestamp)}</span>
									</div>
									<Badge variant="secondary" class="text-xs shrink-0 tabular-nums">{item.executionTime}ms</Badge>
								</div>
								<p class="text-xs font-mono line-clamp-2 text-muted-foreground w-full text-left">
									{item.query}
								</p>
								{#if item.params?.length}
									<p class="text-[11px] font-mono line-clamp-1 break-all text-muted-foreground/80 w-full text-left">
										{m.pending_changes_values({ values: formatWireBindValues(item.params, 200) })}
									</p>
								{/if}
							</Sidebar.MenuButton>
						</Sidebar.MenuItem>
					{/each}
				</ResourceFolder>
			{/if}
		</Sidebar.Menu>
	</Sidebar.GroupContent>
</Sidebar.Group>

<DeleteConfirmDialog
	bind:open={showDeleteQueryDialog}
	title={m.query_delete_title()}
	description={m.query_delete_description({ name: queryToDelete?.name ?? "" })}
	cancelText={m.theme_delete_cancel()}
	confirmText={m.theme_delete_confirm()}
	onconfirm={confirmDeleteQuery}
/>
