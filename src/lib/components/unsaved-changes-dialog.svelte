<script lang="ts">
	import {
		Dialog,
		DialogContent,
		DialogDescription,
		DialogFooter,
		DialogHeader,
		DialogTitle
	} from "$lib/components/ui/dialog";
	import { Button } from "$lib/components/ui/button";
	import { m } from "$lib/paraglide/messages.js";

	type Props = {
		open?: boolean;
		onDiscard: () => void;
		onSave: () => void;
		onCancel: () => void;
	};

	let { open = $bindable(false), onDiscard, onSave, onCancel }: Props = $props();
</script>

<Dialog bind:open>
	<DialogContent class="max-w-md">
		<DialogHeader>
			<DialogTitle>{m.unsaved_title()}</DialogTitle>
			<DialogDescription>{m.unsaved_description()}</DialogDescription>
		</DialogHeader>
		<DialogFooter class="gap-2">
			<Button
				variant="outline"
				onclick={() => {
					open = false;
					onCancel();
				}}>{m.unsaved_cancel()}</Button
			>
			<Button
				variant="destructive"
				onclick={() => {
					open = false;
					onDiscard();
				}}>{m.unsaved_discard()}</Button
			>
			<Button
				onclick={() => {
					open = false;
					onSave();
				}}>{m.unsaved_save()}</Button
			>
		</DialogFooter>
	</DialogContent>
</Dialog>
