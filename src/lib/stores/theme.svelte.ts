import { mode } from "mode-watcher";
import { getSettings } from "$lib/hooks/database/library/index";
import type { ChangeSeq, Themes } from "$lib/hooks/database/library/types";
import { errorToast } from "$lib/utils/toast";
import { libraryErrorMessage } from "$lib/hooks/database/library/messages";
import { log } from "$lib/utils/logger";
import type { Theme, ThemePreferences, ThemeExport } from "$lib/types/theme";
import { BUILT_IN_THEMES, DEFAULT_PREFERENCES } from "$lib/themes/presets";
import { applyTheme, cacheThemeColors } from "$lib/themes/apply";
import { validateThemeColors } from "$lib/themes/color-utils";
import { WriteOrder, onStoredChange } from "./settings-sync";

/** The key the `seq` rule records the themes under (one record: preferences and themes). */
const THEMES = "themes";

/** A new theme as the editor or an import makes it. */
export type ThemeInput = Omit<Theme, "id" | "isBuiltIn" | "createdAt" | "updatedAt">;

/** A stored user theme that reads as one (a name, a mode, colours). */
function isTheme(value: unknown): value is Theme {
  const t = value as Partial<Theme> | null;
  return (
    typeof t === "object" &&
    t !== null &&
    typeof t.id === "string" &&
    typeof t.name === "string" &&
    typeof t.isDark === "boolean" &&
    typeof t.colors === "object" &&
    t.colors !== null
  );
}

/** A theme as sent to Core: without what Core sets. */
function themeBody(theme: Theme | ThemeInput): Record<string, unknown> {
  const { id: _id, createdAt: _c, updatedAt: _u, ...body } = theme as Partial<Theme>;
  return { ...body, isBuiltIn: false };
}

/**
 * Theme store - manages theme preferences, user themes, and theme application.
 *
 * Phase 5d-2: each change is one `settings` call (the
 * preference pair, or one user theme added, changed or removed), whose
 * answer holds the preferences and every user theme, applied by the `seq`
 * rule. Another window's change is read again and applied at once.
 */
export class ThemeStore {
  // Reactive state
  preferences = $state<ThemePreferences>({ ...DEFAULT_PREFERENCES });
  userThemes = $state<Theme[]>([]);
  /** True once `initialize` has run, whether or not the load worked: the theme can be applied. */
  isLoaded = $state(false);
  /** Orders the writes' answers and reads (`seq`; no flicker back to an earlier answer). */
  private readonly order = new WriteOrder(THEMES, () => this.reload());
  /** Writes on their way, for `flush`. */
  private readonly pending = new Set<Promise<unknown>>();

  constructor() {
    onStoredChange("theme", () => (this.isLoaded ? this.reload() : undefined));
  }

  // Derived: all available themes (built-in + user)
  allThemes = $derived([...BUILT_IN_THEMES, ...this.userThemes]);

  // Derived: themes filtered by mode
  lightThemes = $derived(this.allThemes.filter((t) => !t.isDark));
  darkThemes = $derived(this.allThemes.filter((t) => t.isDark));

  // Derived: currently selected light theme
  selectedLightTheme = $derived(
    this.allThemes.find((t) => t.id === this.preferences.lightThemeId) ?? BUILT_IN_THEMES[0],
  );

