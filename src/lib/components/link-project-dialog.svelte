<script lang="ts">
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { Checkbox } from "$lib/components/ui/checkbox";
	import { linkProjectDialogStore as store } from "$lib/stores/link-project-dialog.svelte.js";
	import { m } from "$lib/paraglide/messages.js";
	import FolderGit2Icon from "@lucide/svelte/icons/folder-git-2";
</script>

<Dialog.Root
	open={store.isOpen}
	onOpenChange={(open) => {
		if (!open) store.cancel();
	}}
>
	<Dialog.Content class="max-w-lg">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<FolderGit2Icon class="size-5" />
				{m.shared_link_title({ name: store.projectName })}
			</Dialog.Title>
			<Dialog.Description>
				{m.shared_link_description()}
			</Dialog.Description>
		</Dialog.Header>

		{#if store.connections.length > 0}
			<div class="max-h-64 overflow-y-auto space-y-2 border rounded-lg p-2">
				{#each store.connections as connection (connection.id)}
					<label
						class="flex items-start gap-3 p-2 rounded-md hover:bg-muted/50 cursor-pointer transition-colors"
					>
						<Checkbox
							checked={store.ticked.includes(connection.id)}
							onCheckedChange={() => store.toggle(connection.id)}
						/>
						<div class="flex-1 min-w-0">
							<span class="font-medium">{connection.name}</span>
							{#if connection.isLocalOnly}
								<p class="text-xs text-muted-foreground">{m.shared_link_local_only()}</p>
							{/if}
						</div>
					</label>
				{/each}
			</div>
			<p class="text-xs text-muted-foreground">{m.shared_link_unticked_note()}</p>
		{:else}
			<p class="text-sm text-muted-foreground">{m.shared_link_no_connections()}</p>
		{/if}

		<Dialog.Footer>
			<Button variant="ghost" onclick={() => store.cancel()}>
				{m.header_button_cancel()}
			</Button>
			<Button onclick={() => store.confirm()}>
				{m.shared_link_button({ count: store.ticked.length })}
			</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
