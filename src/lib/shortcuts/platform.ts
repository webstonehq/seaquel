import { isTauri } from "$lib/utils/environment";

let cachedPlatform: string | null = null;

function detectPlatform(): string {
	if (isTauri()) {
		// Use Tauri plugin for accurate detection in desktop app
		// This will be called synchronously, so we need to handle it carefully
		try {
			// Dynamic import won't work synchronously, so use navigator as fallback
			if (typeof navigator !== "undefined") {
				const ua = navigator.userAgent.toLowerCase();
				if (ua.includes("mac")) return "macos";
				if (ua.includes("win")) return "windows";
				if (ua.includes("linux")) return "linux";
			}
		} catch {
			// Fall through to browser detection
		}
	}

	// Browser detection using navigator
	if (typeof navigator !== "undefined") {
		const ua = navigator.userAgent.toLowerCase();
		if (ua.includes("mac")) return "macos";
		if (ua.includes("win")) return "windows";
		if (ua.includes("linux")) return "linux";
	}

	return "unknown";
}

export function isMac(): boolean {
	if (cachedPlatform === null) {
		cachedPlatform = detectPlatform();
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
