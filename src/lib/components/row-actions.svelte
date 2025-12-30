<script lang="ts">
	import { Button } from "$lib/components/ui/button";
	import * as DropdownMenu from "$lib/components/ui/dropdown-menu";
	import { MoreVerticalIcon, TrashIcon, LoaderIcon } from "@lucide/svelte";
	import { m } from "$lib/paraglide/messages.js";

	interface Props {
		onDelete: () => Promise<void>;
		isDeleting?: boolean;
	}

	let { onDelete, isDeleting = false }: Props = $props();
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="ghost" size="icon" class="h-6 w-6" disabled={isDeleting}>
				{#if isDeleting}
					<LoaderIcon class="size-3 animate-spin" />
				{:else}
					<MoreVerticalIcon class="size-3" />
				{/if}
			</Button>
		{/snippet}
	</DropdownMenu.Trigger>
	<DropdownMenu.Content align="start">
		<DropdownMenu.Item onclick={onDelete} class="text-destructive focus:text-destructive">
			<TrashIcon class="size-4 me-2" />
			{m.row_actions_delete()}
		</DropdownMenu.Item>
	</DropdownMenu.Content>
</DropdownMenu.Root>
