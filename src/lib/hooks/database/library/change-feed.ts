/**
 * `ChangeFeed`: other windows' and tabs' stored writes, as refetch requests
 * (phase 5d-1, Decision 18).
 *
 * Core emits one `storageChanged` event per committed write, with its kind,
 * scope, ids, the writer's origin and the change `seq`, never a value. The
 * feed:
 * - skips an event carrying this page's own origin: its write's answer
 *   already updated the page (echo suppression);
 * - groups the rest per kind and scope for 100 ms, then hands each group to
 *   the kind's subscribers as one change (the ids joined, or `null` when any
 *   event asked for a reload of the kind), which refetch and apply it by the
 *   `seq` rule (`RowSeqs`);
 * - asks for every list to be reloaded when events may have been missed:
 *   each time the event channel (re)starts (`onResubscribed`, `initial` the
 *   first time), and when a result or an event shows a new epoch;
 * - asks for the same reload when another process wrote the file (phase 7a,
 *   Decision 6: an `external` event, which names no rows), once per 100 ms;
 * - says when updates stopped (`onEventsUnavailable`) until the channel is
 *   back.
 *
 * It subscribes to `onResubscribed` before `events`, and `start()` must run
 * before the page's first `*List` call, so nothing between the two is lost.
 */
import type { CoreClient, EventsUnavailableReason, StorageChangedEvent } from "$lib/core/client";
import type { RowSeqs } from "./seqs";
import type { ChangeSeq, StoredKind } from "./types";

/** One group of other windows' writes to refetch. */
export interface StorageChange {
  kind: StoredKind;
  /** The project, connection or chat the ids belong to, if the writer said. */
  scope: string | null;
  /** The rows to refetch, or `null` for every row of the kind in the scope. */
  ids: string[] | null;
  /** The highest `seq` among the grouped events. */
  seq: ChangeSeq;
}

/** Why every list should be reloaded. */
export type ReloadReason =
  | { reason: "resubscribed"; initial: boolean }
  | { reason: "epoch" }
  | { reason: "external" };

export interface ChangeFeedOptions {
  client: () => CoreClient;
  /** This page's origin (`pageOrigin()`); `null` when it has none. */
  origin: () => string | null;
  seqs: RowSeqs;
  /**
   * An event carrying this page's own origin that should still be heard:
   * a write Core made for this page after its answer (a stopped turn's
   * reply, stored after the stream ended). Others are skipped.
   */
  acceptOwn?: (event: StorageChangedEvent) => boolean;
  /** How long events of one kind and scope are grouped (Decision 18: 100 ms). */
  delayMs?: number;
}

interface Group {
  kind: StoredKind;
  scope: string | null;
  ids: Set<string> | null;
  seq: ChangeSeq;
  timer: ReturnType<typeof setTimeout>;
}

export class ChangeFeed {
  private readonly handlers = new Map<StoredKind, Set<(change: StorageChange) => void>>();
  private readonly reloadHandlers = new Set<(reason: ReloadReason) => void>();
  private readonly statusHandlers = new Set<(reason: EventsUnavailableReason | null) => void>();
  private readonly groups = new Map<string, Group>();
  /** The grouped `external` reload waiting to be asked for. */
  private externalTimer: ReturnType<typeof setTimeout> | null = null;
  private stops: Array<() => void> = [];
  private _unavailable: EventsUnavailableReason | null = null;
  private readonly delayMs: number;

  constructor(private readonly options: ChangeFeedOptions) {
    this.delayMs = options.delayMs ?? 100;
  }

  /** Why updates stopped arriving, or `null` while they arrive. */
  get unavailable(): EventsUnavailableReason | null {
    return this._unavailable;
  }

