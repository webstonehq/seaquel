<script lang="ts">
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogHeader,
		DialogTitle,
	} from "$lib/components/ui/dialog";
	import { useShortcuts } from "$lib/shortcuts/shortcuts.svelte.js";
	import {
		getShortcutsByCategory,
		categoryLabels,
		categoryOrder,
	} from "$lib/shortcuts/registry.js";
	import ShortcutKeys from "$lib/components/shortcut-keys.svelte";

	const shortcutManager = useShortcuts();

	const shortcutsByCategory = getShortcutsByCategory();
</script>

<Dialog bind:open={shortcutManager.showHelp}>
	<DialogContent class="max-w-lg max-h-[80vh] overflow-y-auto">
		<DialogHeader>
			<DialogTitle>Keyboard Shortcuts</DialogTitle>
			<DialogDescription>
				Quick reference for available keyboard shortcuts
			</DialogDescription>
		</DialogHeader>

		<div class="space-y-6 py-4">
			{#each categoryOrder as category}
				{@const categoryShortcuts = shortcutsByCategory.get(category)}
				{#if categoryShortcuts && categoryShortcuts.length > 0}
					<div>
						<h3 class="text-sm font-medium mb-3 text-muted-foreground">
							{categoryLabels[category]}
						</h3>
						<div class="space-y-2">
							{#each categoryShortcuts as shortcut}
								{#if shortcut.id.startsWith("goToTab")}
									{#if shortcut.id === "goToTab1"}
										<!-- Show only one entry for tab 1-9 shortcuts -->
										<div
											class="flex items-center justify-between py-1.5 px-2 rounded-sm hover:bg-muted/50"
										>
											<span class="text-sm">Go to tab 1-9</span>
											<ShortcutKeys keys={{ mod: true, key: "1-9" }} />
										</div>
									{/if}
								{:else}
									<div
										class="flex items-center justify-between py-1.5 px-2 rounded-sm hover:bg-muted/50"
									>
										<span class="text-sm">{shortcut.description}</span>
										<ShortcutKeys keys={shortcut.keys} />
									</div>
								{/if}
							{/each}
						</div>
					</div>
				{/if}
			{/each}
		</div>
	</DialogContent>
</Dialog>
