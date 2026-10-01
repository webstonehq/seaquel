/**
 * Never save what failed to load.
 *
 * A few stored things are still saved by replacing everything (the shared
 * repos, a window's view state of a project) or by writing one record whole
 * (the license). If the load failed and the store fell
 * back to empty or default state, the next save would overwrite the stored
 * data with that. So a store marks a failed load, and its saves refuse until
 * a later load succeeds.
 *
 * - Interactive saves throw `NotLoadedError`; the UI shows it with
 *   `errorToast`.
 * - Background (debounced) saves call `skipUnloadedSave`: it logs, shows one
 *   toast per session, and the caller returns without writing.
 */

import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";
import { log } from "$lib/utils/logger";

export class NotLoadedError extends Error {
  /** What couldn't be loaded, for the log (not shown to the user). */
  readonly what: string;
  constructor(what: string) {
    super(m.storage_save_refused());
    this.name = "NotLoadedError";
    this.what = what;
  }
}

let toasted = false;

/**
 * Logs a refused background save, and tells the user once per session. A
 * save refused only because the load is still running is logged, not
 * toasted: nothing failed.
 */
export function skipUnloadedSave(what: string, options: { pending?: boolean } = {}): void {
  if (options.pending) {
    void log.warn(`Not saving ${what}: it is still loading, and saving would overwrite it`);
    return;
  }
  void log.warn(`Not saving ${what}: it failed to load, and saving would overwrite it`);
  if (toasted) return;
  toasted = true;
  errorToast(m.storage_save_refused());
}

/** For tests: lets the next refused save toast again. */
export function resetLoadGuardToast(): void {
  toasted = false;
}
