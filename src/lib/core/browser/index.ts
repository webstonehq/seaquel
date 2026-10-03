/**
 * Core in the demo's page (phase 8). `openBrowserCore` starts it:
 *
 * 1. deletes the old demo file's `localStorage` keys, unread (Q2 C);
 * 2. opens the IndexedDB snapshot store, or, if IndexedDB can't be used,
 *    runs in memory with a `STORAGE_UNAVAILABLE` notice;
 *    A snapshot it can't read is left alone, and Core runs in memory with
 *    the same notice;
 * 3. opens the module on the stored snapshot, or on a new file. A snapshot
 *    that doesn't open (`STORAGE_CORRUPT`) or makes Core trap is moved to
 *    `meta.db.unreadable` and Core starts empty, with a `STORAGE_CORRUPT`
 *    notice. Past the restart cap the open rejects with `CORE_FAILED`;
 * 4. replays the view state the last page kept in its journal
 *    (`$lib/storage/view-state-journal`), so Core has it before the page
 *    loads it;
 * 5. saves the snapshot when the page goes away (`pagehide`, hidden), and
 *    routes a trap the page sees outside any call to the restart.
 *
 * The module gets the page's fetch bridge (`./fetch-bridge.ts`) for the
 * assistant's model calls, at this open and at every restart's. The
 * visitor's key never reaches the module's file: it lives in the page's
 * memory (`$lib/services/session-keys`) and goes with each call.
 *
 * Demo only: the module is loaded here behind
 * `import.meta.env.VITE_BUILD_TARGET === "demo"`, which Rollup folds, so
 * desktop and web bundles never contain it. Nothing imports this file
 * outside the demo's start and the tests.
 *
 * The notices are for the page to show once; the codes are demo only.
 */
import { setBrowserCoreClient } from "$lib/core";
import { setBrowserCoreTransport } from "$lib/storage/rust-client";
import { replayViewStateJournal } from "$lib/storage/view-state-journal";
import { log } from "$lib/utils/logger";
import type { CoreClient } from "../client";
import { browserCoreClient } from "./client";
import type { DuckDbBridge } from "./duckdb-bridge";
import { makeFetchBridge } from "./fetch-bridge";
import { deleteOldKeys } from "./old-keys";
import { openSnapshotStore, type SnapshotStore } from "./snapshot-store";
import { BrowserCore, type BrowserModule, type BrowserNotice, type FetchBridge } from "./transport";

export { makeDuckDbBridge, type DuckDbBridge, type PageDuckDbBridge } from "./duckdb-bridge";
export { makeFetchBridge } from "./fetch-bridge";
export {
  BrowserCore,
  CORE_FAILED,
  CORE_RESTARTED,
  STORAGE_CORRUPT,
  type BrowserModule,
  type BrowserNotice,
  type FetchBridge,
} from "./transport";

/**
 * IndexedDB can't be used, or the stored snapshot couldn't be read from it:
 * Core runs in memory and nothing is kept (a snapshot that was never read
 * is never overwritten).
 */
export const STORAGE_UNAVAILABLE = "STORAGE_UNAVAILABLE";

export interface OpenBrowserCoreOptions {
  /** The module's exports; the demo build's own module when left out. */
  module?: BrowserModule;
  bridge: DuckDbBridge & { closeAll?(): Promise<void> };
  /**
   * The assistant's fetch bridge (phase 6 Task 8): the page's `fetch`
   * (`makeFetchBridge()`) when left out, `null` for none (every `ai` call
   * is then `NOT_SUPPORTED`). Tests pass one that reaches only a mock.
   */
  fetch?: FetchBridge | null;
  /** A store to use instead of IndexedDB (tests). */
  store?: SnapshotStore;
  /** IndexedDB; `null` when the page has none. */
  indexedDB?: IDBFactory | null;
  localStorage?: Storage | null;
  /** For `pagehide` and trap events; `null` for none. */
  window?: Pick<Window, "addEventListener" | "removeEventListener"> | null;
  /** For `visibilitychange`; `null` for none. */
  document?: Pick<Document, "visibilityState" | "addEventListener" | "removeEventListener"> | null;
  /** How long reading the stored snapshot may take (default `LOAD_TIMEOUT_MS`). */
  loadTimeoutMs?: number;
  /** A restart's wait for saves in flight (`BrowserCore`'s `SAVE_WAIT_MS`). */
  saveWaitMs?: number;
}

/**
 * Reading the snapshot gets this long; past it Core runs in memory, as for
 * a failed read, and the stored snapshot is left alone.
 */
export const LOAD_TIMEOUT_MS = 3_000;

