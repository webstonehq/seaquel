/**
 * The DuckDB support install around a Core `connect` or `test`.
 *
 * On desktop DuckDB runs in a helper process that is downloaded on first
 * use. Core refuses a DuckDB connect at once with `ENGINE_NOT_INSTALLED`
 * when this version's helper isn't there (it never downloads inside a
 * connect). An interactive connect then opens the install dialog
 * (`$lib/stores/duckdb-install.svelte`) and waits on it; once it installed,
 * the same attempt runs again, typed secrets and all. A background
 * reconnect never opens it. Every connect that meets the dialog while it
 * is open shares it and its one install, and each retries its own connect.
 *
 * The same shape as `withHostKeyPrompt`. Only desktop calls it (for
 * `type === "duckdb"`); the store is imported only in a desktop build, so
 * the web and demo bundles carry neither it nor the dialog.
 */

import { m } from "$lib/paraglide/messages.js";
import { ShownError, createError } from "$lib/errors";
import {
  DuckdbHelperError,
  type DuckdbHelperInstalled,
  type DuckdbHelperProgress,
} from "$lib/api/tauri";
import { isTauri } from "$lib/utils/environment";
import { log } from "$lib/utils/logger";
import { errorCode } from "./client";

export const ENGINE_NOT_INSTALLED = "ENGINE_NOT_INSTALLED";
export const ENGINE_UNAVAILABLE = "ENGINE_UNAVAILABLE";
export const ENGINE_NOT_AVAILABLE = "ENGINE_NOT_AVAILABLE";

/**
 * The user declined or cancelled the install: "DuckDB support isn't
 * installed." The wizard shows it; nothing toasts it on top (it is a
 * `ShownError`: the user just closed the dialog that said so).
 */
export class DuckdbHelperDeclined extends ShownError {
  readonly code = ENGINE_NOT_INSTALLED;
  constructor() {
    const message = m.duckdb_helper_declined();
    super(createError("CONNECTION_FAILED", message, message));
    this.name = "DuckdbHelperDeclined";
  }
}

/**
 * A connect right after an install still answered `ENGINE_NOT_INSTALLED`.
 * The dialog says so with Core's reason (and offers no second download),
 * so it is a `ShownError` too.
 */
export class DuckdbHelperUnusable extends ShownError {
  readonly code = ENGINE_NOT_INSTALLED;
  constructor(readonly reason: string) {
    const message = m.duckdb_helper_unusable({ reason });
    super(createError("CONNECTION_FAILED", message, message));
    this.name = "DuckdbHelperUnusable";
  }
}

/** A DuckDB connect refused for its helper, worded; `code` is Core's. */
export class DuckdbConnectError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "DuckdbConnectError";
  }
}

/** Core's message without the `CODE: ` a `CoreCallError` puts before it. */
function coreMessage(error: unknown, code: string): string {
  const message = error instanceof Error ? error.message : String(error);
  return message.startsWith(`${code}: `) ? message.slice(code.length + 2) : message;
}

/** The engine codes worded for the connect's caller; anything else as it is. */
function worded(error: unknown): unknown {
  switch (errorCode(error)) {
    case ENGINE_UNAVAILABLE:
      return new DuckdbConnectError(ENGINE_UNAVAILABLE, m.duckdb_helper_unavailable());
    case ENGINE_NOT_AVAILABLE:
      return new DuckdbConnectError(ENGINE_NOT_AVAILABLE, m.duckdb_helper_not_available());
    case ENGINE_NOT_INSTALLED:
      return new DuckdbConnectError(ENGINE_NOT_INSTALLED, coreMessage(error, ENGINE_NOT_INSTALLED));
    default:
      return error;
  }
}

type InstallStore = (typeof import("$lib/stores/duckdb-install.svelte"))["duckdbInstallStore"];

/** The one load of the store, shared by connects that meet it at once. */
let storeLoad: Promise<InstallStore> | null = null;

const importStore = () =>
  import("$lib/stores/duckdb-install.svelte").then((s) => s.duckdbInstallStore);
let loadStore: () => Promise<InstallStore> = importStore;

/** Replace how the store is loaded (tests); `null` goes back to the import. */
export function setInstallStoreLoader(loader: (() => Promise<InstallStore>) | null): void {
  loadStore = loader ?? importStore;
  storeLoad = null;
}

/** The install dialog's store, loaded only in a desktop build. */
async function installStore(): Promise<InstallStore | null> {
  if (import.meta.env.VITE_BUILD_TARGET === "web" || import.meta.env.VITE_BUILD_TARGET === "demo") {
    return null;
  }
  // A load that failed (a chunk the webview couldn't fetch) is tried
  // again by the next connect.
  storeLoad ??= loadStore().catch((error: unknown) => {
    storeLoad = null;
    throw error;
  });
  return storeLoad;
}

