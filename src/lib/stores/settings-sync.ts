/**
 * The settings stores and Core's `settings` group (phase 5d-2, Decision
 * 20).
 *
 * - **Other windows' changes.** Each store registers what to reload for its
 *   kind of `storageChanged` event (`onStoredChange`); `LibrarySync` calls
 *   `applyStoredChange` for the settings kinds (`setting`, `aiSettings`,
 *   `theme`, `onboarding`, `tutorial`, `importState`), and the stores read
 *   their record again and apply it at once. A store that never loaded
 *   ignores it.
 * - **`StoredSetting`**: one app-state setting (a `SettingKey`) with the
 *   rules every such store shares: a set made while the load is still out
 *   waits for it, and the load then doesn't put back what it read (bug 23);
 *   a value is applied only when its `seq` is newer than the one applied.
 */
import { errorCode } from "$lib/core/client";
import { getSettings } from "$lib/hooks/database/library/index";
import { RowSeqs } from "$lib/hooks/database/library/seqs";
import { STORAGE_FULL, type ChangeSeq, type SettingKey } from "$lib/hooks/database/library/types";
import { m } from "$lib/paraglide/messages.js";
import { errorToast } from "$lib/utils/toast";

/**
 * A settings write that failed: a web user's full storage (`STORAGE_FULL`,
 * nothing was written) is said, as an error toast; anything else is left to
 * the caller, which logs it.
 */
export function toastIfStorageFull(error: unknown): void {
  if (errorCode(error) === STORAGE_FULL) errorToast(m.storage_full());
}

/** The `storageChanged` kinds the settings stores follow. */
export type SettingsKind =
  | "setting"
  | "aiSettings"
  | "theme"
  | "onboarding"
  | "tutorial"
  | "importState";

type Handler = (ids: readonly string[] | null) => Promise<void> | void;

const handlers = new Map<SettingsKind, Set<Handler>>();

/** Reload what `kind` names when another window changes it. Returns the unsubscribe. */
export function onStoredChange(kind: SettingsKind, handler: Handler): () => void {
  let set = handlers.get(kind);
  if (!set) handlers.set(kind, (set = new Set()));
  set.add(handler);
  return () => set.delete(handler);
}

/**
 * Another window changed `kind` (`ids`: the rows or keys it names, or
 * `null` for all of them): each store holding it reads it again. One
 * store's failure doesn't stop the others.
 */
export async function applyStoredChange(
  kind: SettingsKind,
  ids: readonly string[] | null,
): Promise<void> {
  await Promise.allSettled(
    Array.from(handlers.get(kind) ?? [], async (h) => {
      await h(ids);
    }),
  );
}

/** Whether a `setting` change naming `ids` covers `key`. */
export function names(ids: readonly string[] | null, key: string): boolean {
  return ids === null || ids.includes(key);
}

/**
 * One app-state setting, read and written through `settings`, ordered by
 * `WriteOrder`: a set shows its own answer unless a later set is on its
 * way; a read answering while a set is on its way isn't shown, and is read
 * again once the sets are done, so neither a load that started before a
 * set (bug 23) nor another window's change read meanwhile is lost or
 * undoes the set.
 */
export class StoredSetting {
  private readonly order: WriteOrder;
  /** True once a load answered. */
  loaded = false;

  constructor(
    readonly key: SettingKey,
    /** Shows a stored value (`null`: none). */
    private readonly apply: (value: string | null) => void,
  ) {
    this.order = new WriteOrder(key, () => this.load());
  }

  /**
   * Read the stored value and show it (see the rules above); `again`: after
   * a refused set, shown at the same `seq` too. Rejects when the read fails.
   */
  async load({ again = false } = {}): Promise<void> {
    const { value, seq } = await getSettings().getSetting(this.key);
    this.loaded = true;
    this.order.read(value, seq, this.apply, { again });
  }

  /**
   * Store `value` (`null` deletes it). The caller has already shown it; a
   * refusal shows the stored value again, then rejects.
   */
  async set(value: string | null): Promise<void> {
    try {
      await this.order.write(() => getSettings().setSetting(this.key, value), this.apply);
    } catch (error) {
      toastIfStorageFull(error);
      await this.load({ again: true }).catch(() => {});
      throw error;
    }
  }

  /** Another window changed it: read it again, if this store ever loaded. */
  async reload(): Promise<void> {
    if (!this.loaded) return;
    await this.load();
  }
}

/**
 * The order of one store's optimistic writes: each change shows at once
 * and is sent; an answer is shown only when it is the latest write's (an
 * earlier one landing after a later change would flicker it back). A read
 * answering while a write is on its way isn't shown; with `reread`, it is
 * read again once the writes are done, so another window's change read
 * meanwhile isn't lost. `seq` rules which value wins.
 */
export class WriteOrder {
  private latest = 0;
  private pending = 0;
  private missed = false;
  readonly seqs = new RowSeqs();

  constructor(
    private readonly key: string,
    private readonly reread?: () => Promise<void>,
  ) {}

  /**
   * Runs `call` as the latest write; `show` (if given) gets its answer if it
   * still is the latest and nothing newer was shown.
   */
  async write<T>(
    call: () => Promise<{ value: T; seq: ChangeSeq }>,
    show?: (value: T) => void,
  ): Promise<T> {
    const mine = ++this.latest;
    this.pending += 1;
    try {
      const { value, seq } = await call();
      if (show && mine === this.latest && this.pending === 1) {
        if (this.seqs.take(this.key, seq)) show(value);
      } else if (show && !this.missed) {
        this.seqs.note(this.key, seq);
      }
      // Nothing is recorded for a write that shows nothing (its answer holds
      // other windows' earlier changes, which a later read must still show),
      // nor with a read missed (the read again sees this write and must
      // apply even at its `seq`).
      return value;
    } finally {
      this.pending -= 1;
      if (this.pending === 0 && this.missed && this.reread) {
        this.missed = false;
        void this.reread().catch(() => {});
      }
    }
  }

  /**
   * A read's answer, shown when no write is on its way and it is newer.
   * `again`: a read after a refused write, shown at the same `seq` too (the
   * page shows a change that was never stored).
   */
  read<T>(value: T, seq: ChangeSeq, show: (value: T) => void, { again = false } = {}): boolean {
    if (this.pending > 0) {
      this.missed = true;
      return false;
    }
    if (again) {
      if (!this.seqs.isNewer(this.key, seq) && this.seqs.last(this.key) !== seq.n) return false;
      this.seqs.note(this.key, seq);
    } else if (!this.seqs.take(this.key, seq)) {
      return false;
    }
    show(value);
    return true;
  }
}
