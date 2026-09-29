/**
 * The GUI's side of the change sequence (phase 5d-1, Decision 17).
 *
 * Every write answer and every list carries a `ChangeSeq { epoch, n }`.
 * For each row (and each per-project version list) this keeps the `n` it
 * last applied, and applies an incoming value only when its `n` is higher:
 * a list that started before this page's own write committed can't
 * overwrite that write's newer answer, and a deleted row's number stays
 * behind (a tombstone) so an older list can't bring it back.
 *
 * A different `epoch` is a new workspace (the web server evicted and
 * reopened it, or the app restarted): the numbers start again, so every
 * recorded one is dropped and the listeners reload every list they hold.
 *
 * It also tracks this page's writes in flight, per row, so a refetch of a
 * row waits for them before it is applied (Decision 18); the higher `n`
 * then wins.
 */
import type { ChangeSeq } from "./types";

export class RowSeqs {
  /** The epoch of the workspace the recorded numbers belong to. */
  epoch: string | null = null;
  private readonly applied = new Map<string, number>();
  private readonly inflight = new Map<string, Set<Promise<unknown>>>();
  private readonly newEpochListeners = new Set<() => void>();
  /** Epochs already replaced: a late result from one of them is ignored. */
  private readonly retired = new Set<string>();

  /** Told when a result or event shows a new epoch. Returns the unsubscribe. */
  onNewEpoch(listener: () => void): () => void {
    this.newEpochListeners.add(listener);
    return () => this.newEpochListeners.delete(listener);
  }

  /**
   * Records `seq`'s epoch: `current` for the workspace the recorded numbers
   * belong to; `switched` for a new one, which drops every recorded number
   * and tells the listeners (once); `stale` for an epoch already replaced
   * (a late answer from before an eviction), which is ignored.
   */
  observe(seq: ChangeSeq): "current" | "switched" | "stale" {
    if (this.epoch === null) {
      this.epoch = seq.epoch;
      return "current";
    }
    if (seq.epoch === this.epoch) return "current";
    if (this.retired.has(seq.epoch)) return "stale";
    this.retired.add(this.epoch);
    this.epoch = seq.epoch;
    this.applied.clear();
    // A copy: a listener may unsubscribe while this runs.
    for (const listener of Array.from(this.newEpochListeners)) listener();
    return "switched";
  }

  /** Whether a value at `seq` for `key` is newer than the one applied. Records nothing. */
  isNewer(key: string, seq: ChangeSeq): boolean {
    if (this.retired.has(seq.epoch)) return false;
    if (this.epoch !== null && seq.epoch !== this.epoch) return true;
    const last = this.applied.get(key);
    return last === undefined || seq.n > last;
  }

  /**
   * Take a value at `seq` for `key` if it is newer than the one applied, and
   * record it; false leaves the applied one (a stale list or refetch, or one
   * from a replaced epoch).
   */
  take(key: string, seq: ChangeSeq): boolean {
    if (this.observe(seq) === "stale") return false;
    const last = this.applied.get(key);
    if (last !== undefined && seq.n <= last) return false;
    this.applied.set(key, seq.n);
    return true;
  }

  /** Record `seq` for `key` (this page's own write answer), unless its epoch was replaced. */
  note(key: string, seq: ChangeSeq): void {
    if (this.observe(seq) === "stale") return;
    const last = this.applied.get(key);
    if (last === undefined || seq.n > last) this.applied.set(key, seq.n);
  }

  /** The number applied for `key`, if any. */
  last(key: string): number | undefined {
    return this.applied.get(key);
  }

  /**
   * Run one of this page's writes for the rows `keys` name, marked in flight
   * until it settles. A create names its kind's `pending` key
   * (`rowKey(kind, NEW)`), since its id isn't known yet.
   */
  async write<T>(keys: readonly string[], run: () => Promise<T>): Promise<T> {
    const promise = run();
    for (const key of keys) {
      let set = this.inflight.get(key);
      if (!set) this.inflight.set(key, (set = new Set()));
      set.add(promise);
    }
    try {
      return await promise;
    } finally {
      for (const key of keys) {
        const set = this.inflight.get(key);
        set?.delete(promise);
        if (set?.size === 0) this.inflight.delete(key);
      }
    }
  }

  /** Whether any write for a key starting with `prefix` is in flight. */
  busy(prefix: string): boolean {
    for (const key of this.inflight.keys()) if (key.startsWith(prefix)) return true;
    return false;
  }

  /**
   * Resolves once every write in flight now for a key starting with
   * `prefix` has settled (failed ones too). Writes started later aren't
   * waited for.
   */
  async settled(prefix: string): Promise<void> {
    const waiting: Promise<unknown>[] = [];
    for (const [key, set] of this.inflight) {
      if (key.startsWith(prefix)) waiting.push(...set);
    }
    await Promise.allSettled(waiting);
  }
}

/** The key a create uses while its id is unknown: `rowKey(kind, NEW)`. */
export const NEW = "+new";
