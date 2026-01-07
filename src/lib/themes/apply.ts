import type { Theme, ThemeColors } from "$lib/types/theme";
import { COLOR_VAR_MAP } from "$lib/types/theme";

/**
 * Apply theme colors to the document root element
 * Sets CSS custom properties directly on the root element's inline styles
 */
export function applyThemeColors(colors: ThemeColors): void {
	const root = document.documentElement;

	for (const [key, varName] of Object.entries(COLOR_VAR_MAP)) {
		const value = colors[key as keyof ThemeColors];
		if (value) {
			root.style.setProperty(varName, value);
		}
	}
}

/**
 * Apply a complete theme (wrapper around applyThemeColors)
 */
export function applyTheme(theme: Theme): void {
	applyThemeColors(theme.colors);
}

/**
 * Reset theme colors by removing inline styles
 * Falls back to the stylesheet defaults in layout.css
 */
export function resetThemeColors(): void {
	const root = document.documentElement;

	for (const varName of Object.values(COLOR_VAR_MAP)) {
		root.style.removeProperty(varName);
	}
}

/**
 * Cache theme colors to localStorage for flash prevention
 * This allows app.html to apply colors before the app loads
 */
export function cacheThemeColors(colors: ThemeColors, isDark: boolean): void {
	try {
		localStorage.setItem(
			"seaquel-theme-cache",
			JSON.stringify({
				colors,
				isDark,
				timestamp: Date.now(),
			})
		);
	} catch {
		// Ignore localStorage errors
	}
}

/**
 * Get cached theme colors from localStorage
 */
export function getCachedThemeColors(): { colors: ThemeColors; isDark: boolean } | null {
	try {
		const cached = localStorage.getItem("seaquel-theme-cache");
		if (!cached) return null;

		const parsed = JSON.parse(cached);
		if (!parsed.colors) return null;

		return {
			colors: parsed.colors,
			isDark: parsed.isDark ?? false,
		};
	} catch {
		return null;
	}
}

/**
 * Clear cached theme colors
 */
export function clearThemeCache(): void {
	try {
		localStorage.removeItem("seaquel-theme-cache");
	} catch {
		// Ignore localStorage errors
	}
}

/**
 * Generate CSS text for a theme (useful for preview iframes or export)
 */
export function generateThemeCss(colors: ThemeColors, selector = ":root"): string {
	const lines = [`${selector} {`];

	for (const [key, varName] of Object.entries(COLOR_VAR_MAP)) {
		const value = colors[key as keyof ThemeColors];
		if (value) {
			lines.push(`  ${varName}: ${value};`);
		}
	}

	lines.push("}");
	return lines.join("\n");
}
