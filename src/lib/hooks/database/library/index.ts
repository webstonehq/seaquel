/**
 * `getLibrary`, `getUi`, `getSettings`: the page's library, view state and
 * settings services. Every build stores them through Core (the `library`,
 * `ui` and `settings` groups, sharing the storage client's write queue):
 * desktop and web, and the demo, whose Core runs in the page (phase 8).
 */
import { getStorage } from "$lib/storage/db";
import type { RustStorageClient } from "$lib/storage/rust-client";
import { CoreLibrary } from "./core-library";
import { CoreSettings } from "./core-settings";
import { CoreUi } from "./core-ui";
import type { LibraryService, SettingsService, UiService } from "./types";

export * from "./types";
export { CoreLibrary, type LibraryCaller } from "./core-library";
export { CoreUi, type UiCaller } from "./core-ui";
export { CoreSettings, type SettingsCaller } from "./core-settings";
export { ChangeFeed, type StorageChange, type ReloadReason } from "./change-feed";
export { RowSeqs, NEW } from "./seqs";

let override: LibraryService | null = null;
let coreLibrary: CoreLibrary | null = null;

/** Replace the page's library (tests); `null` goes back to the default. */
export function setLibrary(next: LibraryService | null): void {
  override = next;
}

export function getLibrary(): LibraryService {
  if (override) return override;
  return (coreLibrary ??= new CoreLibrary(() => getStorage() as unknown as RustStorageClient));
}

// -------- The `ui` group (phase 5d-2) --------

let uiOverride: UiService | null = null;
let coreUi: CoreUi | null = null;

/** Replace the page's `UiService` (tests); `null` goes back to the default. */
export function setUi(next: UiService | null): void {
  uiOverride = next;
}

/** The page's `UiService` (a window's view state): Core's `ui` group. */
export function getUi(): UiService {
  if (uiOverride) return uiOverride;
  return (coreUi ??= new CoreUi(() => getStorage() as unknown as RustStorageClient));
}

// -------- The `settings` group (phase 5d-2) --------

let settingsOverride: SettingsService | null = null;
let coreSettings: CoreSettings | null = null;

/** Replace the page's `SettingsService` (tests); `null` goes back to the default. */
export function setSettings(next: SettingsService | null): void {
  settingsOverride = next;
}

/**
 * The page's `SettingsService` (settings, AI settings, themes, onboarding,
 * tutorial progress, import state): Core's `settings` group.
 */
export function getSettings(): SettingsService {
  if (settingsOverride) return settingsOverride;
  return (coreSettings ??= new CoreSettings(() => getStorage() as unknown as RustStorageClient));
}
