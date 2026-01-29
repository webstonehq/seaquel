<script lang="ts">
	import type { Snippet } from "svelte";
	import type { Component } from "svelte";

	interface Props {
		icon: Component<{ class?: string }>;
		title: string;
		description?: string;
		actions?: Snippet;
		class?: string;
	}

	let { icon: Icon, title, description, actions, class: className = "" }: Props = $props();
</script>

<div class="flex-1 flex items-center justify-center p-6 {className}">
	<div class="text-center max-w-xs">
		<Icon class="size-10 mx-auto mb-3 opacity-20" />
		<p class="font-medium mb-2">{title}</p>
		{#if description}
			<p class="text-xs text-muted-foreground mb-4">
				{description}
			</p>
		{/if}
		{#if actions}
			{@render actions()}
		{/if}
	</div>
</div>
