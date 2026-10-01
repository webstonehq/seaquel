import { log } from "$lib/utils/logger";
import { StoredSetting, names, onStoredChange } from "./settings-sync";

export type EditorKeybindingMode = "default" | "vim" | "emacs";

type ChangeListener = () => void;

function readMode(raw: string | null): EditorKeybindingMode {
  return raw === "vim" || raw === "emacs" ? raw : "default";
}

export class EditorSettingsStore {
  keybindingMode = $state<EditorKeybindingMode>("default");
  private listeners: ChangeListener[] = [];
  private readonly stored = new StoredSetting("editorKeybindingMode", (v) =>
    this.show(readMode(v)),
  );

  constructor() {
    onStoredChange("setting", (ids) =>
      names(ids, this.stored.key) ? this.stored.reload() : undefined,
    );
  }

  private show(mode: EditorKeybindingMode): void {
    if (mode === this.keybindingMode) return;
    this.keybindingMode = mode;
    this.listeners.forEach((fn) => fn());
  }

  async load(): Promise<void> {
    try {
      await this.stored.load();
    } catch (error) {
      // The default applies; a later change still saves (Core writes the key alone).
      void log.warn("Failed to load the editor key bindings:", error);
    }
  }

  async setKeybindingMode(value: EditorKeybindingMode): Promise<void> {
    this.show(value);
    try {
      await this.stored.set(value);
    } catch (error) {
      // The setting still applies here for this session.
      void log.warn("Failed to save the editor key bindings:", error);
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
