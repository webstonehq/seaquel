<script lang="ts">
	import { onMount, onDestroy } from 'svelte';
	import { mode } from 'mode-watcher';
	import { useQueryBuilder } from '$lib/hooks/query-builder.svelte';
	import { initMonaco, monaco } from '$lib/monaco';
	import { Button } from '$lib/components/ui/button';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import AlertTriangleIcon from '@lucide/svelte/icons/alert-triangle';
	import { toast } from 'svelte-sonner';

	const qb = useQueryBuilder();

	let editorContainer: HTMLDivElement;
	let editor: monaco.editor.IStandaloneCodeEditor | null = null;
	let isUpdatingFromExternal = false;

	// Derive Monaco theme from mode-watcher
	const editorTheme = $derived(mode.current === 'dark' ? 'seaquel-dark' : 'seaquel-light');

	onMount(async () => {
		await initMonaco();

		editor = monaco.editor.create(editorContainer, {
			value: qb.generatedSql,
			language: 'pgsql',
			theme: editorTheme,
			minimap: { enabled: false },
			fontSize: 13,
			fontFamily: 'ui-monospace, monospace',
			lineNumbers: 'on',
			scrollBeyondLastLine: false,
			automaticLayout: true,
			tabSize: 2,
			wordWrap: 'on',
			padding: { top: 12, bottom: 12 },
			scrollbar: {
				verticalScrollbarSize: 10,
				horizontalScrollbarSize: 10
			},
			readOnly: false
		});

		// Listen for user edits
		editor.onDidChangeModelContent(() => {
			if (isUpdatingFromExternal) return;
			const sql = editor?.getValue() ?? '';
			qb.setSqlOverride(sql);
		});
	});

	onDestroy(() => {
		editor?.dispose();
	});

	// Update editor when generated SQL changes (from visual edits)
	$effect(() => {
		if (editor && qb.isVisualMode) {
			const currentValue = editor.getValue();
			if (currentValue !== qb.generatedSql) {
				isUpdatingFromExternal = true;
				editor.setValue(qb.generatedSql);
				isUpdatingFromExternal = false;
			}
		}
	});

	// React to theme changes
	$effect(() => {
		const theme = mode.current === 'dark' ? 'seaquel-dark' : 'seaquel-light';
		if (editor) {
			monaco.editor.setTheme(theme);
		}
	});

	function handleCopy() {
		navigator.clipboard.writeText(qb.generatedSql);
		toast.success('SQL copied to clipboard');
	}

	function handleReset() {
		qb.setVisualMode(true);
		if (editor) {
			isUpdatingFromExternal = true;
			editor.setValue(qb.generatedSql);
			isUpdatingFromExternal = false;
		}
	}
</script>

<div class="flex flex-col h-full border-l">
	<!-- Header -->
	<div class="flex items-center justify-between px-3 py-2 border-b bg-muted/50">
		<span class="font-medium text-sm">SQL</span>
		<div class="flex items-center gap-1">
			{#if !qb.isVisualMode}
				<Button
					variant="ghost"
					size="sm"
					class="h-7 px-2 gap-1.5 text-xs"
					onclick={handleReset}
					title="Reset to visual mode"
				>
					<RotateCcwIcon class="size-3.5" />
					Reset
				</Button>
			{/if}
			<Button
				variant="ghost"
				size="sm"
				class="h-7 px-2 gap-1.5 text-xs"
				onclick={handleCopy}
				title="Copy SQL to clipboard"
			>
				<CopyIcon class="size-3.5" />
				Copy
			</Button>
		</div>
	</div>

	<!-- Warning banner when in override mode -->
	{#if !qb.isVisualMode}
		<div class="flex items-center gap-2 px-3 py-2 bg-yellow-500/10 border-b border-yellow-500/20">
			<AlertTriangleIcon class="size-4 text-yellow-600 dark:text-yellow-500 shrink-0" />
			<p class="text-xs text-yellow-700 dark:text-yellow-400">
				SQL manually edited. Visual changes won't sync until you reset.
			</p>
		</div>
	{/if}

	<!-- Editor container -->
	<div bind:this={editorContainer} class="flex-1 min-h-0"></div>
</div>
