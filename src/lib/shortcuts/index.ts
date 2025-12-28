export { isMac, getKeySymbols, keySymbols, type KeySymbols } from "./platform.js";
export {
	shortcuts,
	categoryLabels,
	categoryOrder,
	getShortcutsByCategory,
	findShortcut,
	type ShortcutCategory,
	type ShortcutKeys,
	type ShortcutDefinition,
} from "./registry.js";
export { setShortcuts, useShortcuts } from "./shortcuts.svelte.js";
