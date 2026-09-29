<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import PlayIcon from '@lucide/svelte/icons/play';
	import Loader2Icon from '@lucide/svelte/icons/loader-2';
	import MonacoEditor from '$lib/components/monaco-editor.svelte';

	interface Props {
		query: string;
		isExecuting: boolean;
		onQueryChange: (query: string) => void;
		onRun: () => void;
	}

	let { query, isExecuting, onQueryChange, onRun }: Props = $props();

	let editorValue = $derived(query);
</script>

<div class="flex flex-col gap-2">
	<div class="flex items-center justify-between">
		<span class="text-xs font-medium">SQL Query</span>
		<Button variant="outline" size="sm" class="h-6 px-2 text-xs" onclick={onRun} disabled={isExecuting || !query.trim()}>
			{#if isExecuting}
				<Loader2Icon class="mr-1 size-3 animate-spin" />
			{:else}
				<PlayIcon class="mr-1 size-3" />
			{/if}
			Run
		</Button>
	</div>
	<div class="h-32 rounded-md border overflow-hidden">
		<MonacoEditor
			bind:value={editorValue}
			onChange={(v) => onQueryChange(v)}
			onExecute={onRun}
		/>
	</div>
</div>
