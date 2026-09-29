<script lang="ts">
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { deepLinkProjectPickerStore } from "$lib/stores/deep-link-project-picker.svelte.js";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import FolderIcon from "@lucide/svelte/icons/folder";
	import LinkIcon from "@lucide/svelte/icons/link";

	const db = useDatabase();

	let selectedProjectId = $state<string | null>(null);

	// Default to active project when dialog opens
	$effect(() => {
		if (deepLinkProjectPickerStore.open) {
			selectedProjectId = db.state.activeProjectId;
		}
	});

	function handleOpenChange(open: boolean) {
		if (!open) {
			deepLinkProjectPickerStore.resolve(null);
		}
	}

	function handleImport() {
		if (selectedProjectId) {
			deepLinkProjectPickerStore.resolve(selectedProjectId);
		}
	}

	function handleCancel() {
		deepLinkProjectPickerStore.resolve(null);
	}
</script>

<Dialog.Root bind:open={deepLinkProjectPickerStore.open} onOpenChange={handleOpenChange}>
	<Dialog.Content class="max-w-md">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<LinkIcon class="size-5" />
				Import {deepLinkProjectPickerStore.resourceType}
			</Dialog.Title>
			<Dialog.Description>
				Choose which project to add
				<span class="font-medium">{deepLinkProjectPickerStore.resourceName}</span> to.
			</Dialog.Description>
		</Dialog.Header>

		<div class="space-y-1 py-4 max-h-64 overflow-y-auto">
			{#each db.state.projects as project}
				<button
					class="flex w-full items-center gap-3 rounded-md px-3 py-2 text-sm transition-colors
						{selectedProjectId === project.id
						? 'bg-primary/10 text-primary'
						: 'hover:bg-muted/50'}"
					onclick={() => (selectedProjectId = project.id)}
				>
					<FolderIcon class="size-4 shrink-0" />
					<span class="truncate">{project.name}</span>
				</button>
			{/each}
		</div>

		<Dialog.Footer class="gap-2">
			<Button variant="outline" onclick={handleCancel}>Cancel</Button>
			<Button onclick={handleImport} disabled={!selectedProjectId}>Import</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
