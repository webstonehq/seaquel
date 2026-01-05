export type ShortcutCategory = "tabs" | "editor" | "navigation" | "general";

export interface ShortcutKeys {
	mod?: boolean; // Cmd on Mac, Ctrl on Windows/Linux
	ctrl?: boolean;
	alt?: boolean;
	shift?: boolean;
	key: string; // The actual key (single character or key name)
}

export interface ShortcutDefinition {
	id: string;
	keys: ShortcutKeys;
	description: string;
	category: ShortcutCategory;
	handledExternally?: boolean; // True for Monaco-handled shortcuts
}

export const shortcuts: ShortcutDefinition[] = [
	// General
	{
		id: "commandPalette",
		keys: { mod: true, key: "k" },
		description: "Open command palette",
		category: "general",
	},
	{
		id: "showShortcuts",
		keys: { key: "?" },
		description: "Show keyboard shortcuts",
		category: "general",
	},
	{
		id: "openSettings",
		keys: { mod: true, key: "," },
		description: "Open settings",
		category: "general",
	},

	// Tabs
	{
		id: "newTab",
		keys: { mod: true, key: "t" },
		description: "New query tab",
		category: "tabs",
	},
	{
		id: "closeTab",
		keys: { mod: true, key: "w" },
		description: "Close current tab",
		category: "tabs",
	},
	{
		id: "nextTab",
		keys: { mod: true, shift: true, key: "]" },
		description: "Next tab",
		category: "tabs",
	},
	{
		id: "previousTab",
		keys: { mod: true, shift: true, key: "[" },
		description: "Previous tab",
		category: "tabs",
	},
	// Tab 1-9 shortcuts
	...Array.from({ length: 9 }, (_, i) => ({
		id: `goToTab${i + 1}`,
		keys: { mod: true, key: String(i + 1) },
		description: `Go to tab ${i + 1}`,
		category: "tabs" as ShortcutCategory,
	})),

	// Editor
	{
		id: "executeQuery",
		keys: { mod: true, key: "Enter" },
		description: "Execute query",
		category: "editor",
		handledExternally: true, // Handled by Monaco
	},
	{
		id: "saveQuery",
		keys: { mod: true, key: "s" },
		description: "Save query",
		category: "editor",
	},
	{
		id: "formatSql",
		keys: { mod: true, shift: true, key: "f" },
		description: "Format SQL",
		category: "editor",
	},

	// Navigation
	{
		id: "toggleSidebar",
		keys: { mod: true, key: "b" },
		description: "Toggle sidebar",
		category: "navigation",
	},
];

export const categoryLabels: Record<ShortcutCategory, string> = {
	general: "General",
	tabs: "Tab Management",
	editor: "Editor",
	navigation: "Navigation",
};

export const categoryOrder: ShortcutCategory[] = [
	"general",
	"tabs",
	"editor",
	"navigation",
];

export function getShortcutsByCategory(): Map<
	ShortcutCategory,
	ShortcutDefinition[]
> {
	const grouped = new Map<ShortcutCategory, ShortcutDefinition[]>();

	for (const shortcut of shortcuts) {
		const existing = grouped.get(shortcut.category) || [];
		existing.push(shortcut);
		grouped.set(shortcut.category, existing);
	}

	return grouped;
}

export function findShortcut(id: string): ShortcutDefinition | undefined {
	return shortcuts.find((s) => s.id === id);
}
