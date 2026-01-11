<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { connectionDialogStore } from "$lib/stores/connection-dialog.svelte.js";
	import { m } from "$lib/paraglide/messages.js";
	import { getFeatures } from "$lib/features";
	import { LayoutDashboardIcon, PlusIcon } from "@lucide/svelte";

	const db = useDatabase();
	const features = getFeatures();
</script>

<div class="flex-1 flex items-center justify-center p-8">
	<div class="text-center space-y-4">
		<LayoutDashboardIcon class="size-12 mx-auto text-muted-foreground/50" />
		<div class="space-y-1">
			<p class="text-muted-foreground">{m.starter_no_tabs()}</p>
		</div>
		<div class="flex gap-2 justify-center">
			<Button variant="outline" onclick={() => db.starterTabs.reset()}>
				{m.starter_show_getting_started()}
			</Button>
			{#if features.newConnections}
				<Button onclick={() => connectionDialogStore.open()}>
					<PlusIcon class="size-4 me-1" />
					{m.starter_add_connection()}
				</Button>
			{/if}
		</div>
	</div>
</div>
