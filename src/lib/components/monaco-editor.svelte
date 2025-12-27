<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import { mode } from "mode-watcher";
	import { initMonaco, monaco, createSchemaCompletionProvider } from "$lib/monaco";
	import type { SchemaTable } from "$lib/types";

	let {
		value = $bindable(""),
		schema = [] as SchemaTable[],
		onExecute = () => {},
		class: className = ""
	}: {
		value?: string;
		schema?: SchemaTable[];
		onExecute?: () => void;
		class?: string;
	} = $props();

	let container: HTMLDivElement;
	let editor: monaco.editor.IStandaloneCodeEditor | null = null;
	let completionDisposable: monaco.IDisposable | null = null;

	// Track if we're updating programmatically to avoid loops
	let isUpdatingFromProp = false;

	// Derive Monaco theme from mode-watcher
	const editorTheme = $derived(mode.current === "dark" ? "vs-dark" : "vs");

	onMount(async () => {
		await initMonaco();

		editor = monaco.editor.create(container, {
			value: value,
			language: "pgsql",
			theme: editorTheme,
			automaticLayout: true,
			minimap: { enabled: false },
			fontSize: 13,
			fontFamily: "ui-monospace, monospace",
			lineNumbers: "on",
			scrollBeyondLastLine: false,
			wordWrap: "on",
			tabSize: 2,
			padding: { top: 8, bottom: 8 },
			scrollbar: {
				verticalScrollbarSize: 10,
				horizontalScrollbarSize: 10
			}
		});

		// Register schema-aware completion provider
		completionDisposable = monaco.languages.registerCompletionItemProvider(
			"pgsql",
			createSchemaCompletionProvider(() => schema)
		);

		// Sync editor content to bound value
		editor.onDidChangeModelContent(() => {
			if (!isUpdatingFromProp && editor) {
				value = editor.getValue();
			}
		});

		// Add Cmd/Ctrl+Enter keybinding for query execution
		editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
			onExecute();
		});
	});

	onDestroy(() => {
		completionDisposable?.dispose();
		editor?.dispose();
	});

	// React to value prop changes (e.g., when switching tabs or formatting)
	$effect(() => {
		// Explicitly track value to ensure reactivity
		const currentValue = value;
		if (editor && editor.getValue() !== currentValue) {
			isUpdatingFromProp = true;
			editor.setValue(currentValue);
			isUpdatingFromProp = false;
		}
	});

	// React to theme changes
	$effect(() => {
		if (editor) {
			monaco.editor.setTheme(editorTheme);
		}
	});
</script>

<div bind:this={container} class={["h-full w-full", className].filter(Boolean).join(" ")}></div>
