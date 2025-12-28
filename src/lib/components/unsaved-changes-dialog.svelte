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
			<DialogTitle>Unsaved Changes</DialogTitle>
			<DialogDescription>This query has unsaved changes. What would you like to do?</DialogDescription>
		</DialogHeader>
		<DialogFooter class="gap-2">
			<Button
				variant="outline"
				onclick={() => {
					open = false;
					onCancel();
				}}>Cancel</Button
			>
			<Button
				variant="destructive"
				onclick={() => {
					open = false;
					onDiscard();
				}}>Discard</Button
			>
			<Button
				onclick={() => {
					open = false;
					onSave();
				}}>Save</Button
			>
		</DialogFooter>
	</DialogContent>
</Dialog>
