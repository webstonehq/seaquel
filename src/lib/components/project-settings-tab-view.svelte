<script lang="ts">
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import * as Breadcrumb from "$lib/components/ui/breadcrumb/index.js";
	import DeleteConfirmDialog from "$lib/components/delete-confirm-dialog.svelte";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import SyncButton from "$lib/components/shared-queries/sync-button.svelte";
	import SyncStatusBadge from "$lib/components/shared-queries/sync-status-badge.svelte";
	import { projectSyncStatus } from "$lib/components/shared-queries/project-sync-status";
	import FolderOpenIcon from "@lucide/svelte/icons/folder-open";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import LinkIcon from "@lucide/svelte/icons/link";
	import SettingsIcon from "@lucide/svelte/icons/settings";
	import UsersIcon from "@lucide/svelte/icons/users";
	import AlertTriangleIcon from "@lucide/svelte/icons/alert-triangle";
	import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
	import { untrack } from "svelte";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { showErrorUnlessShown } from "$lib/errors";
	import { m } from "$lib/paraglide/messages.js";
	import { isTauri } from "$lib/utils/environment";
	import { DEFAULT_PROJECT_ID } from "$lib/types";
	import { buildDeepLinkUrl } from "$lib/services/deep-link";
	import { linkProjectDialogStore } from "$lib/stores/link-project-dialog.svelte.js";
	import { unlinkProjectDialogStore } from "$lib/stores/unlink-project-dialog.svelte.js";
	import type { SettingsTab } from "$lib/types";

	type ProjectSettingsSection = "general" | "sharing" | "danger";

	interface Props {
		tab: SettingsTab;
	}

	let { tab }: Props = $props();

	const db = useDatabase();

	// Navigation state
	const activeSection = $derived((tab.activeView ?? "all") as ProjectSettingsSection | "all");

	function setSection(section: ProjectSettingsSection | "all") {
		db.settingsTabs.setSettingsView(tab.id, section);
	}

	const navItems = $derived([
		{ id: "general" as const, name: m.project_settings_general(), icon: SettingsIcon },
		...(isTauri() ? [{ id: "sharing" as const, name: m.project_settings_sharing(), icon: UsersIcon }] : []),
		{ id: "danger" as const, name: "Danger Zone", icon: AlertTriangleIcon },
	]);

	const activeSectionName = $derived(activeSection === "all" ? null : (navItems.find((i) => i.id === activeSection)?.name ?? m.project_settings_general()));

	const projectId = $derived(db.state.activeProjectId ?? "");
	const project = $derived(db.state.projects.find((p) => p.id === projectId));
	const repo = $derived(project?.gitRepoPath ? db.sharedRepos.repoForProject(projectId) : null);
	const syncState = $derived(repo ? (db.state.syncStateByRepo[repo.id] ?? null) : null);

	// The project's directory in the repo, as Core's scan reports it (the
	// directory Core stored when it linked the project; never derived from
	// the project's name). `null` when there's none or the scan fails.
	// The repo's path alone: a status refresh replaces the repo object but
	// not its path, so the scan runs again only when the path changes.
	const repoPath = $derived(repo?.path ?? null);
	const sharedDir = $derived.by(() => {
		const path = repoPath;
		const id = projectId;
		if (!path) return Promise.resolve(null);
		return db.sharedRepos
			.scan(path)
			.then((preview) => preview.projects.find((p) => p.linkedProjectIds.includes(id))?.dir ?? null)
			.catch(() => null);
	});

	const copyProjectLink = async () => {
		const dir = await sharedDir;
		if (!repo?.remoteUrl || !dir) return;
		const filePath = `.seaquel/projects/${dir}`;
		const url = buildDeepLinkUrl(repo.remoteUrl, repo.branch, filePath);
		await navigator.clipboard.writeText(url);
		toast.success("Link copied to clipboard");
	};

	let nameInput = $state("");
	let descriptionInput = $state("");
	let remoteUrlInput = $state("");
	let pendingGitRepoPath = $state<string | undefined>(undefined);
	let hasPendingGitChange = $state(false);

	// What the form loaded (phase 5d-1): saving sends only the fields that
	// differ from it, so another window's change to another field survives.
	// While nothing is edited, the form follows the stored project; once the
	// user edits, a change from another window shows a banner instead.
	let loadedFor = $state<string | null>(null);
	let baselineName = $state("");
	let baselineDescription = $state("");
	let openedRevision = $state(0);
	const revision = $derived(db.state.libraryRemoteRevision[`project:${projectId}`] ?? 0);
	const dirty = $derived(
		nameInput !== baselineName || descriptionInput !== baselineDescription || hasPendingGitChange,
	);
	const changedElsewhere = $derived(dirty && revision > openedRevision);

	const loadForm = () => {
		if (!project) return;
		nameInput = project.name;
		descriptionInput = project.description ?? "";
		baselineName = nameInput;
		baselineDescription = descriptionInput;
		remoteUrlInput = repo?.remoteUrl ?? "";
		pendingGitRepoPath = project.gitRepoPath;
		hasPendingGitChange = false;
		openedRevision = revision;
		loadedFor = projectId;
	};

	// Load the form for a project, and follow its stored fields while the
	// form isn't being edited.
	$effect(() => {
		if (!project) return;
		// Tracked: the project's fields, its repo and its remote changes.
		void [projectId, project.name, project.description, project.gitRepoPath, repo, revision];
		untrack(() => {
			if (loadedFor !== projectId || !dirty) loadForm();
		});
	});

	const handleSave = async () => {
		if (!nameInput.trim()) return;

		const savePendingGitRepoPath = pendingGitRepoPath;
		const saveHasPendingGitChange = hasPendingGitChange;
		const saveRemoteUrlInput = remoteUrlInput.trim();

		const updates: { name?: string; description?: string } = {};
		if (nameInput.trim() !== baselineName.trim()) updates.name = nameInput.trim();
		if (descriptionInput.trim() !== baselineDescription.trim()) {
			updates.description = descriptionInput.trim() || undefined;
		}
		if (Object.keys(updates).length > 0) {
			try {
				await db.projects.update(projectId, updates);
			} catch (error) {
				// A taken name, say: the form keeps what the user typed.
				showErrorUnlessShown(error);
				return;
			}
		}
		baselineName = nameInput.trim();
		baselineDescription = descriptionInput.trim();
		nameInput = baselineName;
		descriptionInput = baselineDescription;
		openedRevision = revision;

		const linkedPath = project?.gitRepoPath;
		if (saveHasPendingGitChange && savePendingGitRepoPath !== linkedPath) {
			// Every question first, so a cancel changes nothing (Q30, Q31).
			// A new link asks which connections to share: the user's own,
			// the ones shared from here included (a relink keeps them).
			let share: string[] = [];
			if (savePendingGitRepoPath) {
				const ticked = await linkProjectDialogStore.prompt(
					project?.name ?? "",
					db.state.connections
						.filter(
							(c) =>
								c.projectId === projectId &&
								(!c.sharedConnectionId || c.sharedOrigin === "exported"),
						)
						.map((c) => ({ id: c.id, name: c.name, isLocalOnly: c.isLocalOnly })),
				);
				if (ticked === null) return;
				share = ticked;
			}
			try {
				// Unlinking keeps the user's own connections and asks about the
				// ones the repo brought; a link failing afterwards loses neither.
				if (linkedPath) {
					const unlinked = await db.projects.unlinkWithConfirmation(projectId, (name, imported) =>
						unlinkProjectDialogStore.prompt(name, imported),
					);
					if (!unlinked) return;
				}
				if (savePendingGitRepoPath) {
					await db.projects.linkProject(projectId, savePendingGitRepoPath, share);
				}
			} catch (error) {
				showErrorUnlessShown(error);
				return;
			}
		}

		hasPendingGitChange = false;

		const updatedRepo = db.sharedRepos.repoForProject(projectId);
		if (updatedRepo && saveRemoteUrlInput !== (updatedRepo.remoteUrl ?? "")) {
			try {
				await db.sharedRepos.setRemoteUrl(updatedRepo.id, saveRemoteUrlInput);
			} catch (error) {
				const message = error instanceof Error ? error.message : String(error);
				errorToast(`Failed to set remote URL: ${message}`);
			}
		}

		toast.success("Project settings saved");
	};

	const handleSelectFolder = async () => {
		if (!isTauri()) return;
		try {
			const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
			const selected = await openDialog({
				directory: true,
				multiple: false,
				title: m.project_settings_select_folder(),
			});
			if (selected) {
				pendingGitRepoPath = selected as string;
				hasPendingGitChange = true;

				try {
					const { getRemoteUrl } = await import("$lib/services/git");
					const detectedUrl = await getRemoteUrl(selected as string);
					if (detectedUrl) {
						remoteUrlInput = detectedUrl;
					}
				} catch {
					// No remote configured
				}
			}
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Failed to select folder: ${message}`);
		}
	};

	const handleInitRepo = async () => {
		if (!isTauri()) return;
		try {
			const { open: openDialog } = await import("@tauri-apps/plugin-dialog");
			const selected = await openDialog({
				directory: true,
				multiple: false,
				title: m.project_settings_init_repo(),
			});
			if (selected) {
				const { initRepo, getRemoteUrl } = await import("$lib/services/git");
				await initRepo(selected as string);
				pendingGitRepoPath = selected as string;
				hasPendingGitChange = true;
				toast.success(m.shared_repo_initialized({ name: project?.name ?? "" }));

				try {
					const detectedUrl = await getRemoteUrl(selected as string);
					if (detectedUrl) {
						remoteUrlInput = detectedUrl;
					}
				} catch {
					// No remote configured
				}
			}
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Failed to initialize repository: ${message}`);
		}
	};

	const handleRemoveGitDir = () => {
		pendingGitRepoPath = undefined;
		hasPendingGitChange = true;
	};

	// Scroll spy for highlighting visible section in sidebar nav
	let visibleSectionId = $state<string | null>(null);

	const canDelete = $derived(projectId !== DEFAULT_PROJECT_ID && db.state.projects.length > 1);
	let showDeleteConfirm = $state(false);

	const handleDeleteProject = async () => {
		let removed = false;
		try {
			removed = await db.projects.remove(projectId);
		} catch (error) {
			showErrorUnlessShown(error);
		} finally {
			showDeleteConfirm = false;
		}
		if (removed) db.settingsTabs.remove(tab.id);
	};
