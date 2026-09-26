import { getStorage } from "$lib/storage";

export type EditorKeybindingMode = "default" | "vim" | "emacs";

const SETTING_KEY = "editorKeybindingMode";

type ChangeListener = () => void;

class EditorSettingsStore {
  keybindingMode = $state<EditorKeybindingMode>("default");
  private listeners: ChangeListener[] = [];

  async load(): Promise<void> {
    try {
      const raw = await getStorage().appState.get(SETTING_KEY);
      if (raw === "vim" || raw === "emacs") {
        this.keybindingMode = raw;
      }
    } catch {
      // Default to "default" if storage fails
    }
  }

  async setKeybindingMode(value: EditorKeybindingMode): Promise<void> {
    this.keybindingMode = value;
    this.listeners.forEach((fn) => fn());
    try {
      await getStorage().appState.set(SETTING_KEY, value);
    } catch {
      // Silently fail — setting is still updated in memory
    }
  }

  onChange(fn: ChangeListener): () => void {
    this.listeners.push(fn);
    return () => {
      this.listeners = this.listeners.filter((l) => l !== fn);
    };
  }
}

export const editorSettingsStore = new EditorSettingsStore();
