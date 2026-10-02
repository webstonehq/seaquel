<script lang="ts">
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { unlinkProjectDialogStore as store } from "$lib/stores/unlink-project-dialog.svelte.js";
	import { m } from "$lib/paraglide/messages.js";
	import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
</script>

<Dialog.Root
	open={store.isOpen}
	onOpenChange={(open) => {
		if (!open) store.answer(null);
	}}
>
	<Dialog.Content class="max-w-lg">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<FolderGit2Icon class="size-5" />
				{m.shared_unlink_title({ name: store.projectName })}
			</Dialog.Title>
			<Dialog.Description>
				{m.shared_unlink_description()}
			</Dialog.Description>
		</Dialog.Header>

		<ul class="max-h-64 overflow-y-auto space-y-1 border rounded-lg p-2 text-sm">
			{#each store.imported as connection (connection.id)}
				<li class="px-2 py-1">{connection.name}</li>
			{/each}
		</ul>

		<p class="text-xs text-muted-foreground">{m.shared_unlink_relink_note()}</p>

		<Dialog.Footer>
			<Button variant="ghost" onclick={() => store.answer(null)}>
				{m.header_button_cancel()}
			</Button>
			<Button variant="outline" onclick={() => store.answer("keep")}>
				{m.shared_unlink_keep()}
			</Button>
			<Button variant="destructive" onclick={() => store.answer("remove")}>
				{m.shared_unlink_remove()}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
