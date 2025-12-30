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
		unsavedCount: number;
		onDiscardAll: () => void;
		onCancel: () => void;
	};

	let { open = $bindable(false), unsavedCount, onDiscardAll, onCancel }: Props = $props();
</script>

<Dialog bind:open>
	<DialogContent class="max-w-md">
		<DialogHeader>
			<DialogTitle>{m.batch_unsaved_title()}</DialogTitle>
			<DialogDescription>
				{unsavedCount === 1
					? m.batch_unsaved_description_singular({ count: unsavedCount })
					: m.batch_unsaved_description_plural({ count: unsavedCount })}
			</DialogDescription>
		</DialogHeader>
		<DialogFooter>
			<Button
				variant="outline"
				onclick={() => {
					open = false;
					onCancel();
				}}>{m.batch_unsaved_cancel()}</Button
			>
			<Button
				variant="destructive"
				onclick={() => {
					open = false;
					onDiscardAll();
				}}>{m.batch_unsaved_discard_all()}</Button
			>
		</DialogFooter>
	</DialogContent>
</Dialog>
