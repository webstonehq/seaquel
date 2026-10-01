/**
 * `getLibrary`: the page's `LibraryService`, picked as `getEditService`
 * picks: desktop and web store the library through Core (`CoreLibrary`,
 * the `library` group, sharing the storage client's write queue); the demo,
 * which has no Core, keeps it in TypeScript over its sql.js database
 * (`TsLibrary`) until phase 8.
 */
import { demoDatabase, getStorage } from "$lib/storage/db";
import type { RustStorageClient } from "$lib/storage/rust-client";
import { isTauri, isWeb } from "$lib/utils/environment";
import { CoreLibrary } from "./core-library";
import { CoreSettings } from "./core-settings";
import { CoreUi } from "./core-ui";
import type { TsLibrary } from "./ts-library";
import type { TsSettings } from "./ts-settings";
import type { TsUi } from "./ts-ui";
import type {
  ConnectionDraft,
  LibraryService,
  Seqd,
  SettingsService,
  UiService,
  WireConnection,
} from "./types";

export * from "./types";
export { CoreLibrary, type LibraryCaller } from "./core-library";
export { CoreUi, type UiCaller } from "./core-ui";
export { CoreSettings, type SettingsCaller } from "./core-settings";
export { ChangeFeed, type StorageChange, type ReloadReason } from "./change-feed";
export { RowSeqs, NEW } from "./seqs";

/** The demo's library: `TsLibrary` plus its fixed demo connection. */
export interface DemoLibrary extends LibraryService {
  putDemoConnection(id: string, draft: ConnectionDraft): Promise<Seqd<WireConnection>>;
}

let override: LibraryService | null = null;
let coreLibrary: CoreLibrary | null = null;
let demoLibrary: DemoLibrary | null = null;

/** Replace the page's library (tests); `null` goes back to the default. */
export function setLibrary(next: LibraryService | null): void {
  override = next;
}

export function getLibrary(): LibraryService {
  if (override) return override;
  if (isTauri() || isWeb()) {
    return (coreLibrary ??= new CoreLibrary(() => getStorage() as unknown as RustStorageClient));
  }
  return (demoLibrary ??= lazyDemoLibrary());
}

/** Whether `library` stores the demo's fixed connection (`TsLibrary`). */
export function isDemoLibrary(library: LibraryService): library is DemoLibrary {
  return typeof (library as Partial<DemoLibrary>).putDemoConnection === "function";
}

/**
 * The demo's `TsLibrary`, made on first use over the database its storage
 * client uses, so the other builds never bundle it. A failed open is
 * retried on the next call.
 */
function lazyDemoLibrary(): DemoLibrary {
  let pending: Promise<TsLibrary> | null = null;
  const open = () => {
    pending ??= Promise.all([demoDatabase(), import("./ts-library")])
      .then(([db, { TsLibrary }]) => new TsLibrary(db))
      .catch((error: unknown) => {
        pending = null;
        throw error;
      });
    return pending;
  };
  return new Proxy({} as DemoLibrary, {
    get(_target, method: string) {
      return async (...args: unknown[]) => {
        const library = (await open()) as unknown as Record<string, (...a: unknown[]) => unknown>;
        return library[method](...args);
      };
    },
  });
}

// -------- The `ui` group (phase 5d-2) --------

let uiOverride: UiService | null = null;
let coreUi: CoreUi | null = null;
let demoUi: UiService | null = null;

/** Replace the page's `UiService` (tests); `null` goes back to the default. */
export function setUi(next: UiService | null): void {
  uiOverride = next;
}

/**
 * The page's `UiService` (a window's view state), picked as `getLibrary`
 * picks: Core's `ui` group on desktop and web, `TsUi` over the demo's
 * sql.js file in the demo.
 */
export function getUi(): UiService {
  if (uiOverride) return uiOverride;
  if (isTauri() || isWeb()) {
    return (coreUi ??= new CoreUi(() => getStorage() as unknown as RustStorageClient));
  }
  return (demoUi ??= lazyDemoUi());
}

/** The demo's `TsUi`, made on first use over the demo's database. */
function lazyDemoUi(): UiService {
  let pending: Promise<TsUi> | null = null;
  const open = () => {
    pending ??= Promise.all([demoDatabase(), import("./ts-ui")])
      .then(([db, { TsUi }]) => new TsUi(db))
      .catch((error: unknown) => {
        pending = null;
        throw error;
      });
    return pending;
  };
  return {
    windowGet: async (...a) => (await open()).windowGet(...a),
    windowActivate: async (...a) => (await open()).windowActivate(...a),
    windowStateLoad: async (...a) => (await open()).windowStateLoad(...a),
    windowStateSave: async (...a) => (await open()).windowStateSave(...a),
    // The demo has no `pagehide` save.
    windowStateSaveKeepalive: () => false,
  };
}

// -------- The `settings` group (phase 5d-2) --------

let settingsOverride: SettingsService | null = null;
let coreSettings: CoreSettings | null = null;
let demoSettings: SettingsService | null = null;

/** Replace the page's `SettingsService` (tests); `null` goes back to the default. */
export function setSettings(next: SettingsService | null): void {
  settingsOverride = next;
}

/**
 * The page's `SettingsService` (settings, AI settings, themes, onboarding,
 * tutorial progress, import state), picked as `getLibrary` picks: Core's
 * `settings` group on desktop and web, `TsSettings` over the demo's sql.js
 * file in the demo.
 */
export function getSettings(): SettingsService {
  if (settingsOverride) return settingsOverride;
  if (isTauri() || isWeb()) {
    return (coreSettings ??= new CoreSettings(() => getStorage() as unknown as RustStorageClient));
  }
  return (demoSettings ??= lazyDemoSettings());
}

/** The demo's `TsSettings`, made on first use over the demo's database. */
function lazyDemoSettings(): SettingsService {
  let pending: Promise<TsSettings> | null = null;
  const open = () => {
    pending ??= Promise.all([demoDatabase(), import("./ts-settings")])
      .then(([db, { TsSettings }]) => new TsSettings(db))
      .catch((error: unknown) => {
        pending = null;
        throw error;
      });
    return pending;
  };
  return new Proxy({} as SettingsService, {
    get(_target, method: string) {
      return async (...args: unknown[]) => {
        const settings = (await open()) as unknown as Record<string, (...a: unknown[]) => unknown>;
        return settings[method](...args);
      };
    },
  });
}
