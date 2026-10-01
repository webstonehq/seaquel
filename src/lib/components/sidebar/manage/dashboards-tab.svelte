<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import StarIcon from "@lucide/svelte/icons/star";
	import BookmarkIcon from "@lucide/svelte/icons/bookmark";
	import SearchIcon from "@lucide/svelte/icons/search";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import GitBranchIcon from "@lucide/svelte/icons/git-branch";
	import LayoutDashboardIcon from "@lucide/svelte/icons/layout-dashboard";
	import LinkIcon from "@lucide/svelte/icons/link";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
	import { m } from "$lib/paraglide/messages.js";
	import type { Dashboard } from "$lib/types";
	import ResourceFolder from "./resource-folder.svelte";
	import ResourceItem from "./resource-item.svelte";

	interface Props {
		oncopyShareLink: (resource: { repoId?: string; filePath?: string; name?: string; folder?: string }, resourceType: "dashboard") => Promise<void>;
	}

	let { oncopyShareLink }: Props = $props();

	const db = useDatabase();

	let dashboardStarredExpanded = $state(true);
	let dashboardLocalExpanded = $state(true);
	let dashboardSharedExpanded = $state(true);
	let dashboardSearchQuery = $state("");

	// Delete dashboard confirmation dialog state
	let showDeleteDashboardDialog = $state(false);
	let dashboardToDelete = $state<{ id: string; name: string; type: "local" | "shared" } | null>(null);

	const confirmDeleteDashboard = () => {
		if (!dashboardToDelete) return;
		db.dashboards.deleteDashboard(dashboardToDelete.id);
		const tabsToClose = db.state.dashboardTabs.filter(
			(t) => t.dashboardId === dashboardToDelete!.id
		);
		for (const t of tabsToClose) {
			db.dashboardTabs.remove(t.id);
		}
		showDeleteDashboardDialog = false;
		dashboardToDelete = null;
	};

	// Filtering
	const filteredAllDashboards = $derived(
		db.state.projectDashboards.filter(
			(item) => item.name.toLowerCase().includes(dashboardSearchQuery.toLowerCase()),
		),
	);

	const filteredSharedDashboards = $derived(filteredAllDashboards.filter((d) => d.shared));
	const filteredLocalDashboards = $derived(filteredAllDashboards.filter((d) => !d.shared));

	const starredLocalDashboards = $derived(filteredLocalDashboards.filter((item) => item.starred));
	const starredSharedDashboards = $derived(filteredSharedDashboards.filter((item) => item.starred));
	const totalStarredDashboardCount = $derived(starredLocalDashboards.length + starredSharedDashboards.length);

	const nonStarredLocalDashboards = $derived(filteredLocalDashboards.filter((item) => !item.starred));
	const nonStarredSharedDashboards = $derived(filteredSharedDashboards.filter((item) => !item.starred));

	// Handlers
	const handleShareDashboard = async (dashboard: Dashboard) => {
		try {
			await db.dashboards.shareDashboardById(dashboard.id);
		} catch (error) {
			console.error("Failed to share dashboard:", error);
		}
	};

	const handleUnshareDashboard = async (dashboardId: string) => {
		try {
			await db.dashboards.unshareDashboardById(dashboardId);
		} catch (error) {
			console.error("Failed to unshare dashboard:", error);
		}
	};

	const handleSharedDashboardClick = (dashboard: Dashboard) => {
		db.dashboardTabs.add(dashboard.id, dashboard.name);
	};

	const deleteDashboard = (item: Dashboard, type: "local" | "shared") => {
		dashboardToDelete = { id: item.id, name: item.name, type };
		showDeleteDashboardDialog = true;
	};
</script>

<div class="px-4 py-2 flex">
	<div class="flex items-center gap-1">
		<div class="relative">
			<SearchIcon class="absolute start-2 top-1/2 -translate-y-1/2 size-4 text-muted-foreground" />
			<Input
				bind:value={dashboardSearchQuery}
				placeholder={m.sidebar_search_dashboards()}
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
						onclick={async () => {
							const dashboard = await db.dashboards.createDashboard("New Dashboard", { renameIfTaken: true });
							if (dashboard) {
								db.dashboardTabs.add(dashboard.id, dashboard.name);
							}
						}}
					>
						<PlusIcon class="size-4" />
					</Button>
				{/snippet}
			</Tooltip.Trigger>
			<Tooltip.Content>{m.sidebar_new_dashboard()}</Tooltip.Content>
		</Tooltip.Root>
	</div>
</div>

