import { WebviewWindow, getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { Theme } from "$lib/types/theme";

let themeEditorWindow: WebviewWindow | null = null;

/**
 * Opens the theme editor in a separate window.
 * If the window is already open, focuses it instead.
 * @param theme - The theme to edit, or null to create a new theme
 */
export async function openThemeEditor(theme: Theme | null): Promise<void> {
	try {
		// Check if window already exists
		const existing = await WebviewWindow.getByLabel("theme-editor");
		if (existing) {
			await existing.setFocus();
			return;
		}

		// Build URL with query params
		const url = theme ? `/windows/theme-editor?themeId=${encodeURIComponent(theme.id)}` : "/windows/theme-editor";

		// Get the main window to set as parent
		const mainWindow = getCurrentWebviewWindow();

		// Create new window
		themeEditorWindow = new WebviewWindow("theme-editor", {
			url,
			title: theme ? `Edit Theme: ${theme.name}` : "Create New Theme",
			width: 800,
			height: 600,
			minWidth: 700,
			minHeight: 500,
			center: true,
			resizable: true,
			decorations: true,
			alwaysOnTop: false,
			parent: mainWindow,
		});

		// Handle window creation error
		themeEditorWindow.once("tauri://error", (e) => {
			console.error("Failed to create theme editor window:", e);
			themeEditorWindow = null;
		});

		// Handle window close
		themeEditorWindow.once("tauri://destroyed", () => {
			themeEditorWindow = null;
		});
	} catch (error) {
		console.error("Error opening theme editor:", error);
	}
}

/**
 * Closes the theme editor window if it's open.
 */
export async function closeThemeEditor(): Promise<void> {
	if (themeEditorWindow) {
		await themeEditorWindow.close();
		themeEditorWindow = null;
	}
}

/**
 * Checks if the theme editor window is currently open.
 */
export function isThemeEditorOpen(): boolean {
	return themeEditorWindow !== null;
}
