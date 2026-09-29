<script lang="ts">
	import * as AlertDialog from "$lib/components/ui/alert-dialog/index.js";
	import { m } from "$lib/paraglide/messages.js";
	import { connectionSecretsNotice } from "$lib/stores/connection-secrets-notice.svelte.js";

	// Decision 12a: shown once, then the stored list is cleared.
	const dismiss = () => void connectionSecretsNotice.dismiss();
</script>

<AlertDialog.Root
	open={connectionSecretsNotice.open}
	onOpenChange={(open) => {
		if (!open) dismiss();
	}}
>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{m.connection_secrets_notice_title()}</AlertDialog.Title>
			<AlertDialog.Description>{m.connection_secrets_notice_body()}</AlertDialog.Description>
		</AlertDialog.Header>
		<ul class="list-disc ps-5 text-sm max-h-48 overflow-y-auto">
			{#each connectionSecretsNotice.names as name, i (i)}
				<li>{name}</li>
			{/each}
		</ul>
		<AlertDialog.Footer>
			<AlertDialog.Action onclick={dismiss}>{m.connection_secrets_notice_ok()}</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
