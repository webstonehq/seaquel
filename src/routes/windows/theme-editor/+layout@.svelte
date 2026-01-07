<script lang="ts">
	import "../../layout.css";
	import { ModeWatcher } from "mode-watcher";
	import { themeStore } from "$lib/stores/theme.svelte.js";
	import { onMount } from "svelte";

	let { children } = $props();

	// Initialize theme store for this window
	onMount(async () => {
		await themeStore.initialize();
	});

	// Apply theme colors to this window too
	$effect(() => {
		if (themeStore.isLoaded) {
			themeStore.applyActiveTheme();
	}
	});
</script>

<ModeWatcher />

<div class="min-h-screen bg-background text-foreground">
	{@render children()}
</div>
