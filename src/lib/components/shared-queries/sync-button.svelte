<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu/index.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
	import {
		RefreshCwIcon,
		ArrowDownIcon,
		ArrowUpIcon,
		GitCommitIcon,
		Loader2Icon,
		ChevronDownIcon
	} from "@lucide/svelte";

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

	async function handlePull() {
		if (!repo) return;
		try {
			await db.sharedRepos.pullRepo(repoId);
			toast.success("Repository updated");
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Pull failed: ${message}`);
		}
	}

	async function handlePush() {
		if (!repo) return;
		try {
			await db.sharedRepos.pushRepo(repoId);
			toast.success("Changes pushed");
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Push failed: ${message}`);
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
			toast.success("Repository synced");
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Sync failed: ${message}`);
		}
	}

	async function handleCommit() {
		if (!repo) return;
		try {
			await db.sharedRepos.commitChanges(repoId, "Update shared queries");
			toast.success("Changes committed");
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(`Commit failed: ${message}`);
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
						<span>Sync</span>
						<ChevronDownIcon class="size-3" />
					{/if}
				</Button>
			{/snippet}
		</DropdownMenu.Trigger>
		<DropdownMenu.Content align="end" class="w-48">
			<DropdownMenu.Item onclick={handleSync} disabled={isSyncing}>
				<RefreshCwIcon class="size-4 me-2" />
				Sync All
			</DropdownMenu.Item>
			<DropdownMenu.Separator />
			<DropdownMenu.Item onclick={handlePull} disabled={isSyncing}>
				<ArrowDownIcon class="size-4 me-2" />
				Pull Changes
				{#if needsPull}
					<span class="ms-auto text-xs text-muted-foreground">{syncState?.behindBy}</span>
				{/if}
			</DropdownMenu.Item>
			<DropdownMenu.Item onclick={handlePush} disabled={isSyncing || !needsPush}>
				<ArrowUpIcon class="size-4 me-2" />
				Push Changes
				{#if needsPush}
					<span class="ms-auto text-xs text-muted-foreground">{syncState?.aheadBy}</span>
				{/if}
			</DropdownMenu.Item>
			{#if hasUncommitted}
				<DropdownMenu.Separator />
				<DropdownMenu.Item onclick={handleCommit} disabled={isSyncing}>
					<GitCommitIcon class="size-4 me-2" />
					Commit Changes
					<span class="ms-auto text-xs text-muted-foreground">{syncState?.pendingChanges}</span>
				</DropdownMenu.Item>
			{/if}
			<DropdownMenu.Separator />
			<DropdownMenu.Item onclick={handleRefresh} disabled={isSyncing}>
				<RefreshCwIcon class="size-4 me-2" />
				Refresh Status
			</DropdownMenu.Item>
		</DropdownMenu.Content>
	</DropdownMenu.Root>
{/if}
