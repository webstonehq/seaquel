<script lang="ts">
	import { m } from "$lib/paraglide/messages.js";
	import LinkIcon from "@lucide/svelte/icons/link";
	import ListIcon from "@lucide/svelte/icons/list";

	interface Props {
		onChooseString: () => void;
		onChooseManual: () => void;
	}

	let { onChooseString, onChooseManual }: Props = $props();
</script>

<div class="flex flex-col items-center text-center space-y-6 py-4">
	<div class="space-y-2">
		<h2 class="text-lg font-semibold">{m.wizard_string_choice_title()}</h2>
		<p class="text-sm text-muted-foreground max-w-sm">
			{m.wizard_string_choice_description()}
		</p>
	</div>

	<div class="grid grid-cols-2 gap-4 w-full max-w-md">
		<button
			type="button"
			class="flex flex-col items-center gap-3 p-6 rounded-lg border border-border hover:border-primary hover:bg-primary/5 transition-all focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2"
			onclick={onChooseString}
		>
			<div class="size-12 rounded-full flex items-center justify-center">
				<LinkIcon class="size-6 text-primary" />
			</div>
			<div class="space-y-1">
				<h3 class="font-medium">{m.wizard_string_choice_yes()}</h3>
			</div>
		</button>

		<button
			type="button"
			class="flex flex-col items-center gap-3 p-6 rounded-lg border border-border hover:border-primary hover:bg-primary/5 transition-all focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2"
			onclick={onChooseManual}
		>
			<div class="size-12 rounded-full flex items-center justify-center">
				<ListIcon class="size-6 text-muted-foreground" />
			</div>
			<div class="space-y-1">
				<h3 class="font-medium">{m.wizard_string_choice_no()}</h3>
			</div>
		</button>
	</div>
</div>
