<script lang="ts">
	import { cn } from "$lib/utils.js";
	import * as Kbd from "$lib/components/ui/kbd/index.js";
	import { getKeySymbols, isMac, type ShortcutKeys } from "$lib/shortcuts/index.js";

	let {
		keys,
		class: className = "",
	}: {
		keys: ShortcutKeys;
		class?: string;
	} = $props();

	const symbols = getKeySymbols();

	const keyParts = $derived.by(() => {
		const parts: string[] = [];

		if (keys.mod) parts.push(symbols.mod);
		if (keys.ctrl) parts.push(symbols.ctrl);
		if (keys.alt) parts.push(symbols.alt);
		if (keys.shift) parts.push(symbols.shift);

		// Format the main key
		let mainKey = keys.key;
		if (mainKey === "Enter") mainKey = symbols.enter;
		else if (mainKey === "Backspace") mainKey = symbols.backspace;
		else if (mainKey === "Delete") mainKey = symbols.delete;
		else if (mainKey === "Escape") mainKey = symbols.escape;
		else if (mainKey === "Tab") mainKey = symbols.tab;
		else if (mainKey.length === 1) mainKey = mainKey.toUpperCase();

		parts.push(mainKey);

		return parts;
	});
</script>

{#if isMac()}
	<!-- Mac: show symbols together in a single kbd -->
	<Kbd.Root class={cn(className)}>
		{keyParts.join("")}
	</Kbd.Root>
{:else}
	<!-- Windows/Linux: show with + separators in a group -->
	<Kbd.Group class={cn(className)}>
		{#each keyParts as part, i}
			{#if i > 0}
				<span class="text-muted-foreground">+</span>
			{/if}
			<Kbd.Root>{part}</Kbd.Root>
		{/each}
	</Kbd.Group>
{/if}