  /** Subscribe to the page's events. Call once, before the first list. */
  start(): void {
    if (this.stops.length > 0) return;
    const client = this.options.client();
    // A new epoch in a write's answer or a list reloads everything too.
    this.stops.push(this.options.seqs.onNewEpoch(() => this.newEpoch()));
    // Before `events`, so the channel's first start isn't missed.
    this.stops.push(
      client.onResubscribed(({ initial }) => {
        this.setUnavailable(null);
        this.emitReload({ reason: "resubscribed", initial });
      }),
    );
    this.stops.push(client.onEventsUnavailable((reason) => this.setUnavailable(reason)));
    this.stops.push(
      client.events((event) => {
        if (event.type === "storageChanged") this.receive(event);
      }),
    );
  }

  /** Unsubscribe from the page's events and drop what is grouped. */
  stop(): void {
    for (const stop of this.stops.splice(0)) stop();
    this.dropGroups();
  }

  /** Hear other windows' changes of `kind`. Returns the unsubscribe. */
  subscribe(kind: StoredKind, handler: (change: StorageChange) => void): () => void {
    let set = this.handlers.get(kind);
    if (!set) this.handlers.set(kind, (set = new Set()));
    set.add(handler);
    return () => set.delete(handler);
  }

  /** Hear when every list should be reloaded. Returns the unsubscribe. */
  onReload(handler: (reason: ReloadReason) => void): () => void {
    this.reloadHandlers.add(handler);
    return () => this.reloadHandlers.delete(handler);
  }

  /** Hear when updates stop (a reason) and come back (`null`). Returns the unsubscribe. */
  onStatus(handler: (reason: EventsUnavailableReason | null) => void): () => void {
    this.statusHandlers.add(handler);
    return () => this.statusHandlers.delete(handler);
  }

  private receive(event: StorageChangedEvent): void {
    // A new epoch reloads everything (through `onNewEpoch`), which covers
    // whatever this event names; one from a replaced epoch is ignored.
    if (this.options.seqs.observe(event.seq) !== "current") return;
    const origin = this.options.origin();
    if (origin !== null && event.origin === origin && !this.options.acceptOwn?.(event)) return;
    if (event.kind === "external") {
      this.externalTimer ??= setTimeout(() => {
        this.externalTimer = null;
        this.emitReload({ reason: "external" });
      }, this.delayMs);
      return;
    }
    if (!this.handlers.get(event.kind)?.size) return;

    const key = `${event.kind}\u0001${event.scope ?? ""}`;
    const group = this.groups.get(key);
    if (group) {
      if (group.ids === null || event.ids === null) group.ids = null;
      else for (const id of event.ids) group.ids.add(id);
      if (event.seq.n > group.seq.n) group.seq = event.seq;
      return;
    }
    this.groups.set(key, {
      kind: event.kind,
      scope: event.scope,
      ids: event.ids === null ? null : new Set(event.ids),
      seq: event.seq,
      timer: setTimeout(() => this.flush(key), this.delayMs),
    });
  }

  private flush(key: string): void {
    const group = this.groups.get(key);
    if (!group) return;
    this.groups.delete(key);
    const change: StorageChange = {
      kind: group.kind,
      scope: group.scope,
      ids: group.ids === null ? null : [...group.ids],
      seq: group.seq,
    };
    for (const handler of Array.from(this.handlers.get(group.kind) ?? [])) handler(change);
  }

  private newEpoch(): void {
    this.dropGroups();
    this.emitReload({ reason: "epoch" });
  }

  private dropGroups(): void {
    for (const group of this.groups.values()) clearTimeout(group.timer);
    this.groups.clear();
    if (this.externalTimer !== null) clearTimeout(this.externalTimer);
    this.externalTimer = null;
  }

  private emitReload(reason: ReloadReason): void {
    for (const handler of Array.from(this.reloadHandlers)) handler(reason);
  }

  private setUnavailable(reason: EventsUnavailableReason | null): void {
    if (this._unavailable === reason) return;
    this._unavailable = reason;
    for (const handler of Array.from(this.statusHandlers)) handler(reason);
  }
}
