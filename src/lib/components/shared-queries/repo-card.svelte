<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";
	import SyncStatusBadge from "./sync-status-badge.svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import type { SharedQueryRepo, SyncState } from "$lib/types";
	import {
		ChevronDownIcon,
		FolderOpenIcon,
		Trash2Icon,
		SaveIcon,
		XIcon
	} from "@lucide/svelte";
	import { openPath } from "$lib/api/tauri";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		repo: SharedQueryRepo;
		syncState?: SyncState;
		onDelete: (repo: SharedQueryRepo) => void;
	}

	let { repo, syncState, onDelete }: Props = $props();

	const db = useDatabase();

	let isExpanded = $state(false);
	let editName = $state('');
	let editRemoteUrl = $state('');
	let editBranch = $state('');
	let isSaving = $state(false);

	// Sync edit fields from repo (on mount and when expanded or repo changes)
	$effect.pre(() => {
		editName = repo.name;
		editRemoteUrl = repo.remoteUrl;
		editBranch = repo.branch;
	});

	const hasChanges = $derived(
		editName !== repo.name ||
		editRemoteUrl !== repo.remoteUrl ||
		editBranch !== repo.branch
	);

	const truncatedPath = $derived(() => {
		const maxLength = 40;
		if (repo.path.length <= maxLength) return repo.path;
		return "..." + repo.path.slice(-maxLength);
	});

	async function handleSave() {
		if (!editName.trim()) {
			errorToast(m.shared_name_empty());
			return;
		}

		isSaving = true;

		try {
			// Update name and branch via settings
			if (editName !== repo.name || editBranch !== repo.branch) {
				db.sharedRepos.updateRepoSettings(repo.id, {
					name: editName.trim(),
					branch: editBranch.trim() || "main"
				});
			}

			// Update remote URL if changed (requires git command)
			if (editRemoteUrl !== repo.remoteUrl) {
				await db.sharedRepos.setRemoteUrl(repo.id, editRemoteUrl.trim());
			}

			toast.success(m.shared_settings_saved());
			isExpanded = false;
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(m.shared_save_settings_failed({ message }));
		} finally {
			isSaving = false;
		}
	}

	function handleCancel() {
		editName = repo.name;
		editRemoteUrl = repo.remoteUrl;
		editBranch = repo.branch;
		isExpanded = false;
	}

	async function handleOpenFolder() {
		try {
			await openPath(repo.path);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(m.shared_open_folder_failed({ message }));
		}
	}
</script>

<Collapsible bind:open={isExpanded} class="border rounded-lg">
	<div class="flex items-center gap-2 p-3">
		<CollapsibleTrigger class="flex-1 flex items-center gap-2 text-left">
			<ChevronDownIcon
				class={["size-4 text-muted-foreground transition-transform", isExpanded && "rotate-180"]}
			/>
			<div class="flex-1 min-w-0">
				<div class="font-medium text-sm truncate">{repo.name}</div>
				<div class="text-xs text-muted-foreground truncate">{truncatedPath()}</div>
			</div>
		</CollapsibleTrigger>

		<SyncStatusBadge status={repo.syncStatus} {syncState} showCounts={false} />

		<Button
			variant="ghost"
			size="icon"
			class="size-7 [&_svg:not([class*='size-'])]:size-3.5"
			onclick={handleOpenFolder}
			title={m.shared_open_folder()}
		>
			<FolderOpenIcon />
		</Button>

		<Button
			variant="ghost"
			size="icon"
			class="size-7 text-destructive hover:text-destructive [&_svg:not([class*='size-'])]:size-3.5"
			onclick={() => onDelete(repo)}
			title={m.shared_remove_repo()}
		>
			<Trash2Icon />
		</Button>
	</div>

	<CollapsibleContent>
		<div class="px-3 pb-3 pt-1 border-t space-y-3">
			<div class="space-y-1.5">
				<Label for="edit-name-{repo.id}" class="text-xs">{m.shared_name()}</Label>
				<Input
					id="edit-name-{repo.id}"
					bind:value={editName}
					class="h-8 text-sm"
					disabled={isSaving}
				/>
			</div>

			<div class="space-y-1.5">
				<Label for="edit-remote-{repo.id}" class="text-xs">{m.shared_remote_url()}</Label>
				<Input
					id="edit-remote-{repo.id}"
					bind:value={editRemoteUrl}
					placeholder="git@github.com:org/repo.git"
					class="h-8 text-sm"
					disabled={isSaving}
				/>
				{#if !repo.remoteUrl}
					<p class="text-xs text-muted-foreground">{m.shared_add_remote_hint()}</p>
				{/if}
			</div>

			<div class="space-y-1.5">
				<Label for="edit-branch-{repo.id}" class="text-xs">{m.shared_branch()}</Label>
				<Input
					id="edit-branch-{repo.id}"
					bind:value={editBranch}
					placeholder="main"
					class="h-8 text-sm"
					disabled={isSaving}
				/>
			</div>

			<div class="space-y-1.5">
				<Label class="text-xs">{m.shared_local_path()}</Label>
				<p class="text-xs text-muted-foreground break-all">{repo.path}</p>
			</div>

			{#if hasChanges}
				<div class="flex justify-end gap-2 pt-2">
					<Button
						variant="outline"
						size="sm"
						onclick={handleCancel}
						disabled={isSaving}
					>
						<XIcon class="size-3.5 me-1" />
						{m.common_cancel()}
					</Button>
					<Button size="sm" onclick={handleSave} disabled={isSaving}>
						<SaveIcon class="size-3.5 me-1" />
						{m.common_save()}
					</Button>
				</div>
			{/if}
		</div>
	</CollapsibleContent>
</Collapsible>
