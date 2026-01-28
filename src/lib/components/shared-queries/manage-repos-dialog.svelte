<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
	import { ScrollArea } from "$lib/components/ui/scroll-area";
	import RepoCard from "./repo-card.svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import type { SharedQueryRepo } from "$lib/types";
	import { PlusIcon, FolderGit2Icon } from "@lucide/svelte";
	import { toast } from "svelte-sonner";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		open: boolean;
		onOpenChange: (open: boolean) => void;
		onAddRepo: () => void;
	}

	let { open = $bindable(), onOpenChange, onAddRepo }: Props = $props();

	const db = useDatabase();

	let repoToDelete = $state<SharedQueryRepo | null>(null);
	let showDeleteConfirm = $state(false);

	function handleDeleteRequest(repo: SharedQueryRepo) {
		repoToDelete = repo;
		showDeleteConfirm = true;
	}

	function handleDeleteConfirm() {
		if (repoToDelete) {
			db.sharedRepos.removeRepo(repoToDelete.id);
			toast.success(m.shared_repo_removed({ name: repoToDelete.name }));
			repoToDelete = null;
		}
		showDeleteConfirm = false;
	}

	function handleDeleteCancel() {
		repoToDelete = null;
		showDeleteConfirm = false;
	}

	function handleAddClick() {
		onOpenChange(false);
		onAddRepo();
	}
</script>

<Dialog.Root {open} {onOpenChange}>
	<Dialog.Content class="max-w-lg max-h-[85vh] flex flex-col">
		<Dialog.Header>
			<Dialog.Title>{m.shared_manage_repos_title()}</Dialog.Title>
			<Dialog.Description>
				{m.shared_manage_repos_description()}
			</Dialog.Description>
		</Dialog.Header>

		<div class="flex-1 min-h-0 py-4">
			{#if db.state.sharedRepos.length === 0}
				<div class="flex flex-col items-center justify-center py-12 text-center">
					<FolderGit2Icon class="size-12 text-muted-foreground/50 mb-4" />
					<p class="text-sm text-muted-foreground mb-4">
						{m.shared_no_repos()}
					</p>
					<Button variant="outline" onclick={handleAddClick}>
						<PlusIcon class="size-4 me-2" />
						{m.shared_add_repo()}
					</Button>
				</div>
			{:else}
				<ScrollArea class="h-[400px] pr-4">
					<div class="space-y-3">
						{#each db.state.sharedRepos as repo (repo.id)}
							<RepoCard
								{repo}
								syncState={db.state.syncStateByRepo[repo.id]}
								onDelete={handleDeleteRequest}
							/>
						{/each}
					</div>
				</ScrollArea>
			{/if}
		</div>

		{#if db.state.sharedRepos.length > 0}
			<Dialog.Footer>
				<Button variant="outline" onclick={handleAddClick}>
					<PlusIcon class="size-4 me-2" />
					{m.shared_add_repo()}
				</Button>
			</Dialog.Footer>
		{/if}
	</Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={showDeleteConfirm}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{m.shared_remove_repo_title()}</AlertDialog.Title>
			<AlertDialog.Description>
				{m.shared_remove_repo_description({ name: repoToDelete?.name ?? '' })}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel onclick={handleDeleteCancel}>{m.common_cancel()}</AlertDialog.Cancel>
			<AlertDialog.Action onclick={handleDeleteConfirm}>{m.shared_remove()}</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
