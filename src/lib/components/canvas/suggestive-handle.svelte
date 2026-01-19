<script lang="ts">
	import { Handle, Position } from "@xyflow/svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import * as Popover from "$lib/components/ui/popover";
	import { Button } from "$lib/components/ui/button";
	import type { Component } from "svelte";

	export interface HandleSuggestion {
		label: string;
		icon?: Component;
		action: () => void;
	}

	interface Props {
		nodeId: string;
		type: "source" | "target";
		position: Position;
		id: string;
		isConnectable?: boolean;
		suggestions?: HandleSuggestion[];
		class?: string;
	}

	let {
		nodeId,
		type,
		position,
		id,
		isConnectable = true,
		suggestions = [],
		class: className = "",
	}: Props = $props();

	const db = useDatabase();

	let popoverOpen = $state(false);

	// Check if this handle is connected to any edge
	const isConnected = $derived.by(() => {
		return db.canvasState.edges.some((edge) => {
			if (type === "source") {
				return edge.source === nodeId && edge.sourceHandle === id;
			} else {
				return edge.target === nodeId && edge.targetHandle === id;
			}
		});
	});

	const hasSuggestions = $derived(!isConnected && suggestions.length > 0);

	function handleClick(event: MouseEvent) {
		if (hasSuggestions) {
			event.stopPropagation();
			event.preventDefault();
			popoverOpen = true;
		}
	}

	function executeSuggestion(suggestion: HandleSuggestion) {
		popoverOpen = false;
		suggestion.action();
	}

	// Determine popover side based on handle position
	const popoverSide = $derived.by(() => {
		switch (position) {
			case Position.Right:
				return "right" as const;
			case Position.Left:
				return "left" as const;
			case Position.Top:
				return "top" as const;
			case Position.Bottom:
				return "bottom" as const;
			default:
				return "right" as const;
		}
	});

	// CSS positioning to match xyflow Handle placement
	const triggerPositionClass = $derived.by(() => {
		switch (position) {
			case Position.Right:
				return "absolute right-0 top-1/2 -translate-y-1/2 translate-x-1/2";
			case Position.Left:
				return "absolute left-0 top-1/2 -translate-y-1/2 -translate-x-1/2";
			case Position.Top:
				return "absolute top-0 left-1/2 -translate-x-1/2 -translate-y-1/2";
			case Position.Bottom:
				return "absolute bottom-0 left-1/2 -translate-x-1/2 translate-y-1/2";
			default:
				return "absolute right-0 top-1/2 -translate-y-1/2 translate-x-1/2";
		}
	});
</script>

<Popover.Root bind:open={popoverOpen}>
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<Popover.Trigger
		onclick={handleClick}
		class="{triggerPositionClass} {hasSuggestions ? 'cursor-pointer' : ''}"
	>
		<Handle
			{type}
			{position}
			{isConnectable}
			{id}
			class="{className} {hasSuggestions ? '!ring-2 !ring-primary/50 hover:!ring-primary transition-all' : ''} !relative !transform-none !top-auto !left-auto !right-auto !bottom-auto"
		/>
	</Popover.Trigger>
	<Popover.Content
		class="w-auto p-1"
		side={popoverSide}
		sideOffset={8}
	>
		<div class="flex flex-col gap-0.5">
			{#each suggestions as suggestion}
				<Button
					variant="ghost"
					size="sm"
					class="justify-start h-8 px-2 text-xs"
					onclick={() => executeSuggestion(suggestion)}
				>
					{#if suggestion.icon}
						<suggestion.icon class="size-3.5 mr-2" />
					{/if}
					{suggestion.label}
				</Button>
			{/each}
		</div>
	</Popover.Content>
</Popover.Root>
