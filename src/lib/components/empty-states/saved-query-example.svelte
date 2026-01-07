<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import { savedQueryExample } from "$lib/config/sample-queries.js";
	import BookmarkIcon from "@lucide/svelte/icons/bookmark";
</script>

<div class="flex flex-col items-center text-center p-6 space-y-4">
	<div class="p-4 rounded-full bg-muted">
		<BookmarkIcon class="size-8 text-muted-foreground opacity-60" />
	</div>

	<div class="space-y-2">
		<h3 class="font-medium">{m.empty_saved_title()}</h3>
		<p class="text-sm text-muted-foreground">{m.empty_saved_description()}</p>
	</div>

	<div class="w-full max-w-sm rounded-lg border bg-card p-4 text-left space-y-2">
		<div class="flex items-center gap-2">
			<BookmarkIcon class="size-4 text-primary" />
			<span class="font-medium text-sm">{savedQueryExample.name}</span>
		</div>
		<pre
			class="text-xs font-mono text-muted-foreground bg-muted/50 rounded-md p-2 overflow-x-auto whitespace-pre-wrap">{savedQueryExample.query.slice(0, 120)}...</pre>
	</div>

	<p class="text-xs text-muted-foreground">
		{m.empty_saved_hint()}
	</p>
</div>
