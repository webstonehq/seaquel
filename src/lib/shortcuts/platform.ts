import { platform } from "@tauri-apps/plugin-os";

let cachedPlatform: string | null = null;

export function isMac(): boolean {
	if (cachedPlatform === null) {
		cachedPlatform = platform();
	}
	return cachedPlatform === "macos";
}

export const keySymbols = {
	mac: {
		mod: "\u2318",
		ctrl: "\u2303",
		alt: "\u2325",
		shift: "\u21E7",
		enter: "\u21B5",
		backspace: "\u232B",
		delete: "\u2326",
		escape: "\u238B",
		tab: "\u21E5",
		up: "\u2191",
		down: "\u2193",
		left: "\u2190",
		right: "\u2192",
	},
	other: {
		mod: "Ctrl",
		ctrl: "Ctrl",
		alt: "Alt",
		shift: "Shift",
		enter: "Enter",
		backspace: "Backspace",
		delete: "Delete",
		escape: "Esc",
		tab: "Tab",
		up: "\u2191",
		down: "\u2193",
		left: "\u2190",
		right: "\u2192",
	},
} as const;

export type KeySymbols =
	| (typeof keySymbols)["mac"]
	| (typeof keySymbols)["other"];

export function getKeySymbols(): KeySymbols {
	return isMac() ? keySymbols.mac : keySymbols.other;
}