/**
 * Run `attempt`; on `ENGINE_NOT_INSTALLED` from an `interactive` connect,
 * ask to install DuckDB support and, once it is, run `attempt` once more.
 * A decline or cancel rejects with {@link DuckdbHelperDeclined}; a retry
 * that is still refused with {@link DuckdbHelperUnusable}. A background
 * (`interactive: false`) connect never asks.
 */
export async function withDuckdbHelper<T>(
  attempt: () => Promise<T>,
  options: { interactive: boolean },
): Promise<T> {
  try {
    return await attempt();
  } catch (error) {
    if (errorCode(error) !== ENGINE_NOT_INSTALLED || !options.interactive) throw worded(error);
    let store: InstallStore | null;
    try {
      store = await installStore();
    } catch {
      void log.warn("The DuckDB support dialog didn't load");
      throw worded(error);
    }
    if (!store) throw worded(error);
    const installed = await store.request();
    if (!installed) throw new DuckdbHelperDeclined();
  }
  try {
    return await attempt();
  } catch (error) {
    // The TUI's `after_install` rule: no second download.
    if (errorCode(error) === ENGINE_NOT_INSTALLED) {
      const reason = coreMessage(error, ENGINE_NOT_INSTALLED);
      (await installStore().catch(() => null))?.showUnusable(reason);
      throw new DuckdbHelperUnusable(reason);
    }
    throw worded(error);
  }
}

/**
 * How long after the app is ready the prefetch starts: the first screen,
 * its storage reads and a first connect the user makes go first.
 */
export const PREFETCH_DELAY_MS = 10_000;

/** What the page tells the prefetch; only `connections()` is read after the wait. */
export interface PrefetchPage {
  /** Whether the saved connections were read (a failed load prefetches nothing). */
  loaded: boolean;
  /** A standalone window (the theme editor, the log viewer): only the main window prefetches. */
  standalone: boolean;
  /** The saved connections, every project's. */
  connections: () => readonly { type: string }[];
}

/** Why the page didn't hand it to the store, or how the store's prefetch ended. */
export type PrefetchResult =
  | "not-desktop"
  | "not-loaded"
  | "standalone"
  | "no-duckdb"
  | "metered"
  | import("$lib/stores/duckdb-install.svelte").PrefetchOutcome;

/**
 * Whether the OS says this connection should save data. Only Chromium's
 * webview (Windows' WebView2) has `navigator.connection`; WebKit's don't,
 * so on macOS and Linux this is never true.
 */
function savesData(): boolean {
  try {
    const connection = (globalThis.navigator as { connection?: { saveData?: unknown } } | undefined)
      ?.connection;
    return connection?.saveData === true;
  } catch {
    return false;
  }
}

/**
 * The background prefetch at startup:
 * `PREFETCH_DELAY_MS` after the app is ready, when a
 * saved connection is DuckDB, install this version's helper if it isn't,
 * without asking, as the app downloads its own updates. Desktop only, the
 * main window only, once per page (the store's rule). Never throws and
 * shows nothing: a failure is logged by code, and the dialog covers it at
 * the next DuckDB connect.
 */
export async function prefetchDuckdbHelper(
  page: PrefetchPage,
  options: { delayMs?: number } = {},
): Promise<PrefetchResult> {
  if (
    import.meta.env.VITE_BUILD_TARGET === "web" ||
    import.meta.env.VITE_BUILD_TARGET === "demo" ||
    !isTauri()
  ) {
    return "not-desktop";
  }
  if (page.standalone) return "standalone";
  if (!page.loaded) return "not-loaded";
  const delay = options.delayMs ?? PREFETCH_DELAY_MS;
  if (delay > 0) await new Promise((resolve) => setTimeout(resolve, delay));
  if (!page.connections().some((c) => c.type === "duckdb")) return "no-duckdb";
  if (savesData()) {
    void log.info("DuckDB support: prefetch skipped (the connection saves data)");
    return "metered";
  }
  let store: InstallStore | null;
  try {
    store = await installStore();
  } catch {
    void log.warn("DuckDB support: prefetch skipped (the store didn't load)");
    return "failed";
  }
  if (!store) return "not-desktop";
  return store.prefetch();
}

/**
 * The MCP panel's helper-only install: through the
 * dialog's store, so a dialog opened meanwhile goes straight to this
 * download instead of asking. Desktop only; elsewhere `NOT_SUPPORTED`.
 */
export async function installDuckdbHelperJoined(
  onProgress?: (progress: DuckdbHelperProgress) => void,
): Promise<DuckdbHelperInstalled> {
  const store = isTauri() ? await installStore() : null;
  if (!store) {
    throw new DuckdbHelperError({
      code: "NOT_SUPPORTED",
      message: "DuckDB support is installed only in the desktop app",
    });
  }
  return store.installJoined(onProgress);
}