<Sidebar.Group>
	<Sidebar.GroupContent class="px-2">
		<Sidebar.Menu>
			<!-- Starred folder -->
			<ResourceFolder bind:expanded={dashboardStarredExpanded} icon={StarIcon} label={m.sidebar_starred()} count={totalStarredDashboardCount} emptyMessage={m.sidebar_no_starred_dashboards()}>
				{#each starredSharedDashboards as item (`db-starred-shared:${item.id}`)}
					<ResourceItem
						icon={LayoutDashboardIcon}
						name={item.name}
						starred={true}
						shared={true}
						updatedAt={item.updatedAt}
						onclick={() => handleSharedDashboardClick(item)}
						ondelete={() => deleteDashboard(item, "shared")}
						ontogglestar={() => db.dashboards.toggleDashboardStarred(item.id)}
						onunshare={() => handleUnshareDashboard(item.id)}
					/>
				{/each}
				{#each starredLocalDashboards as item (`db-starred-local:${item.id}`)}
					<ResourceItem
						icon={LayoutDashboardIcon}
						name={item.name}
						starred={true}
						shared={false}
						updatedAt={item.updatedAt}
						onclick={() => db.dashboardTabs.add(item.id, item.name)}
						ondelete={() => deleteDashboard(item, "local")}
						ontogglestar={() => db.dashboards.toggleDashboardStarred(item.id)}
						onshare={() => handleShareDashboard(item)}
					/>
				{/each}
			</ResourceFolder>

			<!-- Local folder -->
			<ResourceFolder bind:expanded={dashboardLocalExpanded} icon={BookmarkIcon} label={m.sidebar_local()} count={nonStarredLocalDashboards.length} emptyMessage={m.sidebar_no_local_dashboards()}>
				{#each nonStarredLocalDashboards as item (`db-local:${item.id}`)}
					<ResourceItem
						icon={LayoutDashboardIcon}
						name={item.name}
						starred={false}
						shared={false}
						updatedAt={item.updatedAt}
						onclick={() => db.dashboardTabs.add(item.id, item.name)}
						ondelete={() => deleteDashboard(item, "local")}
						ontogglestar={() => db.dashboards.toggleDashboardStarred(item.id)}
						onshare={() => handleShareDashboard(item)}
					/>
				{/each}
			</ResourceFolder>

			<!-- Shared folder -->
			{#if db.state.activeProjectHasGit}
				<ResourceFolder bind:expanded={dashboardSharedExpanded} icon={GitBranchIcon} label={m.sidebar_shared()} count={nonStarredSharedDashboards.length}>
					{#each nonStarredSharedDashboards as item (`db-shared:${item.id}`)}
						<Sidebar.MenuItem>
							<ContextMenu.Root>
								<ContextMenu.Trigger>
									<ResourceItem
										icon={LayoutDashboardIcon}
										name={item.name}
										starred={false}
										shared={true}
										updatedAt={item.updatedAt}
										onclick={() => handleSharedDashboardClick(item)}
										ondelete={() => deleteDashboard(item, "shared")}
										ontogglestar={() => db.dashboards.toggleDashboardStarred(item.id)}
										onunshare={() => handleUnshareDashboard(item.id)}
									/>
								</ContextMenu.Trigger>
								<ContextMenu.Content class="w-44">
									<ContextMenu.Item onclick={() => oncopyShareLink(item, "dashboard")}>
										<LinkIcon class="size-4 me-2" />
										{m.share_dashboard()}
									</ContextMenu.Item>
									<ContextMenu.Separator />
									<ContextMenu.Item onclick={() => handleUnshareDashboard(item.id)}>
										<GitBranchIcon class="size-4 me-2" />
										{m.connection_mark_local_only()}
									</ContextMenu.Item>
									<ContextMenu.Item class="text-destructive" onclick={() => deleteDashboard(item, "shared")}>
										<Trash2Icon class="size-4 me-2" />
										{m.history_delete_saved()}
									</ContextMenu.Item>
								</ContextMenu.Content>
							</ContextMenu.Root>
						</Sidebar.MenuItem>
					{/each}
					{#if nonStarredSharedDashboards.length === 0}
						<div class="text-center py-4 text-muted-foreground px-2">
							{#if !db.state.activeProjectHasGit}
								<button
									class="text-xs text-primary hover:underline cursor-pointer"
									onclick={() => { db.settingsTabs.open("project"); }}
								>
									{m.sidebar_share_dashboards()}
								</button>
							{:else}
								<p class="text-xs">{m.sidebar_no_shared_dashboards()}</p>
							{/if}
						</div>
					{/if}
				</ResourceFolder>
			{/if}
		</Sidebar.Menu>
	</Sidebar.GroupContent>
</Sidebar.Group>

<DeleteConfirmDialog
	bind:open={showDeleteDashboardDialog}
	title={m.dashboard_delete_title()}
	description={m.dashboard_delete_description({ name: dashboardToDelete?.name ?? "" })}
	cancelText={m.theme_delete_cancel()}
	confirmText={m.theme_delete_confirm()}
	onconfirm={confirmDeleteDashboard}
/>
