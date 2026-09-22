<script lang="ts">
	import * as Dialog from "$lib/components/ui/dialog/index.js";
	import { Button } from "$lib/components/ui/button";
	import { sshHostKeyPromptStore } from "$lib/stores/ssh-host-key-prompt.svelte.js";
	import { ShieldQuestionIcon } from "@lucide/svelte";

	function handleOpenChange(open: boolean) {
		if (!open) sshHostKeyPromptStore.resolve(false);
	}
</script>

<Dialog.Root bind:open={sshHostKeyPromptStore.open} onOpenChange={handleOpenChange}>
	<Dialog.Content class="sm:max-w-lg">
		<Dialog.Header>
			<Dialog.Title class="flex items-center gap-2">
				<ShieldQuestionIcon class="size-4" aria-hidden="true" />
				Unknown SSH host key
			</Dialog.Title>
			<Dialog.Description>
				The authenticity of
				<span class="font-mono">{sshHostKeyPromptStore.host}:{sshHostKeyPromptStore.port}</span>
				can't be established. Connect only if this fingerprint matches the server's.
			</Dialog.Description>
		</Dialog.Header>

		<div class="rounded border bg-muted/50 p-3">
			<p class="text-xs text-muted-foreground">Key fingerprint</p>
			<p class="font-mono text-sm break-all select-text">{sshHostKeyPromptStore.fingerprint}</p>
		</div>

		<p class="text-xs text-muted-foreground">
			Connecting adds this key to <span class="font-mono">~/.ssh/known_hosts</span>. You won't be
			asked again unless it changes.
		</p>

		<Dialog.Footer>
			<Button variant="outline" onclick={() => sshHostKeyPromptStore.resolve(false)}>Cancel</Button>
			<Button onclick={() => sshHostKeyPromptStore.resolve(true)}>Connect and trust</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
