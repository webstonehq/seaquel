<script lang="ts">
	interface Props {
		label: string;
		value: string | number;
		sublabel?: string;
	}

	let { label, value, sublabel }: Props = $props();
</script>

<div class="rounded-lg border bg-card p-4">
	<p class="text-sm text-muted-foreground">{label}</p>
	<p class="text-2xl font-semibold">{value}</p>
	{#if sublabel}
		<p class="text-xs text-muted-foreground">{sublabel}</p>
	{/if}
</div>
