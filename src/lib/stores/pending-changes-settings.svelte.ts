import { getStorage } from "$lib/storage";

const SETTING_KEY = "pending_changes_enabled";

class PendingChangesSettingsStore {
  enabled = $state(true);

  async load(): Promise<void> {
    try {
      const raw = await getStorage().appState.get(SETTING_KEY);
      if (raw !== null) {
        this.enabled = raw !== "false";
      }
    } catch {
      // Default to enabled if storage fails
    }
  }

  async setEnabled(value: boolean): Promise<void> {
    this.enabled = value;
    try {
      await getStorage().appState.set(SETTING_KEY, String(value));
    } catch {
      // Silently fail — setting is still updated in memory
    }
  }
}

export const pendingChangesSettingsStore = new PendingChangesSettingsStore();
