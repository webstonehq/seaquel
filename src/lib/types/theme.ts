/**
 * Theme color variables - maps to CSS custom properties in layout.css
 * All values should be in oklch format (e.g., "oklch(0.145 0 0)")
 */
export interface ThemeColors {
	// Core backgrounds
	background: string;
	foreground: string;
	card: string;
	cardForeground: string;
	popover: string;
	popoverForeground: string;

	// Interactive elements
	primary: string;
	primaryForeground: string;
	secondary: string;
	secondaryForeground: string;

	// Status/accent colors
	muted: string;
	mutedForeground: string;
	accent: string;
	accentForeground: string;
	destructive: string;

	// Borders and inputs
	border: string;
	input: string;
	ring: string;

	// Chart colors
	chart1: string;
	chart2: string;
	chart3: string;
	chart4: string;
	chart5: string;

	// Sidebar specific
	sidebar: string;
	sidebarForeground: string;
	sidebarPrimary: string;
	sidebarPrimaryForeground: string;
	sidebarAccent: string;
	sidebarAccentForeground: string;
	sidebarBorder: string;
	sidebarRing: string;
}

/**
 * Theme definition
 */
export interface Theme {
	/** Unique identifier (uuid for user themes, slug for built-in) */
	id: string;
	/** Display name */
	name: string;
	/** Optional description */
	description?: string;
	/** Optional author name */
	author?: string;
	/** Whether this is a built-in theme (prevents editing/deletion) */
	isBuiltIn: boolean;
	/** Whether this is a dark or light theme */
	isDark: boolean;
	/** Theme color values */
	colors: ThemeColors;
	/** Creation timestamp (user themes only) */
	createdAt?: string;
	/** Last update timestamp (user themes only) */
	updatedAt?: string;
}

/**
 * Theme export format (for sharing)
 */
export interface ThemeExport {
	name: string;
	description?: string;
	author?: string;
	isDark: boolean;
	colors: ThemeColors;
}

/**
 * User's theme preferences
 */
export interface ThemePreferences {
	/** ID of theme to use in light mode */
	lightThemeId: string;
	/** ID of theme to use in dark mode */
	darkThemeId: string;
}

/**
 * Persisted theme data structure
 */
export interface PersistedThemeData {
	preferences: ThemePreferences;
	userThemes: Theme[];
}

/**
 * Color groups for the theme editor UI
 */
export const COLOR_GROUPS = [
	{
		name: "Background",
		keys: ["background", "foreground", "card", "cardForeground", "popover", "popoverForeground"],
	},
	{
		name: "Interactive",
		keys: ["primary", "primaryForeground", "secondary", "secondaryForeground"],
	},
	{
		name: "Status",
		keys: ["muted", "mutedForeground", "accent", "accentForeground", "destructive"],
	},
	{
		name: "Borders",
		keys: ["border", "input", "ring"],
	},
	{
		name: "Charts",
		keys: ["chart1", "chart2", "chart3", "chart4", "chart5"],
	},
	{
		name: "Sidebar",
		keys: [
			"sidebar",
			"sidebarForeground",
			"sidebarPrimary",
			"sidebarPrimaryForeground",
			"sidebarAccent",
			"sidebarAccentForeground",
			"sidebarBorder",
			"sidebarRing",
		],
	},
] as const;

/**
 * Maps ThemeColors keys to CSS variable names
 */
export const COLOR_VAR_MAP: Record<keyof ThemeColors, string> = {
	background: "--background",
	foreground: "--foreground",
	card: "--card",
	cardForeground: "--card-foreground",
	popover: "--popover",
	popoverForeground: "--popover-foreground",
	primary: "--primary",
	primaryForeground: "--primary-foreground",
	secondary: "--secondary",
	secondaryForeground: "--secondary-foreground",
	muted: "--muted",
	mutedForeground: "--muted-foreground",
	accent: "--accent",
	accentForeground: "--accent-foreground",
	destructive: "--destructive",
	border: "--border",
	input: "--input",
	ring: "--ring",
	chart1: "--chart-1",
	chart2: "--chart-2",
	chart3: "--chart-3",
	chart4: "--chart-4",
	chart5: "--chart-5",
	sidebar: "--sidebar",
	sidebarForeground: "--sidebar-foreground",
	sidebarPrimary: "--sidebar-primary",
	sidebarPrimaryForeground: "--sidebar-primary-foreground",
	sidebarAccent: "--sidebar-accent",
	sidebarAccentForeground: "--sidebar-accent-foreground",
	sidebarBorder: "--sidebar-border",
	sidebarRing: "--sidebar-ring",
};
