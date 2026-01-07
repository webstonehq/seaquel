<script lang="ts">
	import type { DatabaseTypeConfig } from "$lib/stores/connection-wizard.svelte.js";
	import DatabaseIcon from "@lucide/svelte/icons/database";
	import CheckIcon from "@lucide/svelte/icons/check";

	interface Props {
		config: DatabaseTypeConfig;
		selected: boolean;
		onclick: () => void;
	}

	let { config, selected, onclick }: Props = $props();

	// Database type colors for visual distinction
	const typeColors: Record<string, string> = {
		postgres: "bg-blue-500/10 text-blue-600 dark:text-blue-400",
		mysql: "bg-orange-500/10 text-orange-600 dark:text-orange-400",
		mariadb: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
		sqlite: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
		mongodb: "bg-green-500/10 text-green-600 dark:text-green-400",
		mssql: "bg-red-500/10 text-red-600 dark:text-red-400",
	};

	const iconColor = $derived(typeColors[config.value] || "bg-muted text-muted-foreground");
</script>

<button
	type="button"
	class="relative flex flex-col items-start gap-3 p-4 rounded-lg border text-left transition-all hover:bg-accent/50 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 {selected
		? 'border-primary bg-primary/5 ring-1 ring-primary'
		: 'border-border'}"
	{onclick}
>
	{#if selected}
		<div class="absolute top-3 right-3">
			<div class="size-5 rounded-full bg-primary flex items-center justify-center">
				<CheckIcon class="size-3 text-primary-foreground" />
			</div>
		</div>
	{/if}

	<div class="size-10 rounded-lg flex items-center justify-center {iconColor}">
		<DatabaseIcon class="size-5" />
	</div>

	<div class="space-y-1">
		<h3 class="font-medium text-sm">{config.label}</h3>
		<p class="text-xs text-muted-foreground line-clamp-2">{config.description}</p>
	</div>
</button>
