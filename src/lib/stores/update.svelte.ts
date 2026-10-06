import type { UpdateInfo } from "$lib/api/tauri";
import { errorCode } from "$lib/core/client";
import { StoredSetting, names, onStoredChange } from "./settings-sync";

export type UpdateChannel = "stable" | "beta";

/** `install_update`'s refusal of a download the channel no longer offers. */
const UPDATE_STALE = "UPDATE_STALE";

export class UpdateStore {
  updateInfo = $state<UpdateInfo | null>(null);
  isDownloaded = $state(false);
  isInstalling = $state(false);
  dismissedThisSession = $state(false);
  skippedVersion = $state<string | null>(null);
  upToDate = $state(false);
  /**
   * The chosen update channel; `null` until one is stored. The app then
   * follows the build's default: beta for a pre-release build, stable
   * otherwise.
   */
  channel = $state<UpdateChannel | null>(null);

  private upToDateTimer: ReturnType<typeof setTimeout> | null = null;

  private initialized = false;
  /** True once a stored channel (or none) was shown: later changes come from a set or another window. */
  private channelShown = false;
  /**
   * Bumped by `forgetUpdate`: a check answering after a later switch or
   * stale install doesn't show what the old channel offered. The
   * `update-downloaded` event (`(app)/+layout.svelte`) isn't guarded; an
   * install of a download from the old channel answers `UPDATE_STALE`.
   */
  private checkGeneration = 0;
  private readonly stored = new StoredSetting("skippedUpdateVersion", (v) => {
    this.skippedVersion = v;
  });
  private readonly storedChannel = new StoredSetting("updateChannel", (v) => {
    const next = v === "stable" || v === "beta" ? v : null;
    const changed = this.channelShown && next !== this.channel;
    this.channel = next;
    this.channelShown = true;
    // Another window switched (or this window's set was refused): an update
    // found on the channel shown until now isn't this channel's.
    if (changed) this.forgetUpdate();
  });

  constructor() {
    onStoredChange("setting", async (ids) => {
      await Promise.all([
        names(ids, this.stored.key) ? this.stored.reload() : undefined,
        names(ids, this.storedChannel.key) ? this.storedChannel.reload() : undefined,
      ]);
    });
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
    try {
      await this.storedChannel.load();
    } catch (error) {
      console.error("Failed to load update channel:", error);
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

  /** Saves the channel, forgets an update found on the old one and checks the new feed. */
  async setChannel(value: UpdateChannel): Promise<void> {
    if (value === this.channel) return;
    this.channel = value;
    this.forgetUpdate();
    try {
      await this.storedChannel.set(value);
    } catch (error) {
      // The stored channel (shown again) is still the one the updater reads.
      console.error("Failed to persist update channel:", error);
    }
    await this.recheck();
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
      // The app restarts after an install; an answer means it didn't.
      this.isInstalling = false;
    } catch (error) {
      this.isInstalling = false;
      if (errorCode(error) === UPDATE_STALE) {
        // The download is from another channel, or the channel has nothing
        // newer any more: drop it and show what the channel offers now.
        this.forgetUpdate();
        await this.recheck();
        return;
      }
      console.error("Failed to install update:", error);
    }
  }

  private forgetUpdate(): void {
    this.checkGeneration++;
    this.updateInfo = null;
    this.isDownloaded = false;
  }

  /** Checks the channel's feed and shows an update it offers. */
  private async recheck(): Promise<void> {
    const generation = ++this.checkGeneration;
    try {
      const { checkForUpdate } = await import("$lib/api/tauri");
      const info = await checkForUpdate();
      if (info && generation === this.checkGeneration) this.setUpdateAvailable(info);
    } catch (error) {
      console.error("Failed to check for updates:", error);
    }
  }
}

export const updateStore = new UpdateStore();
