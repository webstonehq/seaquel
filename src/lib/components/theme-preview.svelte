<script lang="ts">
	import type { ThemeColors } from "$lib/types/theme";
	import { oklchToHex } from "$lib/themes/color-utils";

	interface Props {
		colors: ThemeColors;
		size?: "sm" | "md";
	}

	let { colors, size = "md" }: Props = $props();

	// Convert oklch colors to hex for display in the swatches
	const swatchColors = $derived([
		{ color: colors.background, name: "bg" },
		{ color: colors.primary, name: "primary" },
		{ color: colors.secondary, name: "secondary" },
		{ color: colors.accent, name: "accent" },
		{ color: colors.destructive, name: "destructive" },
		{ color: colors.muted, name: "muted" },
	]);

	function getHex(oklch: string): string {
		return oklchToHex(oklch);
	}

	const sizeClasses = $derived(
		size === "sm"
			? "h-4 gap-0.5"
			: "h-6 gap-1"
	);

	const swatchClasses = $derived(
		size === "sm"
			? "w-4 rounded-sm"
			: "w-6 rounded"
	);
</script>

<div class="flex {sizeClasses} overflow-hidden rounded-md border">
	{#each swatchColors as swatch}
		<div
			class="{swatchClasses} flex-shrink-0"
			style="background-color: {getHex(swatch.color)}"
			title={swatch.name}
		></div>
	{/each}
</div>
