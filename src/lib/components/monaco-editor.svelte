<script lang="ts" module>
	export interface MonacoEditorRef {
		getCursorOffset: () => number;
	}
</script>

<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import { mode } from "mode-watcher";
	import { initMonaco, monaco, createSchemaCompletionProvider } from "$lib/monaco";
	import type { SchemaTable } from "$lib/types";

	let {
		value = $bindable(""),
		ref = $bindable<MonacoEditorRef | null>(null),
		schema = [] as SchemaTable[],
		onExecute = () => {},
		onChange = (_value: string) => {},
		class: className = ""
	}: {
		value?: string;
		ref?: MonacoEditorRef | null;
		schema?: SchemaTable[];
		onExecute?: () => void;
		onChange?: (value: string) => void;
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

		// Sync editor content to bound value and notify parent
		editor.onDidChangeModelContent(() => {
			if (!isUpdatingFromProp && editor) {
				const newValue = editor.getValue();
				value = newValue;
				onChange(newValue);
			}
		});

		// Add Cmd/Ctrl+Enter keybinding for query execution
		editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
			onExecute();
		});

		// Expose ref for external access
		ref = {
			getCursorOffset: () => {
				if (!editor) return 0;
				const position = editor.getPosition();
				if (!position) return 0;
				return editor.getModel()?.getOffsetAt(position) ?? 0;
			}
		};
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
