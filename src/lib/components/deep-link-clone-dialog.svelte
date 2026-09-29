<script lang="ts">
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { deepLinkDialogStore } from "$lib/stores/deep-link-dialog.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
	import LinkIcon from "@lucide/svelte/icons/link";
	import LoaderIcon from "@lucide/svelte/icons/loader";

	const db = useDatabase();

	let isCloning = $state(false);
	let username = $state("");
	let password = $state("");
	let error = $state("");

	function handleOpenChange(open: boolean) {
		if (!open) {
			deepLinkDialogStore.resolve(false);
			resetState();
		}
	}

	function resetState() {
		isCloning = false;
		username = "";
		password = "";
		error = "";
	}

	async function handleClone() {
		isCloning = true;
		error = "";

		try {
			const { appDataDir } = await import("@tauri-apps/api/path");
			const dataDir = await appDataDir();
			const { join } = await import("@tauri-apps/api/path");

			// Derive a local path from the repo URL
			const repoName = deepLinkDialogStore.repoUrl
				.replace(/\.git$/, "")
				.split("/")
				.pop() || "shared-queries";
			const localPath = await join(dataDir, "shared-repos", repoName);

			const credentials = username || password
				? { username, password }
				: undefined;

			await db.sharedRepos.cloneRepo(
				repoName,
				deepLinkDialogStore.repoUrl,
				localPath,
				credentials,
			);

			toast.success(`Cloned repository: ${repoName}`);
			deepLinkDialogStore.resolve(true);
			resetState();
		} catch (err) {
			error = err instanceof Error ? err.message : String(err);
		} finally {
			isCloning = false;
		}
	}

	function handleCancel() {
		deepLinkDialogStore.resolve(false);
		resetState();
	}
</script>

<Dialog.Root bind:open={deepLinkDialogStore.open} onOpenChange={handleOpenChange}>
	<Dialog.Content class="max-w-md">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<LinkIcon class="size-5" />
				Clone Shared Repository?
			</Dialog.Title>
			<Dialog.Description>
				This link references a {deepLinkDialogStore.resourceType} in a repository you don't have locally.
			</Dialog.Description>
		</Dialog.Header>

		<div class="space-y-4 py-4">
			<div class="space-y-1">
				<p class="text-sm font-medium">Repository</p>
				<p class="text-sm text-muted-foreground break-all">{deepLinkDialogStore.repoUrl}</p>
			</div>

			<div class="space-y-1">
				<p class="text-sm font-medium capitalize">{deepLinkDialogStore.resourceType}</p>
				<p class="text-sm text-muted-foreground break-all">{deepLinkDialogStore.filePath}</p>
			</div>

			<div class="space-y-2">
				<p class="text-sm font-medium">Credentials (optional)</p>
				<Input
					bind:value={username}
					placeholder="Username"
					class="h-8 text-sm"
					disabled={isCloning}
				/>
				<Input
					bind:value={password}
					placeholder="Password or token"
					type="password"
					class="h-8 text-sm"
					disabled={isCloning}
				/>
			</div>

			{#if error}
				<p class="text-sm text-destructive">{error}</p>
			{/if}
		</div>

		<Dialog.Footer class="gap-2">
			<Button variant="outline" onclick={handleCancel} disabled={isCloning}>
				Cancel
			</Button>
			<Button onclick={handleClone} disabled={isCloning}>
				{#if isCloning}
					<LoaderIcon class="size-4 me-2 animate-spin" />
					Cloning...
				{:else}
					Clone & Open
				{/if}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
