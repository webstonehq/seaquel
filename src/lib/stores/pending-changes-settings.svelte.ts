import { log } from "$lib/utils/logger";
import { StoredSetting, names, onStoredChange } from "./settings-sync";

export class PendingChangesSettingsStore {
  enabled = $state(true);
  private readonly stored = new StoredSetting("pending_changes_enabled", (v) => this.show(v));

  constructor() {
    onStoredChange("setting", (ids) =>
      names(ids, this.stored.key) ? this.stored.reload() : undefined,
    );
  }

  /** Unset reads as on. */
  private show(raw: string | null): void {
    this.enabled = raw !== "false";
  }

  async load(): Promise<void> {
    try {
      await this.stored.load();
    } catch (error) {
      // Enabled by default when the setting can't be read.
      void log.warn("Failed to load the pending changes setting:", error);
    }
  }

  async setEnabled(value: boolean): Promise<void> {
    this.enabled = value;
    try {
      await this.stored.set(String(value));
    } catch (error) {
      // The setting still applies here for this session.
      void log.warn("Failed to save the pending changes setting:", error);
    }
  }
}

export const pendingChangesSettingsStore = new PendingChangesSettingsStore();
