<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { useSidebar } from '$lib/components/ui/sidebar';
	import { useShortcuts } from '$lib/shortcuts';

	let { children } = $props();

	const shortcuts = useShortcuts();
	const sidebar = useSidebar();

	onMount(() => {
		shortcuts.registerHandler('toggleSidebar', () => {
			sidebar.toggle();
		});
	});

	onDestroy(() => {
		shortcuts.unregisterHandler('toggleSidebar');
	});
</script>

{@render children()}
