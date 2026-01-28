<script lang="ts">
	import type { DatabaseTypeConfig } from "$lib/stores/connection-wizard.svelte.js";
	import DatabaseIcon from "@lucide/svelte/icons/database";

	interface Props {
		config: DatabaseTypeConfig;
		onclick: () => void;
	}

	let { config, onclick }: Props = $props();

	// Database type colors for visual distinction
	const typeColors: Record<string, string> = {
		postgres: "bg-blue-500/10 text-blue-600 dark:text-blue-400",
		mysql: "bg-orange-500/10 text-orange-600 dark:text-orange-400",
		mariadb: "bg-amber-500/10 text-amber-600 dark:text-amber-400",
		sqlite: "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
		duckdb: "bg-yellow-500/10 text-yellow-600 dark:text-yellow-400",
		mssql: "bg-red-500/10 text-red-600 dark:text-red-400",
	};

	const iconColor = $derived(typeColors[config.value] || "bg-muted text-muted-foreground");
</script>

<button
	type="button"
	class="flex flex-col items-start gap-3 p-4 rounded-lg text-left transition-all hover:bg-accent/50 focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2"
	{onclick}
>
	<div class="size-10 rounded-lg flex items-center justify-center {iconColor}">
		<DatabaseIcon class="size-5" />
	</div>

	<div class="space-y-1">
		<h3 class="font-medium text-sm">{config.label}</h3>
		<p class="text-xs text-muted-foreground line-clamp-2">{config.description}</p>
	</div>
</button>
