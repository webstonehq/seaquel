import { load } from "@tauri-apps/plugin-store";
import { mode } from "mode-watcher";
import type {
	Theme,
	ThemeColors,
	ThemePreferences,
	ThemeExport,
	PersistedThemeData,
} from "$lib/types/theme";
import { BUILT_IN_THEMES, DEFAULT_PREFERENCES } from "$lib/themes/presets";
import { applyTheme, cacheThemeColors } from "$lib/themes/apply";
import { validateThemeColors } from "$lib/themes/color-utils";

const STORE_FILE = "themes.json";

/**
 * Theme store - manages theme preferences, user themes, and theme application
 */
class ThemeStore {
	// Reactive state
	preferences = $state<ThemePreferences>({ ...DEFAULT_PREFERENCES });
	userThemes = $state<Theme[]>([]);
	isLoaded = $state(false);

	// Persistence timer for debouncing
	private persistenceTimer: ReturnType<typeof setTimeout> | null = null;
	private readonly PERSISTENCE_DEBOUNCE_MS = 500;

	// Derived: all available themes (built-in + user)
	allThemes = $derived([...BUILT_IN_THEMES, ...this.userThemes]);

	// Derived: themes filtered by mode
	lightThemes = $derived(this.allThemes.filter((t) => !t.isDark));
	darkThemes = $derived(this.allThemes.filter((t) => t.isDark));

	// Derived: currently selected light theme
	selectedLightTheme = $derived(
		this.allThemes.find((t) => t.id === this.preferences.lightThemeId) ?? BUILT_IN_THEMES[0]
	);

	// Derived: currently selected dark theme
	selectedDarkTheme = $derived(
		this.allThemes.find((t) => t.id === this.preferences.darkThemeId) ?? BUILT_IN_THEMES[1]
	);

	// Derived: active theme based on current mode
	activeTheme = $derived.by(() => {
		const isDark = mode.current === "dark";
		return isDark ? this.selectedDarkTheme : this.selectedLightTheme;
	});

	/**
	 * Initialize the theme store - load from persistence
	 */
	async initialize(): Promise<void> {
		try {
			const store = await load(STORE_FILE, {
				autoSave: false,
				defaults: {
					preferences: DEFAULT_PREFERENCES,
					userThemes: [],
				},
			});

			const preferences = (await store.get("preferences")) as ThemePreferences | null;
			const userThemes = (await store.get("userThemes")) as Theme[] | null;

			if (preferences) {
				this.preferences = preferences;
			}
			if (userThemes) {
				this.userThemes = userThemes;
			}

			this.isLoaded = true;
		} catch (error) {
			console.error("Failed to load theme preferences:", error);
			this.isLoaded = true;
		}
	}

	/**
	 * Apply the active theme to the DOM
	 */
	applyActiveTheme(): void {
		if (!this.isLoaded) return;

		const theme = this.activeTheme;
		applyTheme(theme);

		// Cache for flash prevention
		cacheThemeColors(theme.colors, theme.isDark);
	}

	/**
	 * Set the theme for light mode
	 */
	setLightTheme(themeId: string): void {
		const theme = this.lightThemes.find((t) => t.id === themeId);
		if (!theme) return;

		this.preferences = {
			...this.preferences,
			lightThemeId: themeId,
		};

		this.schedulePersist();
	}

	/**
	 * Set the theme for dark mode
	 */
	setDarkTheme(themeId: string): void {
		const theme = this.darkThemes.find((t) => t.id === themeId);
		if (!theme) return;

		this.preferences = {
			...this.preferences,
			darkThemeId: themeId,
		};

		this.schedulePersist();
	}

	/**
	 * Add a new user theme
	 */
	addTheme(
		theme: Omit<Theme, "id" | "isBuiltIn" | "createdAt" | "updatedAt">
	): Theme {
		const now = new Date().toISOString();
		const newTheme: Theme = {
			...theme,
			id: crypto.randomUUID(),
			isBuiltIn: false,
			createdAt: now,
			updatedAt: now,
		};

		this.userThemes = [...this.userThemes, newTheme];
		this.schedulePersist();

		return newTheme;
	}

