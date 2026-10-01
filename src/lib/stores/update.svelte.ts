import type { UpdateInfo } from "$lib/api/tauri";
import { StoredSetting, names, onStoredChange } from "./settings-sync";

export class UpdateStore {
  updateInfo = $state<UpdateInfo | null>(null);
  isDownloaded = $state(false);
  isInstalling = $state(false);
  dismissedThisSession = $state(false);
  skippedVersion = $state<string | null>(null);
  upToDate = $state(false);

  private upToDateTimer: ReturnType<typeof setTimeout> | null = null;

  private initialized = false;
  private readonly stored = new StoredSetting("skippedUpdateVersion", (v) => {
    this.skippedVersion = v;
  });

  constructor() {
    onStoredChange("setting", (ids) =>
      names(ids, this.stored.key) ? this.stored.reload() : undefined,
    );
  }

  get visible(): boolean {
    if (!this.updateInfo) return false;
    if (this.dismissedThisSession) return false;
    if (this.skippedVersion === this.updateInfo.version) return false;
    return true;
  }

  async initialize(): Promise<void> {
    if (this.initialized) return;
    try {
      await this.stored.load();
    } catch (error) {
      console.error("Failed to load skipped update version:", error);
    }
    this.initialized = true;
  }

  showUpToDate(): void {
    this.upToDate = true;
    if (this.upToDateTimer) clearTimeout(this.upToDateTimer);
    this.upToDateTimer = setTimeout(() => {
      this.upToDate = false;
      this.upToDateTimer = null;
    }, 5000);
  }

  setUpdateAvailable(info: UpdateInfo): void {
    this.updateInfo = info;
    this.isDownloaded = false;
    this.dismissedThisSession = false;
  }

  setUpdateDownloaded(info: UpdateInfo): void {
    this.updateInfo = info;
    this.isDownloaded = true;
    this.dismissedThisSession = false;
  }

  async skip(): Promise<void> {
    if (!this.updateInfo) return;
    this.skippedVersion = this.updateInfo.version;
    try {
      await this.stored.set(this.updateInfo.version);
    } catch (error) {
      console.error("Failed to persist skipped version:", error);
    }
  }

  later(): void {
    this.dismissedThisSession = true;
  }

  async install(): Promise<void> {
    if (!this.isDownloaded || this.isInstalling) return;
    this.isInstalling = true;
    try {
      const { installUpdate } = await import("$lib/api/tauri");
      await installUpdate();
    } catch (error) {
      console.error("Failed to install update:", error);
      this.isInstalling = false;
    }
  }
}

export const updateStore = new UpdateStore();
