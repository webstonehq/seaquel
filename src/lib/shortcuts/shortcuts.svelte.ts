import { setContext, getContext } from "svelte";
import { shortcuts, type ShortcutDefinition, type ShortcutKeys } from "./registry.js";
import { isMac } from "./platform.js";

type ShortcutHandler = () => void;

class ShortcutManager {
	private handlers = new Map<string, ShortcutHandler>();
	showHelp = $state(false);

	registerHandler(id: string, handler: ShortcutHandler) {
		this.handlers.set(id, handler);
	}

	unregisterHandler(id: string) {
		this.handlers.delete(id);
	}

	handleKeydown = (e: KeyboardEvent) => {
		// Skip if typing in an input/textarea (unless it's a global shortcut)
		const target = e.target as HTMLElement;
		const isInInput =
			target instanceof HTMLInputElement ||
			target instanceof HTMLTextAreaElement ||
			target.isContentEditable;

		// Check for '?' to show help (but not in inputs)
		if (
			!isInInput &&
			e.key === "?" &&
			!e.metaKey &&
			!e.ctrlKey &&
			!e.altKey
		) {
			e.preventDefault();
			this.showHelp = true;
			return;
		}

		// Find matching shortcut
		for (const shortcut of shortcuts) {
			if (shortcut.handledExternally) continue;

			if (this.matchesShortcut(e, shortcut.keys)) {
				// Check if we should skip input fields for this shortcut
				if (isInInput && !this.isGlobalShortcut(shortcut)) continue;

				const handler = this.handlers.get(shortcut.id);
				if (handler) {
					e.preventDefault();
					handler();
					return;
				}
			}
		}
	};

	private matchesShortcut(e: KeyboardEvent, keys: ShortcutKeys): boolean {
		const mac = isMac();
		const modPressed = mac ? e.metaKey : e.ctrlKey;

		// Check mod key
		if (keys.mod && !modPressed) return false;
		if (!keys.mod && modPressed) return false;

		// Check other modifier keys
		if (keys.ctrl && !e.ctrlKey) return false;
		if (keys.alt && !e.altKey) return false;
		if (keys.shift !== undefined && keys.shift !== e.shiftKey) return false;

		// Handle special key matching (brackets with shift produce different characters)
		const keyLower = e.key.toLowerCase();
		const expectedKey = keys.key.toLowerCase();

		if (keyLower === expectedKey) return true;

		// Handle bracket shortcuts with shift (] and } are the same key)
		if (keys.key === "]" && (e.key === "]" || e.key === "}")) return true;
		if (keys.key === "[" && (e.key === "[" || e.key === "{")) return true;

		return false;
	}

	private isGlobalShortcut(shortcut: ShortcutDefinition): boolean {
		// Shortcuts that should work even in input fields
		const globalIds = ["toggleSidebar", "showShortcuts", "commandPalette"];
		return globalIds.includes(shortcut.id);
	}

	toggleHelp() {
		this.showHelp = !this.showHelp;
	}

	closeHelp() {
		this.showHelp = false;
	}
}

const SHORTCUTS_KEY = Symbol("shortcuts");

export function setShortcuts(): ShortcutManager {
	return setContext(SHORTCUTS_KEY, new ShortcutManager());
}

export function useShortcuts(): ShortcutManager {
	return getContext<ShortcutManager>(SHORTCUTS_KEY);
}
