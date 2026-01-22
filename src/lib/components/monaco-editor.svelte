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
		onToggleSidebar = () => {},
		onChange = (_value: string) => {},
		class: className = ""
	}: {
		value?: string;
		ref?: MonacoEditorRef | null;
		schema?: SchemaTable[];
		onExecute?: () => void;
		onToggleSidebar?: () => void;
		onChange?: (value: string) => void;
		class?: string;
	} = $props();

	let container: HTMLDivElement;
	let editor: monaco.editor.IStandaloneCodeEditor | null = null;
	let completionDisposable: monaco.IDisposable | null = null;

	// Track if we're updating programmatically to avoid loops
	let isUpdatingFromProp = false;

	// Derive Monaco theme from mode-watcher (use custom themes with variable styling)
	const editorTheme = $derived(mode.current === "dark" ? "seaquel-dark" : "seaquel-light");

	// Decoration type for template variables
	let variableDecorationType: monaco.editor.IEditorDecorationsCollection | null = null;

	/**
	 * Find all template variables ({{var}}) in the editor and apply decorations
	 */
	function updateVariableDecorations() {
		if (!editor) return;

		const model = editor.getModel();
		if (!model) return;

		const text = model.getValue();
		const decorations: monaco.editor.IModelDeltaDecoration[] = [];

		// Match {{variable_name}} patterns
		const regex = /\{\{([a-zA-Z_][a-zA-Z0-9_]*)\}\}/g;
		let match;

		while ((match = regex.exec(text)) !== null) {
			const startPos = model.getPositionAt(match.index);
			const endPos = model.getPositionAt(match.index + match[0].length);

			decorations.push({
				range: new monaco.Range(
					startPos.lineNumber,
					startPos.column,
					endPos.lineNumber,
					endPos.column
				),
				options: {
					inlineClassName: "template-variable-decoration"
				}
			});
		}

		// Apply decorations (replaces previous ones)
		if (variableDecorationType) {
			variableDecorationType.set(decorations);
		} else {
			variableDecorationType = editor.createDecorationsCollection(decorations);
		}
	}

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
			// Update variable decorations on content change
			updateVariableDecorations();
		});

		// Initial decoration pass
		updateVariableDecorations();

		// Add Cmd/Ctrl+Enter keybinding for query execution
		editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
			onExecute();
		});

		// Add Cmd/Ctrl+B keybinding to toggle sidebar (override Monaco's default bracket jump)
		editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyB, () => {
			onToggleSidebar();
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
			// Update decorations for new content
			updateVariableDecorations();
		}
	});

	// React to theme changes
	$effect(() => {
		// Explicitly access mode.current to ensure reactivity
		const theme = mode.current === "dark" ? "seaquel-dark" : "seaquel-light";
		if (editor) {
			monaco.editor.setTheme(theme);
		}
	});
</script>

<div bind:this={container} class={["h-full w-full", className].filter(Boolean).join(" ")}></div>

<style>
	/* Template variable styling - applied via Monaco decorations */
	:global(.template-variable-decoration) {
		color: #9333ea !important;
		font-weight: 600;
		background-color: rgba(147, 51, 234, 0.1);
		border-radius: 3px;
		padding: 0 2px;
		margin: 0 -2px;
	}

	/* Dark mode styling - using mode-watcher's dark class on :root */
	:global(.dark .template-variable-decoration) {
		color: #c084fc !important;
		background-color: rgba(192, 132, 252, 0.15);
	}
</style>
