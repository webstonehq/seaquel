<script lang="ts">
	import type { StarterTab } from "$lib/types";
	import GettingStartedContent from "./getting-started-content.svelte";
	import MigrationTipsContent from "./migration-tips-content.svelte";

	interface Props {
		tab: StarterTab;
	}

	let { tab }: Props = $props();
</script>

{#if tab.type === 'getting-started'}
	<GettingStartedContent />
{:else if tab.type === 'migration-tips'}
	<MigrationTipsContent />
{/if}
