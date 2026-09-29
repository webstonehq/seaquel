<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import RefreshCwIcon from "@lucide/svelte/icons/refresh-cw";
	import ArrowDownIcon from "@lucide/svelte/icons/arrow-down";
	import ArrowUpIcon from "@lucide/svelte/icons/arrow-up";
	import GitCommitIcon from "@lucide/svelte/icons/git-commit";
	import Loader2Icon from "@lucide/svelte/icons/loader-2";
	import ChevronDownIcon from "@lucide/svelte/icons/chevron-down";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		repoId: string;
		size?: "default" | "sm" | "icon";
	}

	let { repoId, size = "sm" }: Props = $props();

	const db = useDatabase();

	const repo = $derived(db.state.sharedRepos.find((r) => r.id === repoId));
	const syncState = $derived(db.state.syncStateByRepo[repoId]);

	const isSyncing = $derived(syncState?.isSyncing ?? false);
	const hasUncommitted = $derived((syncState?.pendingChanges ?? 0) > 0);
	const needsPull = $derived((syncState?.behindBy ?? 0) > 0);
	const needsPush = $derived((syncState?.aheadBy ?? 0) > 0);

	let showCommitDialog = $state(false);
	let commitMessage = $state("");

	async function handlePull() {
		if (!repo) return;
		try {
			await db.sharedRepos.pullRepo(repoId);
			toast.success(m.shared_repo_updated());
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(m.shared_pull_failed({ message }));
		}
	}

	async function handlePush() {
		if (!repo) return;
		try {
			await db.sharedRepos.pushRepo(repoId);
			toast.success(m.shared_changes_pushed());
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(m.shared_push_failed({ message }));
		}
	}

	async function handleSync() {
		if (!repo) return;
		try {
			// Pull first, then push if needed
			await db.sharedRepos.pullRepo(repoId);
			if (needsPush) {
				await db.sharedRepos.pushRepo(repoId);
			}
			toast.success(m.shared_repo_synced());
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(m.shared_sync_failed({ message }));
		}
	}

	function openCommitDialog() {
		commitMessage = "";
		showCommitDialog = true;
	}

	async function handleCommit() {
		if (!repo || !commitMessage.trim()) return;
		const message = commitMessage.trim();
		showCommitDialog = false;
		try {
			await db.sharedRepos.commitChanges(repoId, message);
			toast.success(m.shared_changes_committed());
		} catch (error) {
			const msg = error instanceof Error ? error.message : String(error);
			errorToast(m.shared_commit_failed({ message: msg }));
		}
	}

	async function handleRefresh() {
		if (!repo) return;
		await db.sharedRepos.refreshRepoStatus(repoId);
	}
</script>

{#if repo}
	<DropdownMenu.Root>
		<DropdownMenu.Trigger>
			{#snippet child({ props })}
				<Button
					{...props}
					variant="outline"
					{size}
					disabled={isSyncing}
					class="gap-1"
				>
					{#if isSyncing}
						<Loader2Icon class="size-4 animate-spin" />
					{:else}
						<RefreshCwIcon class="size-4" />
					{/if}
					{#if size !== "icon"}
						<span>{m.shared_sync()}</span>
						<ChevronDownIcon class="size-3" />
					{/if}
				</Button>
			{/snippet}
		</DropdownMenu.Trigger>
		<DropdownMenu.Content align="end" class="w-48">
			<DropdownMenu.Item onclick={handleSync} disabled={isSyncing}>
				<RefreshCwIcon class="size-4 me-2" />
				{m.shared_sync_all()}
			</DropdownMenu.Item>
			<DropdownMenu.Separator />
			<DropdownMenu.Item onclick={handlePull} disabled={isSyncing}>
				<ArrowDownIcon class="size-4 me-2" />
				{m.shared_pull_changes()}
				{#if needsPull}
					<span class="ms-auto text-xs text-muted-foreground">{syncState?.behindBy}</span>
				{/if}
			</DropdownMenu.Item>
			<DropdownMenu.Item onclick={handlePush} disabled={isSyncing || !needsPush}>
				<ArrowUpIcon class="size-4 me-2" />
				{m.shared_push_changes()}
				{#if needsPush}
					<span class="ms-auto text-xs text-muted-foreground">{syncState?.aheadBy}</span>
				{/if}
			</DropdownMenu.Item>
			{#if hasUncommitted}
				<DropdownMenu.Separator />
				<DropdownMenu.Item onclick={openCommitDialog} disabled={isSyncing}>
					<GitCommitIcon class="size-4 me-2" />
					{m.shared_commit_changes()}
					<span class="ms-auto text-xs text-muted-foreground">{syncState?.pendingChanges}</span>
				</DropdownMenu.Item>
			{/if}
			<DropdownMenu.Separator />
			<DropdownMenu.Item onclick={handleRefresh} disabled={isSyncing}>
				<RefreshCwIcon class="size-4 me-2" />
				{m.shared_refresh_status()}
			</DropdownMenu.Item>
		</DropdownMenu.Content>
	</DropdownMenu.Root>
{/if}

<!-- Commit Message Dialog -->
<Dialog.Root bind:open={showCommitDialog}>
	<Dialog.Content class="max-w-md">
		<Dialog.Header>
			<Dialog.Title>{m.shared_commit_changes()}</Dialog.Title>
		</Dialog.Header>
		<div class="py-4">
			<Input
				bind:value={commitMessage}
				placeholder={m.shared_commit_message_placeholder()}
				onkeydown={(e) => e.key === "Enter" && handleCommit()}
				autofocus
			/>
		</div>
		<Dialog.Footer class="gap-2">
			<Button variant="outline" onclick={() => { showCommitDialog = false; }}>
				{m.header_button_cancel()}
			</Button>
			<Button onclick={handleCommit} disabled={!commitMessage.trim()}>
				{m.shared_commit_changes()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
