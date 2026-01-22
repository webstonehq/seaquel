<script lang="ts">
	import { SvelteSet } from "svelte/reactivity";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import {
		SidebarMenu,
		SidebarMenuItem,
		SidebarMenuButton
	} from "$lib/components/ui/sidebar";
	import { Badge } from "$lib/components/ui/badge";
	import { Button } from "$lib/components/ui/button";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import * as ContextMenu from "$lib/components/ui/context-menu/index.js";
	import * as Select from "$lib/components/ui/select/index.js";
	import SyncStatusBadge from "./sync-status-badge.svelte";
	import SyncButton from "./sync-button.svelte";
	import AddRepoDialog from "./add-repo-dialog.svelte";
	import ManageReposDialog from "./manage-repos-dialog.svelte";
	import SyncConflictDialog from "./sync-conflict-dialog.svelte";
	import type { SharedQuery, SharedQueryFolder } from "$lib/types";
	import {
		ChevronRightIcon,
		FolderIcon,
		FileTextIcon,
		PlusIcon,
		Trash2Icon,
		CopyIcon,
		PlayIcon,
		AlertTriangleIcon,
		SettingsIcon
	} from "@lucide/svelte";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";

	const db = useDatabase();

	let expandedFolders = new SvelteSet<string>();
	let showAddRepoDialog = $state(false);
	let showManageReposDialog = $state(false);
	let showConflictDialog = $state(false);

	// Conflict detection
	const conflictFiles = $derived(db.state.activeRepoSyncState?.conflictFiles ?? []);
	const hasConflicts = $derived(conflictFiles.length > 0);

	// Build folder tree from queries
	const folderTree = $derived.by(() => {
		if (!db.state.activeRepoId) return null;

		const queries = db.state.activeRepoQueries;
		const root: SharedQueryFolder = {
			name: "",
			path: "",
			children: [],
			queries: []
		};

		// First, collect all folder paths
		const folderPaths = new Set<string>();
		queries.forEach((q) => {
			if (q.folder) {
				// Add folder and all parent folders
				const parts = q.folder.split("/");
				for (let i = 1; i <= parts.length; i++) {
					folderPaths.add(parts.slice(0, i).join("/"));
				}
			}
		});

		// Create folder nodes
		const folderMap = new Map<string, SharedQueryFolder>();
		folderMap.set("", root);

		Array.from(folderPaths)
			.sort()
			.forEach((path) => {
				const parts = path.split("/");
				const name = parts[parts.length - 1];
				const parentPath = parts.slice(0, -1).join("/");

				const folder: SharedQueryFolder = {
					name,
					path,
					children: [],
					queries: []
				};

				folderMap.set(path, folder);
				const parent = folderMap.get(parentPath);
				if (parent) {
					parent.children.push(folder);
				}
			});

		// Add queries to their folders
		queries.forEach((query) => {
			const folder = folderMap.get(query.folder);
			if (folder) {
				folder.queries.push(query);
			}
		});

		return root;
	});

	const toggleFolder = (path: string) => {
		if (expandedFolders.has(path)) {
			expandedFolders.delete(path);
		} else {
			expandedFolders.add(path);
		}
	};

	const handleQueryClick = (query: SharedQuery) => {
		// Open query in existing tab or create new one
		db.queryTabs.loadSharedQuery(query.id, query.name, query.query, () => db.ui.setActiveView("query"));
	};

	const handleCopyToLocal = async (query: SharedQuery) => {
		if (!db.state.activeConnectionId) {
			errorToast("Please connect to a database first");
			return;
		}

		const id = db.savedQueries.saveQuery(query.name, query.query, undefined, query.parameters);
		if (id) {
			toast.success(`Query "${query.name}" saved to local queries`);
		} else {
			errorToast("Failed to save query");
		}
	};

	const handleDeleteQuery = async (query: SharedQuery) => {
		try {
			await db.sharedQueries.deleteQuery(query.id);
			toast.success(`Query "${query.name}" deleted`);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Failed to delete query: ${message}`);
		}
	};

</script>

<div class="py-1">
	{#if db.state.sharedRepos.length === 0}
		<div class="py-2 text-center text-muted-foreground">
			<p class="text-xs mb-2">No shared repositories</p>
			<Button
				variant="outline"
				size="sm"
				class="text-xs"
				onclick={() => (showAddRepoDialog = true)}
			>
				<PlusIcon class="size-3 me-1" />
				Add Repository
			</Button>
		</div>
	{:else}
		<!-- Repository selector -->
		<div class="py-1 pr-2">
			<Select.Root
				type="single"
				value={db.state.activeRepoId ?? undefined}
				onValueChange={(v) => db.sharedRepos.setActiveRepo(v ?? null)}
			>
				<Select.Trigger class="h-7 text-xs w-full">
					{db.state.activeRepo?.name ?? "Select repository"}
				</Select.Trigger>
				<Select.Content>
					{#each db.state.sharedRepos as repo (repo.id)}
						<Select.Item value={repo.id} class="text-xs">
							<div class="flex items-center gap-2">
								<span class="flex-1">{repo.name}</span>
								<SyncStatusBadge
									status={repo.syncStatus}
									syncState={db.state.syncStateByRepo[repo.id]}
									showCounts={false}
								/>
							</div>
						</Select.Item>
					{/each}
				</Select.Content>
			</Select.Root>
		</div>

		{#if db.state.activeRepoId && db.state.activeRepo}
			<!-- Sync controls -->
			<div class="py-1 flex items-center gap-2 pr-2">
				<SyncButton repoId={db.state.activeRepoId} size="sm" />
				<SyncStatusBadge
					status={db.state.activeRepo.syncStatus}
					syncState={db.state.activeRepoSyncState}
				/>
				<Button
					size="icon"
					variant="ghost"
					class="size-6 ms-auto [&_svg:not([class*='size-'])]:size-3"
					onclick={() => (showManageReposDialog = true)}
					title="Manage repositories"
				>
					<SettingsIcon />
				</Button>
			</div>

			<!-- Conflict warning -->
			{#if hasConflicts}
				<div class="py-1">
					<Button
						variant="destructive"
						size="sm"
						class="w-full h-7 text-xs gap-1.5"
						onclick={() => (showConflictDialog = true)}
					>
						<AlertTriangleIcon class="size-3" />
						{conflictFiles.length} Conflict{conflictFiles.length === 1 ? "" : "s"} - Resolve Now
					</Button>
				</div>
			{/if}

			<!-- Folder tree -->
			<SidebarMenu class="mt-1">
				{#if folderTree}
					{@render folderNode(folderTree, true)}
				{/if}
			</SidebarMenu>
		{/if}
	{/if}
</div>

<AddRepoDialog bind:open={showAddRepoDialog} onOpenChange={(open) => (showAddRepoDialog = open)} />

<ManageReposDialog
	bind:open={showManageReposDialog}
	onOpenChange={(open) => (showManageReposDialog = open)}
	onAddRepo={() => (showAddRepoDialog = true)}
/>

{#if db.state.activeRepoId && hasConflicts}
	<SyncConflictDialog
		bind:open={showConflictDialog}
		onOpenChange={(open) => (showConflictDialog = open)}
		repoId={db.state.activeRepoId}
		{conflictFiles}
	/>
{/if}

{#snippet folderNode(folder: SharedQueryFolder, isRoot: boolean)}
	{#if !isRoot}
		<Collapsible open={expandedFolders.has(folder.path)} onOpenChange={() => toggleFolder(folder.path)}>
			<SidebarMenuItem>
				<CollapsibleTrigger>
					{#snippet child({ props })}
						<SidebarMenuButton {...props}>
							<ChevronRightIcon class={["size-3 transition-transform", expandedFolders.has(folder.path) && "rotate-90"]} />
							<FolderIcon class="size-3" />
							<span class="flex-1 text-xs">{folder.name}</span>
							<Badge variant="secondary" class="text-xs h-4 px-1">
								{folder.queries.length + folder.children.reduce((acc, c) => acc + c.queries.length, 0)}
							</Badge>
						</SidebarMenuButton>
					{/snippet}
				</CollapsibleTrigger>
				<CollapsibleContent>
					<SidebarMenu class="ms-3 border-s border-sidebar-border ps-2">
						{#each folder.children as child (child.path)}
							{@render folderNode(child, false)}
						{/each}
						{#each folder.queries as query (query.id)}
							{@render queryNode(query)}
						{/each}
					</SidebarMenu>
				</CollapsibleContent>
			</SidebarMenuItem>
		</Collapsible>
	{:else}
		<!-- Root level items -->
		{#each folder.children as child (child.path)}
			{@render folderNode(child, false)}
		{/each}
		{#each folder.queries as query (query.id)}
			{@render queryNode(query)}
		{/each}
		{#if folder.children.length === 0 && folder.queries.length === 0}
			<div class="text-center py-4 text-muted-foreground px-2">
				<p class="text-xs">No queries in this repository</p>
			</div>
		{/if}
	{/if}
{/snippet}

{#snippet queryNode(query: SharedQuery)}
	<ContextMenu.Root>
		<ContextMenu.Trigger class="w-full">
			<SidebarMenuItem>
				<SidebarMenuButton
					class="h-auto py-1.5"
					onclick={() => handleQueryClick(query)}
				>
					<FileTextIcon class="size-3 shrink-0" />
					<div class="flex-1 min-w-0">
						<div class="text-xs truncate">{query.name}</div>
						{#if query.tags.length > 0}
							<div class="flex gap-1 mt-0.5">
								{#each query.tags.slice(0, 3) as tag (tag)}
									<Badge variant="outline" class="text-[10px] h-4 px-1">{tag}</Badge>
								{/each}
							</div>
						{/if}
					</div>
				</SidebarMenuButton>
			</SidebarMenuItem>
		</ContextMenu.Trigger>
		<ContextMenu.Content class="w-48">
			<ContextMenu.Item onclick={() => handleQueryClick(query)}>
				<PlayIcon class="size-4 me-2" />
				Open in Tab
			</ContextMenu.Item>
			<ContextMenu.Item onclick={() => handleCopyToLocal(query)}>
				<CopyIcon class="size-4 me-2" />
				Copy to Local Queries
			</ContextMenu.Item>
			<ContextMenu.Separator />
			<ContextMenu.Item
				class="text-destructive focus:text-destructive"
				onclick={() => handleDeleteQuery(query)}
			>
				<Trash2Icon class="size-4 me-2" />
				Delete
			</ContextMenu.Item>
		</ContextMenu.Content>
	</ContextMenu.Root>
{/snippet}