  // Derived: currently selected dark theme
  selectedDarkTheme = $derived(
    this.allThemes.find((t) => t.id === this.preferences.darkThemeId) ?? BUILT_IN_THEMES[1],
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
      const { value, seq } = await getSettings().getThemes();
      this.order.read(value, seq, (t) => this.show(t));
    } catch (error) {
      // Built-in themes still apply.
      console.error("Failed to load theme preferences:", error);
    }
    this.isLoaded = true;
  }

  /** Another window changed a theme or the preferences: read them again and apply. */
  async reload({ again = false } = {}): Promise<void> {
    const { value, seq } = await getSettings().getThemes();
    if (this.order.read(value, seq, (t) => this.show(t), { again })) this.applyActiveTheme();
  }

  private show(themes: Themes): void {
    this.preferences = { ...themes.preferences };
    this.userThemes = themes.userThemes.filter(isTheme);
  }

  /** Runs one `settings` call, shows its answer, and toasts a failure (then reads again). */
  private async write<T>(
    call: () => Promise<{ value: T; seq: ChangeSeq }>,
    themesOf: (value: T) => Themes,
  ): Promise<T | null> {
    const run = this.order.write(call, (value) => this.show(themesOf(value)));
    this.pending.add(run);
    try {
      return await run;
    } catch (error) {
      void log.error("Failed to save theme settings:", error);
      errorToast(libraryErrorMessage(error));
      await this.reload({ again: true }).catch(() => {});
      return null;
    } finally {
      this.pending.delete(run);
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
  async setLightTheme(themeId: string): Promise<void> {
    const theme = this.lightThemes.find((t) => t.id === themeId);
    if (!theme) return;

    this.preferences = {
      ...this.preferences,
      lightThemeId: themeId,
    };

    await this.savePreferences();
  }

  /**
   * Set the theme for dark mode
   */
  async setDarkTheme(themeId: string): Promise<void> {
    const theme = this.darkThemes.find((t) => t.id === themeId);
    if (!theme) return;

    this.preferences = {
      ...this.preferences,
      darkThemeId: themeId,
    };

    await this.savePreferences();
  }

  private async savePreferences(): Promise<void> {
    const { lightThemeId, darkThemeId } = this.preferences;
    await this.write(
      () => getSettings().setThemePreferences(lightThemeId, darkThemeId),
      (t) => t,
    );
  }

  /**
   * Add a new user theme. Core gives it its id; `null` when it wasn't
   * saved (the error is shown).
   */
  async addTheme(theme: ThemeInput): Promise<Theme | null> {
    const created = await this.write(
      () => getSettings().createUserTheme(themeBody(theme)),
      (v) => v.themes,
    );
    if (!created) return null;
    const added = created.themes.userThemes.filter(isTheme).find((t) => t.id === created.id);
    // Shown by now, unless a later write's answer is still to come.
    if (added && !this.userThemes.some((t) => t.id === added.id)) {
      this.userThemes = [...this.userThemes, added];
    }
    return added ?? null;
  }

  /**
   * Update an existing user theme. False when it wasn't saved (the error is
   * shown and the stored themes read again).
   */
  async updateTheme(
    id: string,
    updates: Partial<Omit<Theme, "id" | "isBuiltIn">>,
  ): Promise<boolean> {
    const index = this.userThemes.findIndex((t) => t.id === id);
    if (index === -1) return false;

    const updated: Theme = { ...this.userThemes[index], ...updates };
    // Shown at once; Core's answer then holds its times.
    this.userThemes = [
      ...this.userThemes.slice(0, index),
      updated,
      ...this.userThemes.slice(index + 1),
    ];
    const saved = await this.write(
      () => getSettings().updateUserTheme(id, themeBody(updated)),
      (t) => t,
    );
    return saved !== null;
  }

  /**
   * Delete a user theme. A preference that named it goes back to the
   * default in the same call.
   */
  async deleteTheme(id: string): Promise<void> {
    const theme = this.userThemes.find((t) => t.id === id);
    if (!theme) return;
    await this.write(
      () => getSettings().removeUserTheme(id),
      (t) => t,
    );
  }

  /**
   * Duplicate a theme (creates editable copy)
   */
  duplicateTheme(id: string): Promise<Theme | null> {
    const source = this.allThemes.find((t) => t.id === id);
    if (!source) return Promise.resolve(null);

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
  importTheme(json: string): Promise<Theme | null> {
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
    return this.preferences.lightThemeId === id || this.preferences.darkThemeId === id;
  }

  /** Waits for the theme writes on their way (a window closing). */
  async flush(): Promise<void> {
    await Promise.allSettled(this.pending);
  }
}

export const themeStore = new ThemeStore();