	/**
	 * Update an existing user theme
	 */
	updateTheme(id: string, updates: Partial<Omit<Theme, "id" | "isBuiltIn">>): void {
		const index = this.userThemes.findIndex((t) => t.id === id);
		if (index === -1) return;

		const existing = this.userThemes[index];
		const updated: Theme = {
			...existing,
			...updates,
			updatedAt: new Date().toISOString(),
		};

		this.userThemes = [
			...this.userThemes.slice(0, index),
			updated,
			...this.userThemes.slice(index + 1),
		];

		this.schedulePersist();
	}

	/**
	 * Delete a user theme
	 */
	deleteTheme(id: string): void {
		const theme = this.userThemes.find((t) => t.id === id);
		if (!theme) return;

		this.userThemes = this.userThemes.filter((t) => t.id !== id);

		// If this theme was selected, reset to default
		if (this.preferences.lightThemeId === id) {
			this.preferences = {
				...this.preferences,
				lightThemeId: "default-light",
			};
		}
		if (this.preferences.darkThemeId === id) {
			this.preferences = {
				...this.preferences,
				darkThemeId: "default-dark",
			};
		}

		this.schedulePersist();
	}

	/**
	 * Duplicate a theme (creates editable copy)
	 */
	duplicateTheme(id: string): Theme | null {
		const source = this.allThemes.find((t) => t.id === id);
		if (!source) return null;

		return this.addTheme({
			name: `${source.name} (Copy)`,
			description: source.description,
			author: source.author,
			isDark: source.isDark,
			colors: { ...source.colors },
		});
	}

	/**
	 * Import a theme from JSON
	 */
	importTheme(json: string): Theme {
		const parsed = JSON.parse(json) as ThemeExport;

		// Validate required fields
		if (!parsed.name || typeof parsed.name !== "string") {
			throw new Error("Theme must have a name");
		}
		if (typeof parsed.isDark !== "boolean") {
			throw new Error("Theme must specify isDark");
		}
		if (!validateThemeColors(parsed.colors)) {
			throw new Error("Theme has invalid or missing color values");
		}

		return this.addTheme({
			name: parsed.name,
			description: parsed.description,
			author: parsed.author,
			isDark: parsed.isDark,
			colors: parsed.colors,
		});
	}

	/**
	 * Export a theme to JSON
	 */
	exportTheme(id: string): string {
		const theme = this.allThemes.find((t) => t.id === id);
		if (!theme) {
			throw new Error("Theme not found");
		}

		const exportData: ThemeExport = {
			name: theme.name,
			description: theme.description,
			author: theme.author,
			isDark: theme.isDark,
			colors: theme.colors,
		};

		return JSON.stringify(exportData, null, 2);
	}

	/**
	 * Get a theme by ID
	 */
	getTheme(id: string): Theme | undefined {
		return this.allThemes.find((t) => t.id === id);
	}

	/**
	 * Check if a theme is currently active
	 */
	isThemeActive(id: string): boolean {
		return (
			this.preferences.lightThemeId === id || this.preferences.darkThemeId === id
		);
	}

	// Persistence

	private schedulePersist(): void {
		if (this.persistenceTimer) {
			clearTimeout(this.persistenceTimer);
		}
		this.persistenceTimer = setTimeout(() => {
			this.persist();
			this.persistenceTimer = null;
		}, this.PERSISTENCE_DEBOUNCE_MS);
	}

	private async persist(): Promise<void> {
		try {
			const store = await load(STORE_FILE, {
				autoSave: true,
				defaults: {
					preferences: DEFAULT_PREFERENCES,
					userThemes: [],
				},
			});

			await store.set("preferences", this.preferences);
			await store.set("userThemes", this.userThemes);
			await store.save();
		} catch (error) {
			console.error("Failed to persist theme settings:", error);
		}
	}

	/**
	 * Flush pending persistence immediately
	 */
	flush(): void {
		if (this.persistenceTimer) {
			clearTimeout(this.persistenceTimer);
			this.persistenceTimer = null;
			this.persist();
		}
	}
}

export const themeStore = new ThemeStore();
