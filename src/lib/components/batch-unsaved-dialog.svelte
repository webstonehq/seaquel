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
		unsavedCount: number;
		onDiscardAll: () => void;
		onCancel: () => void;
	};

	let { open = $bindable(false), unsavedCount, onDiscardAll, onCancel }: Props = $props();
</script>

<Dialog bind:open>
	<DialogContent class="max-w-md">
		<DialogHeader>
			<DialogTitle>Unsaved Changes</DialogTitle>
			<DialogDescription>
				{unsavedCount}
				{unsavedCount === 1 ? "query has" : "queries have"} unsaved changes. Discard all changes?
			</DialogDescription>
		</DialogHeader>
		<DialogFooter>
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
					onDiscardAll();
				}}>Discard All</Button
			>
		</DialogFooter>
	</DialogContent>
</Dialog>
