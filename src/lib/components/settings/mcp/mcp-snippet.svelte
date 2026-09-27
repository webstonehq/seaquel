<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import { Button } from "$lib/components/ui/button";
	import { toast } from "svelte-sonner";
	import { errorToast } from "$lib/utils/toast";
	import CopyIcon from "@lucide/svelte/icons/copy";

	interface Props {
		title: string;
		hint?: string;
		text: string;
	}

	let { title, hint, text }: Props = $props();

	async function copy() {
		try {
			await navigator.clipboard.writeText(text);
			toast.success(m.settings_mcp_copied());
		} catch (error) {
			errorToast(error instanceof Error ? error.message : String(error));
		}
	}
</script>

<div class="space-y-2">
	<div class="flex items-end justify-between gap-2">
		<div>
			<p class="text-sm font-medium">{title}</p>
			{#if hint}
				<p class="text-xs text-muted-foreground">{hint}</p>
			{/if}
		</div>
		<Button variant="outline" size="sm" onclick={copy}>
			<CopyIcon class="size-3.5" />
			{m.settings_mcp_copy()}
		</Button>
	</div>
	<pre
		class="bg-muted rounded-md px-3 py-2 text-xs font-mono whitespace-pre-wrap break-all select-text">{text}</pre>
</div>
