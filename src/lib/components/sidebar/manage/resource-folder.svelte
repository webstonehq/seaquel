<script lang="ts">
	import type { Snippet, Component } from "svelte";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Badge } from "$lib/components/ui/badge";
	import ChevronRightIcon from "@lucide/svelte/icons/chevron-right";
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "$lib/components/ui/collapsible";

	interface Props {
		expanded: boolean;
		icon: Component<{ class?: string }>;
		label: string;
		count: number;
		emptyMessage?: string;
		children: Snippet;
	}

	let { expanded = $bindable(), icon: Icon, label, count, emptyMessage, children }: Props = $props();
</script>

<Collapsible bind:open={expanded}>
	<Sidebar.MenuItem>
		<CollapsibleTrigger>
			{#snippet child({ props })}
				<Sidebar.MenuButton {...props} class="pr-1">
					<ChevronRightIcon class={["size-4 transition-transform", expanded && "rotate-90"]} />
					<Icon class="size-4" />
					<span class="flex-1">{label}</span>
					<Badge variant="secondary" class="text-xs">{count}</Badge>
				</Sidebar.MenuButton>
			{/snippet}
		</CollapsibleTrigger>
		<CollapsibleContent class="flex">
			<Sidebar.Menu class="ms-4 border-s border-sidebar-border ps-2">
				{@render children()}
				{#if count === 0 && emptyMessage}
					<div class="text-center py-4 text-muted-foreground px-2">
						<p class="text-xs">{emptyMessage}</p>
					</div>
				{/if}
			</Sidebar.Menu>
		</CollapsibleContent>
	</Sidebar.MenuItem>
</Collapsible>
