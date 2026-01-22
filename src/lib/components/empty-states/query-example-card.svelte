<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { m } from "$lib/paraglide/messages.js";
	import type { SampleQuery } from "$lib/config/sample-queries.js";
	import PlayIcon from "@lucide/svelte/icons/play";
	import CopyIcon from "@lucide/svelte/icons/copy";
	import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";

	interface Props {
		query: SampleQuery;
		onTry: (query: string) => void;
	}

	let { query, onTry }: Props = $props();

	const copyToClipboard = async () => {
		try {
			await navigator.clipboard.writeText(query.query);
			toast.success(m.query_copied_to_clipboard({ format: "SQL" }));
		} catch {
			errorToast(m.query_copy_failed());
		}
	};
</script>

<div class="rounded-lg border bg-card p-4 space-y-3">
	<div class="space-y-1">
		<h3 class="font-medium text-sm">{query.name}</h3>
		<p class="text-xs text-muted-foreground">{query.description}</p>
	</div>

	<div class="rounded-md bg-muted/50 p-3 overflow-x-auto">
		<pre class="text-xs font-mono text-muted-foreground whitespace-pre-wrap">{query.query}</pre>
	</div>

	<div class="flex gap-2">
		<Button size="sm" class="flex-1" onclick={() => onTry(query.query)}>
			<PlayIcon class="size-3 me-2" />
			{m.empty_query_sample_cta()}
		</Button>
		<Button size="sm" variant="outline" onclick={copyToClipboard}>
			<CopyIcon class="size-3" />
		</Button>
	</div>
</div>
