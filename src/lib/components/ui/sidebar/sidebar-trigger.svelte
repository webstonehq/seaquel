<script lang="ts">
	import { Button } from "$lib/components/ui/button/index.js";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { cn } from "$lib/utils.js";
	import PanelLeftIcon from "@lucide/svelte/icons/panel-left";
	import type { ComponentProps } from "svelte";
	import { useSidebar } from "./context.svelte.js";
	import { findShortcut } from "$lib/shortcuts/index.js";
	import ShortcutKeys from "$lib/components/shortcut-keys.svelte";

	let {
		ref = $bindable(null),
		class: className,
		onclick,
		...restProps
	}: ComponentProps<typeof Button> & {
		onclick?: (e: MouseEvent) => void;
	} = $props();

	const sidebar = useSidebar();
</script>

<Tooltip.Root>
	<Tooltip.Trigger>
		<Button
			data-sidebar="trigger"
			data-slot="sidebar-trigger"
			variant="ghost"
			size="icon"
			class={cn("size-7", className)}
			type="button"
			onclick={(e) => {
				onclick?.(e);
				sidebar.toggle();
			}}
			{...restProps}
		>
			<PanelLeftIcon />
			<span class="sr-only">Toggle Sidebar</span>
		</Button>
	</Tooltip.Trigger>
	<Tooltip.Content side="right">
		<span class="flex items-center gap-2">
			Toggle Sidebar
			{#if findShortcut('toggleSidebar')}
				<ShortcutKeys keys={findShortcut('toggleSidebar')!.keys} />
			{/if}
		</span>
	</Tooltip.Content>
</Tooltip.Root>
