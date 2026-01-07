<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import DatabaseTypeCard from "./database-type-card.svelte";
	import { databaseTypes, type DatabaseTypeConfig } from "$lib/stores/connection-wizard.svelte.js";
	import type { DatabaseType } from "$lib/types";

	interface Props {
		selectedType: DatabaseType;
		onSelect: (type: DatabaseType) => void;
	}

	let { selectedType, onSelect }: Props = $props();
</script>

<div class="flex flex-col gap-6 py-4">
	<div class="space-y-2 text-center">
		<h2 class="text-lg font-semibold">{m.wizard_type_title()}</h2>
		<p class="text-sm text-muted-foreground">
			{m.wizard_type_description()}
		</p>
	</div>

	<div class="grid grid-cols-2 gap-3">
		{#each databaseTypes as dbType}
			<DatabaseTypeCard
				config={dbType}
				selected={selectedType === dbType.value}
				onclick={() => onSelect(dbType.value)}
			/>
		{/each}
	</div>
</div>