export interface OpenedBrowserCore {
  core: BrowserCore;
  client: CoreClient;
  notices: BrowserNotice[];
  /** Removes the page listeners and stops snapshotting. */
  close(): void;
}

/**
 * The demo build's module, instantiated. The branch is on the build-time
 * constant itself, so Rollup drops the import from every other build.
 */
const loadDemoModule =
  import.meta.env.VITE_BUILD_TARGET === "demo"
    ? async (): Promise<BrowserModule> => {
        const glue = await import("$lib/wasm/browser-pkg/seaquel_browser.js");
        const { default: wasmUrl } =
          await import("$lib/wasm/browser-pkg/seaquel_browser_bg.wasm?url");
        await glue.default({ module_or_path: fetch(wasmUrl) });
        return glue as unknown as BrowserModule;
      }
    : null;

export async function openBrowserCore(options: OpenBrowserCoreOptions): Promise<OpenedBrowserCore> {
  const notices: BrowserNotice[] = [];
  deleteOldKeys(options.localStorage === undefined ? undefined : options.localStorage);

  const unavailable = (message: string) => notices.push({ code: STORAGE_UNAVAILABLE, message });
  let store: SnapshotStore | null = options.store ?? null;
  let image: Uint8Array | null = null;
  if (!options.store) {
    try {
      store = await openSnapshotStore(
        options.indexedDB === undefined ? globalThis.indexedDB : options.indexedDB,
      );
    } catch (error) {
      void log.warn("IndexedDB can't be used; the demo keeps nothing:", errorName(error));
      store = null;
      unavailable(
        "This browser isn't letting the demo store data, so nothing you do here is kept after you close the page.",
      );
    }
  }
  if (store) {
    try {
      let timer: ReturnType<typeof setTimeout> | undefined;
      image = await Promise.race([
        store.load(),
        new Promise<never>((_, reject) => {
          timer = setTimeout(
            () => reject(new Error("TimeoutError")),
            options.loadTimeoutMs ?? LOAD_TIMEOUT_MS,
          );
        }),
      ]).finally(() => clearTimeout(timer));
    } catch (error) {
      // Not shown to be corrupt, only unread (or too slow to read): keep it
      // as it is and store nothing over it this time.
      void log.warn("Reading the demo's saved data failed:", errorName(error));
      store = null;
      image = null;
      unavailable(
        "The demo couldn't read its saved data this time, so nothing you do here is kept after you close the page. The saved data is left as it was.",
      );
    }
  }

  const module =
    options.module ??
    (await (loadDemoModule ?? (() => Promise.reject(new Error("not the demo build"))))());
  const core = await BrowserCore.open({
    module,
    bridge: options.bridge,
    store,
    image,
    saveWaitMs: options.saveWaitMs,
    fetch: options.fetch === undefined ? makeFetchBridge() : (options.fetch ?? undefined),
  });
  notices.push(...core.notices);
  // The view state the last page saved at `pagehide` (Task 7 probe, item 1),
  // before anything loads it. Core keeps it only if its `rev` is newer.
  await replayViewStateJournal(
    (body) => core.call(body),
    options.localStorage === undefined ? undefined : options.localStorage,
  );

  const win = options.window === undefined ? globalThis.window : options.window;
  const doc = options.document === undefined ? globalThis.document : options.document;
  const onPageHide = () => core.flushNow();
  const onVisibility = () => {
    if (doc?.visibilityState === "hidden") core.flushNow();
  };
  const onError = (event: Event) => {
    if (core.noticeTrap((event as ErrorEvent).error)) event.preventDefault();
  };
  const onRejection = (event: Event) => {
    if (core.noticeTrap((event as PromiseRejectionEvent).reason)) event.preventDefault();
  };
  win?.addEventListener("pagehide", onPageHide);
  doc?.addEventListener("visibilitychange", onVisibility);
  win?.addEventListener("error", onError);
  win?.addEventListener("unhandledrejection", onRejection);

  const client = browserCoreClient(core);
  return {
    core,
    client,
    notices,
    close() {
      win?.removeEventListener("pagehide", onPageHide);
      doc?.removeEventListener("visibilitychange", onVisibility);
      win?.removeEventListener("error", onError);
      win?.removeEventListener("unhandledrejection", onRejection);
      core.close();
    },
  };
}

/**
 * Makes `opened` the page's Core: `getCoreClient()` and `RustStorageClient`
 * use it from now on (demo builds only).
 */
export function useBrowserCore(opened: OpenedBrowserCore): void {
  setBrowserCoreClient(opened.client);
  setBrowserCoreTransport(async (body) => JSON.parse(await opened.core.call(body)) as unknown);
}

function errorName(error: unknown): string {
  return error instanceof Error ? error.name : typeof error;
}
