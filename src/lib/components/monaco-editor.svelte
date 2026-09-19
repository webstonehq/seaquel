<script lang="ts" module>
	export interface MonacoEditorRef {
		getCursorOffset: () => number;
		insertText: (text: string) => void;
	}
</script>

<script lang="ts">
	import { onMount, onDestroy } from "svelte";
	import { mode } from "mode-watcher";
	import { initMonaco, createSchemaCompletionProvider, type Monaco } from "$lib/monaco";
	// Type-only import — erased at compile time. The runtime monaco namespace
	// is captured from `initMonaco()` into the `monaco` local below.
	import type * as MonacoNS from "monaco-editor";
	import { editorSettingsStore } from "$lib/stores/editor-settings.svelte.js";
	import type { SchemaTable } from "$lib/types";

	let {
		value = $bindable(""),
		ref = $bindable<MonacoEditorRef | null>(null),
		schema = [] as SchemaTable[],
		onExecute = () => {},
		onToggleSidebar = () => {},
		onChange = (_value: string) => {},
		onAIInlinePrompt,
		class: className = ""
	}: {
		value?: string;
		ref?: MonacoEditorRef | null;
		schema?: SchemaTable[];
		onExecute?: () => void;
		onToggleSidebar?: () => void;
		onChange?: (value: string) => void;
		onAIInlinePrompt?: (pos: { lineNumber: number; column: number }) => void;
		class?: string;
	} = $props();

	// oxlint-disable-next-line eslint(no-unassigned-vars)
	let container: HTMLDivElement;
	// oxlint-disable-next-line eslint(no-unassigned-vars)
	let vimStatusBar: HTMLDivElement;
	let monaco: Monaco | null = null;
	let editor: MonacoNS.editor.IStandaloneCodeEditor | null = null;
	let completionDisposable: MonacoNS.IDisposable | null = null;
	let keybindingDisposable: MonacoNS.IDisposable | null = null;
	let unsubscribeKeybindings: (() => void) | null = null;

	function applyKeybindingMode() {
		if (!editor) return;

		if (keybindingDisposable) {
			keybindingDisposable.dispose();
			keybindingDisposable = null;
		}

		const keybindingMode = editorSettingsStore.keybindingMode;

		if (keybindingMode === "vim") {
			import("monaco-vim").then(({ initVimMode }) => {
				if (editor && editorSettingsStore.keybindingMode === "vim") {
					keybindingDisposable = initVimMode(editor, vimStatusBar);
				}
			});
		} else if (keybindingMode === "emacs") {
			import("monaco-emacs").then(({ EmacsExtension }) => {
				if (editor && editorSettingsStore.keybindingMode === "emacs") {
					const emacs = new EmacsExtension(editor);
					emacs.start();
					keybindingDisposable = emacs;
				}
			});
		}
	}

	// Track if we're updating programmatically to avoid loops
	let isUpdatingFromProp = false;

	// Derive Monaco theme from mode-watcher (use custom themes with variable styling)
	const editorTheme = $derived(mode.current === "dark" ? "seaquel-dark" : "seaquel-light");

	// Decoration type for template variables
	let variableDecorationType: MonacoNS.editor.IEditorDecorationsCollection | null = null;

	/**
	 * Find all template variables ({{var}}) in the editor and apply decorations
	 */
	function updateVariableDecorations() {
		if (!editor || !monaco) return;

		const model = editor.getModel();
		if (!model) return;

		const text = model.getValue();
		const decorations: MonacoNS.editor.IModelDeltaDecoration[] = [];

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
		monaco = await initMonaco();

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
			createSchemaCompletionProvider(monaco, () => schema)
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

		// Work around WKWebView/Tauri bug: the first character keypress with an
		// active selection is silently dropped by the browser's text input pipeline.
		// Intercept it and type programmatically via Monaco's command system instead.
		// (The "/" key is excluded because it's handled by addCommand below.)
		const editorForWorkaround = editor;
		editor.onKeyDown((e) => {
			const sel = editorForWorkaround.getSelection();
			if (
				sel && !sel.isEmpty() &&
				e.browserEvent.key.length === 1 &&
				e.browserEvent.key !== "/" &&
				!e.browserEvent.isComposing &&
				!e.browserEvent.ctrlKey &&
				!e.browserEvent.metaKey &&
				!e.browserEvent.altKey
			) {
				e.preventDefault();
				e.stopPropagation();
				editorForWorkaround.trigger("keyboard", "type", { text: e.browserEvent.key });
			}
		});

		// Add Cmd/Ctrl+Enter keybinding for query execution
		editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, () => {
			onExecute();
		});

		// Add Cmd/Ctrl+B keybinding to toggle sidebar (override Monaco's default bracket jump)
		editor.addCommand(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyB, () => {
			onToggleSidebar();
		});

		// Trigger AI inline prompt when "/" is typed at the start of an empty line
		const editorRef = editor;
		editor.addCommand(monaco.KeyCode.Slash, () => {
			const pos = editorRef.getPosition();
			const model = editorRef.getModel();
			if (!pos || !model) {
				editorRef.trigger("keyboard", "type", { text: "/" });
				return;
			}
			const lineContent = model.getLineContent(pos.lineNumber);
			const beforeCursor = lineContent.substring(0, pos.column - 1);
			if (onAIInlinePrompt && beforeCursor.trim() === "") {
				onAIInlinePrompt({ lineNumber: pos.lineNumber, column: pos.column });
			} else {
				editorRef.trigger("keyboard", "type", { text: "/" });
			}
		});

		// Expose ref for external access
		ref = {
			getCursorOffset: () => {
				if (!editor) return 0;
				const position = editor.getPosition();
				if (!position) return 0;
				return editor.getModel()?.getOffsetAt(position) ?? 0;
			},
			insertText: (text: string) => {
				if (!editor || !monaco) return;
				const position = editor.getPosition();
				if (!position) return;
				editor.executeEdits("ai-inline", [{
					range: new monaco.Range(
						position.lineNumber,
						position.column,
						position.lineNumber,
						position.column,
					),
					text,
				}]);
				// Move cursor to end of inserted text
				const lines = text.split("\n");
				const newLine = position.lineNumber + lines.length - 1;
				const newCol = lines.length === 1
					? position.column + text.length
					: lines[lines.length - 1].length + 1;
				editor.setPosition({ lineNumber: newLine, column: newCol });
				editor.focus();
			}
		};

		// Apply keybinding mode (vim/emacs) after editor is ready
		applyKeybindingMode();
		unsubscribeKeybindings = editorSettingsStore.onChange(applyKeybindingMode);
	});

	onDestroy(() => {
		unsubscribeKeybindings?.();
		keybindingDisposable?.dispose();
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
		if (editor && monaco) {
			monaco.editor.setTheme(theme);
		}
	});

</script>

<div class="flex h-full w-full flex-col">
	<div bind:this={container} dir="ltr" class={["min-h-0 flex-1", className].filter(Boolean).join(" ")}></div>
	<div
		bind:this={vimStatusBar}
		class={[
			"vim-status-bar px-2 py-0.5 text-xs font-mono text-muted-foreground bg-muted/50 border-t",
			editorSettingsStore.keybindingMode !== "vim" && "hidden"
		].filter(Boolean).join(" ")}
	></div>
</div>

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
