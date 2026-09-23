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

		// Lessons embedded on the marketing site get little room, so start with
		// the sidebar collapsed to its icon rail. The header toggle expands it.
		if (window.self !== window.top) {
			sidebar.setOpen(false);
		}
	});

	onDestroy(() => {
		shortcuts.unregisterHandler('toggleSidebar');
	});
</script>

{@render children()}
