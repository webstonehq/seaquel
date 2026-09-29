<script lang="ts">
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { sharedProjectImportStore } from "$lib/stores/shared-project-import.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { toast } from "svelte-sonner";
	import { showErrorUnlessShown } from "$lib/errors";
	import { m } from "$lib/paraglide/messages.js";
	import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
	import LoaderIcon from "@lucide/svelte/icons/loader";

	const db = useDatabase();

	const selectedCount = $derived(
		sharedProjectImportStore.discoveredProjects.filter((p) => p.selected).length
	);

	async function handleImport() {
		const selected = sharedProjectImportStore.discoveredProjects.filter((p) => p.selected);
		if (selected.length === 0 || !sharedProjectImportStore.folderPath) return;

		sharedProjectImportStore.isImporting = true;
		try {
			await db.projects.importFromGitRepo(sharedProjectImportStore.folderPath, selected);
			toast.success(m.shared_import_success({ count: selected.length }));
			sharedProjectImportStore.reset();
		} catch (error) {
			showErrorUnlessShown(error);
			sharedProjectImportStore.isImporting = false;
		}
	}
</script>

<Dialog.Root
	bind:open={sharedProjectImportStore.isOpen}
	onOpenChange={(open) => {
		if (!open) sharedProjectImportStore.reset();
	}}
>
	<Dialog.Content class="max-w-lg">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<FolderGit2Icon class="size-5" />
				{m.shared_import_title()}
			</Dialog.Title>
			<Dialog.Description>
				{m.shared_import_description()}
			</Dialog.Description>
		</Dialog.Header>

		{#if sharedProjectImportStore.discoveredProjects.length > 0}
			<div class="space-y-4 py-4">
				<!-- Selection controls -->
				<div class="flex items-center justify-between text-sm">
					<span class="text-muted-foreground">
						{m.shared_import_found({ count: sharedProjectImportStore.discoveredProjects.length })}
					</span>
					<div class="flex gap-2">
						<Button
							variant="ghost"
							size="sm"
							onclick={() => sharedProjectImportStore.selectAll()}
						>
							{m.dbeaver_import_select_all()}
						</Button>
						<Button
							variant="ghost"
							size="sm"
							onclick={() => sharedProjectImportStore.deselectAll()}
						>
							{m.dbeaver_import_deselect_all()}
						</Button>
					</div>
				</div>

				<!-- Project list -->
				<div class="max-h-64 overflow-y-auto space-y-2 border rounded-lg p-2">
					{#each sharedProjectImportStore.discoveredProjects as project, index (index)}
						<label
							class="flex items-start gap-3 p-2 rounded-md hover:bg-muted/50 cursor-pointer transition-colors"
						>
							<Checkbox
								checked={project.selected}
								onCheckedChange={() => sharedProjectImportStore.toggleProject(index)}
							/>
							<div class="flex-1 min-w-0">
								<span class="font-medium">{project.name}</span>
								{#if project.connections.length > 0}
									<p class="text-xs text-muted-foreground">
										{project.connections.length} connection{project.connections.length !== 1 ? "s" : ""}
									</p>
								{/if}
							</div>
						</label>
					{/each}
				</div>

				<!-- Folder path -->
				<code class="text-xs bg-muted px-2 py-1.5 rounded truncate block">
					{sharedProjectImportStore.folderPath}
				</code>
			</div>
		{/if}

		<Dialog.Footer>
			<Button variant="ghost" onclick={() => sharedProjectImportStore.reset()}>
				{m.header_button_cancel()}
			</Button>
			<Button
				onclick={handleImport}
				disabled={selectedCount === 0 || sharedProjectImportStore.isImporting}
			>
				{#if sharedProjectImportStore.isImporting}
					<LoaderIcon class="size-4 animate-spin" />
				{/if}
				{m.shared_import_button({ count: selectedCount })}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
