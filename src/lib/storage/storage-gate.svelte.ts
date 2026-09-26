/**
 * The storage check the app makes before it loads anything.
 *
 * Three storage failures can't be fixed by retrying, so the app stops and
 * shows `storage-error-screen.svelte` instead of starting on empty state:
 *
 * - `LEGACY_STORAGE` (desktop only): the data dir holds only the JSON files
 *   releases before 2026.4.5 wrote, and no `seaquel.db`;
 * - `STORAGE_CORRUPT`: the metadata file isn't a readable SQLite database;
 * - `NO_DATA_DIR` (desktop only): no `SEAQUEL_DATA_DIR` and no platform
 *   data dir.
 *
 * Any other failure (`STORAGE_ERROR`, `UPSTREAM_UNAVAILABLE`, a network
 * error on web) may be transient, so the probe is retried twice (after
 * about 250 ms, then 1 s). If it still fails, the failure is logged and the
 * app starts; stores whose loads fail then refuse to save (`load-guard.ts`).
 */

import { log } from "$lib/utils/logger";
import { getStorage } from "$lib/storage";

export type BlockingStorageError =
  /** `detail` is the server's message: the data dir and the legacy files. */
  | { kind: "legacy"; detail: string }
  /**
   * `path` is the file the server named (`DATA_DIR/…` on web), or null when
   * its message has another shape. `untouched` is true only when the server
   * said the file wasn't changed.
   */
  | { kind: "corrupt"; detail: string; path: string | null; untouched: boolean }
  /** `detail` is the server's message. */
  | { kind: "no-data-dir"; detail: string };

// The wording of `StorageError::Corrupt` in crates/seaquel-storage/src/error.rs.
const CORRUPT_MARKER = " isn't a readable Seaquel database";
const UNTOUCHED_SENTENCE = "The file wasn't changed.";

function codeAndMessage(error: unknown): { code: string; message: string } | null {
  if (typeof error !== "object" || error === null) return null;
  const { code, message } = error as { code?: unknown; message?: unknown };
  if (typeof code !== "string" || typeof message !== "string") return null;
  // `CoreCallError` prefixes its message with the code.
  const prefix = `${code}: `;
  return { code, message: message.startsWith(prefix) ? message.slice(prefix.length) : message };
}

/** The screen a storage error calls for, or null if the app should carry on. */
export function classifyStorageError(error: unknown): BlockingStorageError | null {
  const parsed = codeAndMessage(error);
  if (!parsed) return null;
  const { code, message } = parsed;
  if (code === "LEGACY_STORAGE") return { kind: "legacy", detail: message };
  if (code === "NO_DATA_DIR") return { kind: "no-data-dir", detail: message };
  if (code === "STORAGE_CORRUPT") {
    const at = message.indexOf(CORRUPT_MARKER);
    return {
      kind: "corrupt",
      detail: message,
      path: at > 0 ? message.slice(0, at) : null,
      untouched: at > 0 && message.trimEnd().endsWith(UNTOUCHED_SENTENCE),
    };
  }
  return null;
}

/** The waits before the second and third probe. */
export const PROBE_RETRY_DELAYS_MS = [250, 1000];

export interface StorageGateOptions {
  retryDelaysMs?: number[];
  sleep?: (ms: number) => Promise<void>;
}

export class StorageGate {
  /** Set when storage can't be used at all; the app shell shows the screen. */
  blocked = $state<BlockingStorageError | null>(null);
  #check: Promise<boolean> | null = null;
  readonly #delays: number[];
  readonly #sleep: (ms: number) => Promise<void>;

  constructor(options: StorageGateOptions = {}) {
    this.#delays = options.retryDelaysMs ?? PROBE_RETRY_DELAYS_MS;
    this.#sleep = options.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms)));
  }

  /**
   * Makes the first storage call, once for every caller. True if the app
   * should start; false if it must stop at the error screen.
   */
  check(): Promise<boolean> {
    this.#check ??= this.#probe();
    return this.#check;
  }

  async #probe(): Promise<boolean> {
    for (let attempt = 0; ; attempt++) {
      try {
        await getStorage().appState.get("lastActiveProjectId");
        return true;
      } catch (error) {
        const blocking = classifyStorageError(error);
        if (blocking) {
          void log.error("Storage can't be opened; showing the storage error screen:", error);
          this.blocked = blocking;
          return false;
        }
        const delay = this.#delays[attempt];
        if (delay === undefined) {
          void log.warn("First storage call failed after retries; starting anyway:", error);
          return true;
        }
        void log.warn(`First storage call failed; retrying in ${delay} ms:`, error);
        await this.#sleep(delay);
      }
    }
  }
}

export const storageGate = new StorageGate();
