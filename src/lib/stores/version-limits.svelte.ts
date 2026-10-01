import { StoredSetting, names, onStoredChange } from "./settings-sync";

export type VersionLimitKey = "query_version_limit" | "dashboard_version_limit";

/** A stored limit as the settings show it; `null` when unset or unreadable (the default shows). */
function readLimit(value: string | null): number | null {
  const parsed = value ? parseInt(value, 10) : NaN;
  return Number.isNaN(parsed) ? null : parsed;
}

/**
 * The two version limits the settings page shows (`query_version_limit`,
 * `dashboard_version_limit`), through `settings`: a save made before the
 * read answers isn't undone by it, and another window's change applies at
 * once (`StoredSetting`'s rules). One per settings page.
 */
export class VersionLimitsStore {
  query = $state(100);
  dashboard = $state(100);
  private readonly stored: Record<VersionLimitKey, StoredSetting> = {
    query_version_limit: new StoredSetting("query_version_limit", (v) => {
      this.query = readLimit(v) ?? 100;
    }),
    dashboard_version_limit: new StoredSetting("dashboard_version_limit", (v) => {
      this.dashboard = readLimit(v) ?? 100;
    }),
  };
  private readonly stop: () => void;

  constructor() {
    this.stop = onStoredChange("setting", async (ids) => {
      await Promise.all(
        Object.values(this.stored)
          .filter((s) => names(ids, s.key))
          .map((s) => s.reload()),
      );
    });
  }

  /** Reads both; a failed read rejects (the defaults stay). */
  async load(): Promise<void> {
    await Promise.all(Object.values(this.stored).map((s) => s.load()));
  }

  /** Stores a limit (already clamped by the caller); rejects on a refusal. */
  async save(key: VersionLimitKey, value: number): Promise<void> {
    if (key === "query_version_limit") this.query = value;
    else this.dashboard = value;
    await this.stored[key].set(String(value));
  }

  /** Stops following other windows' changes (the page closed). */
  dispose(): void {
    this.stop();
  }
}
