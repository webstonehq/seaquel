<script lang="ts">
	import type { Component } from "svelte";
	import { useDatabase } from "$lib/hooks/database.svelte.js";
	import { formatRelativeTime } from "$lib/utils.js";
	import * as Sidebar from "$lib/components/ui/sidebar/index.js";
	import { Button } from "$lib/components/ui/button";
	import Trash2Icon from "@lucide/svelte/icons/trash-2";
	import StarIcon from "@lucide/svelte/icons/star";
	import GitBranchIcon from "@lucide/svelte/icons/git-branch";
	import * as Tooltip from "$lib/components/ui/tooltip/index.js";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		icon: Component<{ class?: string }>;
		name: string;
		starred: boolean;
		shared: boolean;
		updatedAt?: Date;
		onclick: () => void;
		ondelete: () => void;
		ontogglestar: () => void;
		onshare?: () => void;
		onunshare?: () => void;
	}

	let { icon: Icon, name, starred, shared, updatedAt, onclick, ondelete, ontogglestar, onshare, onunshare }: Props = $props();

	const db = useDatabase();
</script>

<Sidebar.MenuItem>
	<Sidebar.MenuButton
		class="h-auto py-2 flex-col items-start gap-1 group/query overflow-hidden"
		{onclick}
	>
		<div class="flex items-center w-full gap-2">
			<div class="flex items-center gap-2 flex-1 min-w-0">
				<Icon class="size-3 text-primary shrink-0" />
				<span class="text-sm font-medium truncate">{name}</span>
			</div>
			<Button
				size="icon"
				variant="ghost"
				class="size-5 shrink-0 [&_svg:not([class*='size-'])]:size-3 opacity-0 group-hover/query:opacity-100 transition-opacity hover:text-destructive"
				aria-label={m.history_delete_saved()}
				onclick={(e) => {
					e.stopPropagation();
					ondelete();
				}}
			>
				<Trash2Icon />
			</Button>
			<Button
				size="icon"
				variant="ghost"
				class={[
					"size-5 shrink-0 [&_svg:not([class*='size-'])]:size-3",
					starred ? "text-yellow-500" : "opacity-0 group-hover/query:opacity-100 transition-opacity"
				]}
				aria-label={starred ? m.sidebar_unstar_query() : m.sidebar_star_query()}
				onclick={(e) => {
					e.stopPropagation();
					ontogglestar();
				}}
			>
				<StarIcon class={starred ? "fill-current" : ""} />
			</Button>
			{#if db.state.activeProjectHasGit}
				<Tooltip.Root>
					<Tooltip.Trigger>
						{#snippet child({ props })}
							<button
								{...props}
								class={[
									"shrink-0 cursor-pointer transition-colors",
									shared ? "text-green-500 hover:text-muted-foreground" : "text-muted-foreground/40 hover:text-green-500"
								]}
								onclick={(e) => {
									e.stopPropagation();
									if (shared) {
										onunshare?.();
									} else {
										onshare?.();
									}
								}}
							>
								<GitBranchIcon class="size-3!" />
							</button>
						{/snippet}
					</Tooltip.Trigger>
					<Tooltip.Content>{shared ? m.connection_mark_local_only() : m.connection_share()}</Tooltip.Content>
				</Tooltip.Root>
			{/if}
		</div>
		{#if updatedAt}
			<p class="text-xs text-muted-foreground w-full text-start">
				{m.sidebar_updated({ time: formatRelativeTime(updatedAt) })}
			</p>
		{/if}
	</Sidebar.MenuButton>
</Sidebar.MenuItem>
