<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { Input } from "$lib/components/ui/input";
	import { Label } from "$lib/components/ui/label";
	import { Tabs, TabsContent, TabsList, TabsTrigger } from "$lib/components/ui/tabs";
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";
	import { FolderIcon, GitBranchIcon, Loader2Icon } from "@lucide/svelte";
	import { open as openDialog } from "@tauri-apps/plugin-dialog";
	import { join, appDataDir } from "@tauri-apps/api/path";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		open: boolean;
		onOpenChange: (open: boolean) => void;
	}

	let { open = $bindable(), onOpenChange }: Props = $props();

	const db = useDatabase();

	let mode = $state<"clone" | "init">("clone");
	let name = $state("");
	let remoteUrl = $state("");
	let localPath = $state("");
	let isLoading = $state(false);

	// Generate default local path when name changes
	$effect(() => {
		if (name && !localPath) {
			generateDefaultPath();
		}
	});

	async function generateDefaultPath() {
		try {
			const appDir = await appDataDir();
			const safeName = name.toLowerCase().replace(/[^a-z0-9-]/g, "-").replace(/-+/g, "-");
			localPath = await join(appDir, "shared-queries", safeName);
		} catch (error) {
			console.error("Failed to generate default path:", error);
		}
	}

	async function browseFolder() {
		try {
			const selected = await openDialog({
				directory: true,
				multiple: false,
				title: m.shared_select_folder_title()
			});
			if (selected && typeof selected === "string") {
				localPath = selected;
			}
		} catch (error) {
			console.error("Failed to open folder dialog:", error);
		}
	}

	async function handleSubmit() {
		if (!name.trim()) {
			errorToast(m.shared_name_required());
			return;
		}

		if (!localPath.trim()) {
			errorToast(m.shared_path_required());
			return;
		}

		if (mode === "clone" && !remoteUrl.trim()) {
			errorToast(m.shared_remote_required());
			return;
		}

		isLoading = true;

		try {
			if (mode === "clone") {
				await db.sharedRepos.cloneRepo(name, remoteUrl, localPath);
				toast.success(m.shared_repo_cloned({ name }));
			} else {
				await db.sharedRepos.initRepo(name, localPath);
				toast.success(m.shared_repo_initialized({ name }));
			}

			// Reset form and close dialog
			resetForm();
			onOpenChange(false);
		} catch (error) {
			const message = error instanceof Error ? error.message : String(error);
			errorToast(mode === "clone" ? m.shared_clone_failed({ message }) : m.shared_init_failed({ message }));
		} finally {
			isLoading = false;
		}
	}

	function resetForm() {
		name = "";
		remoteUrl = "";
		localPath = "";
		mode = "clone";
	}

	function handleOpenChange(isOpen: boolean) {
		if (!isOpen) {
			resetForm();
		}
		onOpenChange(isOpen);
	}
</script>

<Dialog.Root {open} onOpenChange={handleOpenChange}>
	<Dialog.Content class="max-w-lg">
		<Dialog.Header>
			<Dialog.Title>{m.shared_add_repo_title()}</Dialog.Title>
			<Dialog.Description>
				{m.shared_add_repo_description()}
			</Dialog.Description>
		</Dialog.Header>

		<Tabs bind:value={mode} class="mt-4">
			<TabsList class="grid w-full grid-cols-2">
				<TabsTrigger value="clone">
					<GitBranchIcon class="size-4 me-2" />
					{m.shared_clone_repo()}
				</TabsTrigger>
				<TabsTrigger value="init">
					<FolderIcon class="size-4 me-2" />
					{m.shared_new_repo()}
				</TabsTrigger>
			</TabsList>

			<TabsContent value="clone" class="space-y-4 mt-4">
				<div class="space-y-2">
					<Label for="name">{m.shared_repo_name()}</Label>
					<Input
						id="name"
						bind:value={name}
						placeholder="Team Queries"
						disabled={isLoading}
					/>
				</div>

				<div class="space-y-2">
					<Label for="remote-url">{m.shared_git_remote_url()}</Label>
					<Input
						id="remote-url"
						bind:value={remoteUrl}
						placeholder="git@github.com:team/queries.git"
						disabled={isLoading}
					/>
					<p class="text-xs text-muted-foreground">
						{m.shared_git_url_hint()}
					</p>
				</div>

				<div class="space-y-2">
					<Label for="local-path">{m.shared_local_folder()}</Label>
					<div class="flex gap-2">
						<Input
							id="local-path"
							bind:value={localPath}
							placeholder="/path/to/queries"
							disabled={isLoading}
							class="flex-1"
						/>
						<Button
							variant="outline"
							size="icon"
							onclick={browseFolder}
							disabled={isLoading}
						>
							<FolderIcon class="size-4" />
						</Button>
					</div>
					<p class="text-xs text-muted-foreground">
						{m.shared_clone_folder_hint()}
					</p>
				</div>
			</TabsContent>

			<TabsContent value="init" class="space-y-4 mt-4">
				<div class="space-y-2">
					<Label for="init-name">{m.shared_repo_name()}</Label>
					<Input
						id="init-name"
						bind:value={name}
						placeholder="My Queries"
						disabled={isLoading}
					/>
				</div>

				<div class="space-y-2">
					<Label for="init-path">{m.shared_local_folder()}</Label>
					<div class="flex gap-2">
						<Input
							id="init-path"
							bind:value={localPath}
							placeholder="/path/to/queries"
							disabled={isLoading}
							class="flex-1"
						/>
						<Button
							variant="outline"
							size="icon"
							onclick={browseFolder}
							disabled={isLoading}
						>
							<FolderIcon class="size-4" />
						</Button>
					</div>
					<p class="text-xs text-muted-foreground">
						{m.shared_init_folder_hint()}
					</p>
				</div>
			</TabsContent>
		</Tabs>

		<Dialog.Footer class="mt-6">
			<Button variant="outline" onclick={() => handleOpenChange(false)} disabled={isLoading}>
				{m.common_cancel()}
			</Button>
			<Button onclick={handleSubmit} disabled={isLoading}>
				{#if isLoading}
					<Loader2Icon class="size-4 me-2 animate-spin" />
				{/if}
				{mode === "clone" ? m.shared_clone_repo() : m.shared_create_repo()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