</script>

<Sidebar.Provider style="min-height: 0;" class="h-full">
	<Sidebar.Root collapsible="none" class="hidden md:flex border-r">
		<Sidebar.Content>
			<Sidebar.Group>
				<Sidebar.GroupLabel class="gap-2">
					<SettingsIcon class="size-4" />
					<span>{m.project_settings_title()}</span>
				</Sidebar.GroupLabel>
				<Sidebar.GroupContent>
					<Sidebar.Menu>
						{#each navItems as item (item.id)}
							<Sidebar.MenuItem>
								<Sidebar.MenuButton
									isActive={activeSection === item.id || (activeSection === "all" && visibleSectionId === item.id)}
									onclick={() => setSection(item.id)}
								>
									<item.icon class="size-4" />
									<span>{item.name}</span>
								</Sidebar.MenuButton>
							</Sidebar.MenuItem>
						{/each}
					</Sidebar.Menu>
				</Sidebar.GroupContent>
			</Sidebar.Group>
		</Sidebar.Content>
	</Sidebar.Root>
	<main class="flex flex-1 flex-col overflow-hidden">
		<header class="flex h-12 shrink-0 items-center gap-2 border-b">
			<div class="flex items-center gap-2 px-4">
				<Breadcrumb.Root>
					<Breadcrumb.List>
						<Breadcrumb.Item class="hidden md:block">
							{#if activeSection === "all"}
								<Breadcrumb.Page>{m.project_settings_title()}</Breadcrumb.Page>
							{:else}
								<Breadcrumb.Link href="#" onclick={() => setSection("all")}>
									{m.project_settings_title()}
								</Breadcrumb.Link>
							{/if}
						</Breadcrumb.Item>
						{#if activeSectionName}
							<Breadcrumb.Separator class="hidden md:block" />
							<Breadcrumb.Item>
								<Breadcrumb.Page>{activeSectionName}</Breadcrumb.Page>
							</Breadcrumb.Item>
						{/if}
					</Breadcrumb.List>
				</Breadcrumb.Root>
			</div>
		</header>
		<div class="flex flex-1 flex-col gap-6 overflow-y-auto p-4 pt-4" {@attach (container) => {
			const _view = activeSection;
			const sectionEls = container.querySelectorAll<HTMLElement>("[data-section]");
			if (sectionEls.length === 0) return;
			const observer = new IntersectionObserver(
				(entries) => {
					let topSection: { id: string; top: number } | null = null;
					for (const entry of entries) {
						if (entry.isIntersecting) {
							const rect = entry.boundingClientRect;
							if (!topSection || rect.top < topSection.top) {
								topSection = { id: entry.target.getAttribute("data-section")!, top: rect.top };
							}
						}
					}
					if (topSection) {
						visibleSectionId = topSection.id;
					}
				},
				{ root: container, rootMargin: "0px 0px -60% 0px", threshold: 0 },
			);
			for (const el of sectionEls) observer.observe(el);
			return () => observer.disconnect();
		}}>
			{#if activeSection === "all" || activeSection === "general"}
				<div class="space-y-6" data-section="general">
					<div>
						<h2 class="text-lg font-medium">{m.project_settings_general()}</h2>
						<p class="text-sm text-muted-foreground mt-1">
							Basic information about this project
						</p>
					</div>

					{#if changedElsewhere}
						<div
							class="flex items-center justify-between gap-3 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm"
							role="status"
						>
							<span>{m.library_changed_elsewhere()}</span>
							<Button size="sm" variant="outline" onclick={loadForm}>
								<RefreshCwIcon class="size-3.5 me-1" />
								{m.library_changed_elsewhere_reload()}
							</Button>
						</div>
					{/if}

					<div class="space-y-4">
						<div class="space-y-2">
							<label for="project-name" class="text-sm font-medium">
								{m.project_name_placeholder()}
							</label>
							<Input
								id="project-name"
								bind:value={nameInput}
								placeholder={m.project_name_placeholder()}
								onkeydown={(e) => e.key === "Enter" && handleSave()}
							/>
						</div>
						<div class="space-y-2">
							<label for="project-description" class="text-sm font-medium">
								Description
							</label>
							<textarea
								id="project-description"
								bind:value={descriptionInput}
								placeholder="Optional project description"
								class="w-full px-3 py-2 border rounded-md bg-background text-sm min-h-16 resize-y"
							></textarea>
						</div>
					</div>

					<div class="pt-2">
						<Button onclick={handleSave} disabled={!nameInput.trim()}>
							{m.common_save()}
						</Button>
					</div>
				</div>
			{/if}

			{#if activeSection === "all" || activeSection === "sharing"}
				<div class="space-y-6" data-section="sharing">
					<div>
						<h2 class="text-lg font-medium">{m.project_settings_sharing()}</h2>
						<p class="text-sm text-muted-foreground mt-1">
							{m.project_settings_sharing_description()}
						</p>
					</div>

					{#if !pendingGitRepoPath}
						<div class="rounded-md border border-dashed p-6 text-center space-y-3">
							<p class="text-sm text-muted-foreground">
								{m.project_settings_no_git()}
							</p>
							<div class="flex gap-2 justify-center">
								<Button variant="outline" size="sm" onclick={handleSelectFolder}>
									<FolderOpenIcon class="size-3 me-1" />
									{m.project_settings_select_folder()}
								</Button>
								<Button variant="outline" size="sm" onclick={handleInitRepo}>
									{m.project_settings_init_repo()}
								</Button>
							</div>
						</div>
					{:else}
						<div class="space-y-4">
							<div class="space-y-2">
								<span class="text-sm font-medium">{m.project_settings_git_path()}</span>
								<div class="flex items-center gap-2 min-w-0">
									<code class="flex-1 min-w-0 text-xs bg-muted px-2 py-1.5 rounded truncate block">
										{pendingGitRepoPath}
									</code>
									<Button variant="ghost" size="sm" class="shrink-0" onclick={handleSelectFolder}>
										Change
									</Button>
								</div>
							</div>

							<div class="space-y-2">
								<span class="text-sm font-medium">
									{m.project_settings_remote_url()}
								</span>
								<Input
									bind:value={remoteUrlInput}
									placeholder={m.shared_git_remote_url()}
									class="text-sm"
								/>
							</div>

							{#if repo}
								<div class="flex items-center gap-2 min-w-0 flex-wrap">
									<SyncButton repoId={repo.id} size="sm" />
									<SyncStatusBadge
										status={projectSyncStatus(repo.syncStatus, db.state.sharedSyncSkipped[projectId])}
										{syncState}
									/>
									{#if repo.lastSyncAt}
										<span class="text-xs text-muted-foreground ms-auto truncate">
											Last synced {repo.lastSyncAt.toLocaleString()}
										</span>
									{/if}
								</div>
							{/if}

							<div class="flex items-center gap-3">
								{#await sharedDir then dir}
									{#if repo?.remoteUrl && dir}
										<button
											class="text-xs text-muted-foreground hover:text-foreground cursor-pointer flex items-center gap-1"
											onclick={copyProjectLink}
										>
											<LinkIcon class="size-3" />
											{m.share_project()}
										</button>
									{/if}
								{/await}
								<button
									class="text-xs text-muted-foreground hover:text-destructive cursor-pointer flex items-center gap-1"
									onclick={handleRemoveGitDir}
								>
									<Trash2Icon class="size-3" />
									{m.project_settings_remove_git()}
								</button>
							</div>
						</div>

						<div class="pt-2">
							<Button onclick={handleSave} disabled={!nameInput.trim()}>
								{m.common_save()}
							</Button>
						</div>
					{/if}
				</div>
			{/if}

			{#if activeSection === "all" || activeSection === "danger"}
				<div class="space-y-6" data-section="danger">
					<div>
						<h2 class="text-lg font-medium">Danger Zone</h2>
						<p class="text-sm text-muted-foreground mt-1">
							Irreversible actions for this project
						</p>
					</div>

					<div class="rounded-lg border border-destructive/50 p-4 space-y-3">
						<div class="flex items-center justify-between">
							<div>
								<p class="text-sm font-medium">{m.project_delete()}</p>
								<p class="text-xs text-muted-foreground">
									Permanently delete this project and all its tabs. This cannot be undone.
								</p>
							</div>
							<Button
								variant="destructive"
								size="sm"
								disabled={!canDelete}
								onclick={() => { showDeleteConfirm = true; }}
							>
								<Trash2Icon class="size-4 me-1" />
								{m.project_delete()}
							</Button>
						</div>
						{#if !canDelete}
							<p class="text-xs text-muted-foreground">
								You cannot delete the default or last remaining project.
							</p>
						{/if}
					</div>
				</div>
			{/if}
		</div>
	</main>
</Sidebar.Provider>

<!-- Delete Project Confirmation -->
<DeleteConfirmDialog
	bind:open={showDeleteConfirm}
	title={m.project_delete_dialog_title()}
	description={m.project_delete_dialog_description({ name: project?.name ?? "" })}
	cancelText={m.header_button_cancel()}
	confirmText={m.project_delete_confirm()}
	onconfirm={handleDeleteProject}
/>
