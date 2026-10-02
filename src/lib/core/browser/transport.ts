/**
 * `BrowserCore`: the page's one Core (phase 8, Decisions 2, 6, 15 and 16),
 * over the browser module's exports (`crates/seaquel-browser`).
 *
 * - **Calls and streams.** `call` sends a request's bytes and resolves with
 *   the `CoreResponse` JSON; a refusal rejects with a `CoreCallError`.
 *   `stream` hands each `CoreEvent` to its handler and resolves with the
 *   count once the stream has ended and every event was delivered.
 * - **No re-entrancy** (Decision 15). The module calls `onEvent` while it is
 *   mid-call, and a call back into it from there would hit wasm-bindgen's
 *   borrow checks or a `RefCell` Core holds. So events, stream items and a
 *   stream's end go through one mailbox and reach TypeScript in a later
 *   microtask, in order; nothing here calls the module from a callback the
 *   module made.
 * - **The snapshot** (Decision 6). After a call or stream ends, if the
 *   commit counter moved past what's stored, one macrotask later (so writes
 *   in a burst share one) the file is serialized and saved. The counter is
 *   read right before the snapshot, synchronously, so the snapshot holds
 *   every commit it counted. One save is in flight at a time; a later one
 *   waits for it, then takes a fresh snapshot, so the newest always lands
 *   last. A snapshot refused because a call holds the file is retried when
 *   that call ends. `flushNow` (on `pagehide`, or the page going hidden)
 *   saves at once even while a save is in flight: the store applies saves
 *   in call order.
 * - **A trap** (Decision 16). A `WebAssembly.RuntimeError` from any export,
 *   or the module's panic hook (a panic inside an async call leaves its
 *   promise pending forever), restarts it: every call in flight fails with
 *   `CORE_RESTARTED`, each stream ends with that error, the dead instance's
 *   DuckDB connections are closed, and a fresh instance of the same
 *   compiled module opens on the last stored snapshot. Calls made meanwhile
 *   wait for it. `onRestarted` handlers then run (the client fires
 *   `onResubscribed` and marks its connections closed).
 */
import { CoreCallError } from "$lib/storage/rust-client";
import type { CoreEvent } from "$lib/types/generated/CoreEvent";
import type { RpcError } from "$lib/types/generated/RpcError";
import { log } from "$lib/utils/logger";
import type { DuckDbBridge } from "./duckdb-bridge";
import type { SnapshotStore } from "./snapshot-store";

/** The module's exports (`src/lib/wasm/browser-pkg/seaquel_browser.js`). */
export interface BrowserModule {
  open(bridge: DuckDbBridge, image: Uint8Array | undefined, onTrap: () => void): Promise<number>;
  call(body: Uint8Array): Promise<string>;
  stream(body: Uint8Array, onEvent: (json: string) => void): Promise<number>;
  events(onEvent: (json: string) => void): number;
  unsubscribe(id: number): void;
  snapshot(): Uint8Array;
  commits(): number;
  ensureDemoConnection(): Promise<string>;
  /** Added by `scripts/build-wasm.mjs`: a fresh instance of the same compiled module. */
  __seaquel_reinstantiate(): unknown;
  /** Added by `scripts/build-wasm.mjs`: a trap the live instance threw through its closures. */
  __seaquel_isCurrentTrap(error: unknown): boolean;
}

/**
 * Every export `BrowserModule` names, with its arity, checked against the
 * built module by `browser-core.test.ts`; `satisfies` makes `npm run check`
 * fail when the interface gains or loses one without this list.
 */
export const BROWSER_MODULE_EXPORTS = {
  open: 3,
  call: 1,
  stream: 2,
  events: 1,
  unsubscribe: 1,
  snapshot: 0,
  commits: 0,
  ensureDemoConnection: 0,
  __seaquel_reinstantiate: 0,
  __seaquel_isCurrentTrap: 1,
} as const satisfies Record<keyof BrowserModule, number>;

/** A call or stream that was in flight when the module trapped. */
export const CORE_RESTARTED = "CORE_RESTARTED";
/** The module trapped too often to keep restarting it: every call fails with this. */
export const CORE_FAILED = "CORE_FAILED";
/** The stored snapshot didn't open (or made Core trap); it was kept aside and Core started empty. */
export const STORAGE_CORRUPT = "STORAGE_CORRUPT";

/** At most this many restarts in `RESTART_WINDOW_MS`; one more and Core gives up. */
export const MAX_RESTARTS = 3;
export const RESTART_WINDOW_MS = 60_000;

const RESTARTED_MESSAGE = "Seaquel restarted after an internal error. Run it again.";
const FAILED_MESSAGE =
  "Seaquel stopped after repeated internal errors. Reload the page to start it again.";

/** Something the page should say once (`STORAGE_CORRUPT`, `STORAGE_UNAVAILABLE`). */
export interface BrowserNotice {
  code: string;
  message: string;
}

/** The module trapped: in an open, or outside a call. */
class TrapError extends Error {}

export interface BrowserCoreOptions {
  module: BrowserModule;
  bridge: DuckDbBridge & { closeAll?(): Promise<void> };
  /** `null`: run in memory, nothing kept. */
  store: SnapshotStore | null;
  /** The stored snapshot to open, or `null` for a new file. */
  image: Uint8Array | null;
  /**
   * How long a restart waits for saves in flight before it reopens on the
   * newest that landed (default `SAVE_WAIT_MS`): a save the store never
   * answers mustn't hold Core down.
   */
  saveWaitMs?: number;
}

/** A restart's wait for saves in flight. */
export const SAVE_WAIT_MS = 3_000;

/** The module's refusal: an `RpcError`'s JSON text. Anything else it throws is a trap. */
function refusal(error: unknown): RpcError | null {
  if (typeof error !== "string") return null;
  try {
    const parsed = JSON.parse(error) as RpcError;
    if (typeof parsed?.code === "string" && typeof parsed.message === "string") return parsed;
  } catch {
    // not JSON: fall through
  }
  return { code: "UNKNOWN", message: error };
}

function restartedError(): CoreCallError {
  return new CoreCallError({ code: CORE_RESTARTED, message: RESTARTED_MESSAGE });
}

function failedError(): CoreCallError {
  return new CoreCallError({ code: CORE_FAILED, message: FAILED_MESSAGE });
}

/** Delivers queued work in order, in a microtask, never inside a module call. */
class Mailbox {
  private readonly queue: Array<() => void> = [];
  private scheduled = false;

  post(work: () => void): void {
    this.queue.push(work);
    if (this.scheduled) return;
    this.scheduled = true;
    queueMicrotask(() => this.drain());
  }

  private drain(): void {
    this.scheduled = false;
    for (let work = this.queue.shift(); work; work = this.queue.shift()) {
      try {
        work();
      } catch (error) {
        void log.error("A Core event handler threw:", error);
      }
    }
  }
}

interface InFlight {
  fail(error: Error): void;
}

export class BrowserCore {
  private readonly mailbox = new Mailbox();
  private readonly inFlight = new Set<InFlight>();
  private readonly eventHandlers = new Set<(event: CoreEvent) => void>();
  private readonly restartHandlers = new Set<() => void>();
  private subscription: number | null = null;
  private restarting: Promise<void> | null = null;
  /** Moves at each restart, so a trap is acted on once per instance. */
  private generation = 0;
  private closed = false;
  /** Past the restart cap: every call fails with `CORE_FAILED`. */
  private fatal = false;
  /** When each recent restart happened (`MAX_RESTARTS` per `RESTART_WINDOW_MS`). */
  private readonly restartTimes: number[] = [];
  /** An open is running: a trap now fails that open instead of restarting. */
  private openTrapped: (() => void) | null = null;
  /** What the page should say once: a snapshot kept aside. */
  readonly notices: BrowserNotice[] = [];

  /** The counter as of the newest snapshot saved (or the open's baseline). */
  private storedCommits = 0;
  /** The newest snapshot saved, the image a restart reopens. */
  private lastImage: Uint8Array | null;
  /** Saves are numbered as they're taken; `lastImage` is the highest that landed. */
  private saveSeq = 0;
  private lastImageSeq = 0;
  private snapshotTimer: ReturnType<typeof setTimeout> | null = null;
  private saving: Promise<void> | null = null;
  private saveAgain = false;
  /** Every save started and not yet settled, for `settled()`. */
  private readonly saves = new Set<Promise<void>>();

  private constructor(private readonly options: BrowserCoreOptions) {
    this.lastImage = options.image;
  }

  /**
   * Opens the module on `options.image`. An image that doesn't open
   * (`STORAGE_CORRUPT`) or makes the module trap is moved aside and Core
   * starts empty, with a notice. Rejects with a `CoreCallError`:
   * `CORE_FAILED` past the restart cap, or a refusal.
   */
  static async open(options: BrowserCoreOptions): Promise<BrowserCore> {
    const core = new BrowserCore(options);
    try {
      await core.openRecovering(options.image);
    } catch (error) {
      if (error instanceof TrapError) {
        core.fatal = true;
        throw failedError();
      }
      throw error;
    }
    return core;
  }

  private get module(): BrowserModule {
    return this.options.module;
  }

  private readonly onTrap = () => {
    // Called from inside the module's panic hook. During an open, that
    // open fails (its promise would never settle). Otherwise restart once
    // the module has unwound, never from here, and only if a synchronous
    // caller didn't already see the same trap and restart.
    if (this.openTrapped) {
      this.openTrapped();
      return;
    }
    const generation = this.generation;
    setTimeout(() => {
      if (generation === this.generation) void this.trapped(new Error("the module panicked"));
    }, 0);
  };

  /**
   * A `WebAssembly.RuntimeError` the page saw outside any call (a trap
   * inside an async export's task, e.g. a stack overflow, which the panic
   * hook doesn't see). `openBrowserCore` routes the window's `error` and
   * `unhandledrejection` events here. Returns whether it was a trap.
   */
  noticeTrap(error: unknown): boolean {
    if (!(error instanceof WebAssembly.RuntimeError)) return false;
    // Only a trap of this module's live instance: the glue records each one
    // thrown through its closures (`__seaquel_isCurrentTrap`), so this holds
    // in WebKit too, whose stacks name no module. Another module's trap (the
    // editor module recovers by itself; DuckDB's runs in its worker), and
    // one from an instance already restarted from, isn't ours to restart.
    let ours = false;
    try {
      ours = this.module.__seaquel_isCurrentTrap(error);
    } catch {
      ours = false;
    }
    // A trap recorded under an older generation (already restarted from)
    // isn't ours either: it isn't `preventDefault`ed, so it still shows as
    // uncaught in the console. That is cosmetic and on purpose; treating it
    // as ours would restart Core a second time for one trap.
    if (!ours) return false;
    const generation = this.generation;
    setTimeout(() => {
      if (generation === this.generation) void this.trapped(error);
    }, 0);
    return true;
  }

  /** Whether one more restart fits under the cap; records it if so. */
  private mayRestart(): boolean {
    const now = Date.now();
    while (this.restartTimes.length && now - this.restartTimes[0] > RESTART_WINDOW_MS) {
      this.restartTimes.shift();
    }
    if (this.restartTimes.length >= MAX_RESTARTS) return false;
    this.restartTimes.push(now);
    return true;
  }

  /** A fresh instance of the module, its old closures neutered by the glue. */
  private async reinstantiate(): Promise<void> {
    this.generation += 1;
    this.module.__seaquel_reinstantiate();
    // The dead instance's DuckDB connections: nothing will close them.
    await this.options.bridge.closeAll?.().catch(() => {});
  }

  /**
   * Opens on `image`, recovering what can be: an image that's
   * `STORAGE_CORRUPT` or makes the open trap is moved aside and the open
   * retried on a new file; a trap with no image is retried on a fresh
   * instance. Each fresh instance counts toward the cap; past it, a
   * `TrapError`.
   */
  private async openRecovering(image: Uint8Array | null): Promise<void> {
    for (;;) {
      try {
        await this.openInstance(image);
        this.lastImage = image;
        return;
      } catch (error) {
        if (error instanceof TrapError) {
          if (!this.mayRestart()) throw error;
          await this.reinstantiate();
        } else if (!(image && error instanceof CoreCallError && error.code === STORAGE_CORRUPT)) {
          throw error;
        }
        if (image) {
          await this.keepAside();
          image = null;
        }
      }
    }
  }

  /** Moves the stored snapshot aside (never lost) and says so once. */
  private async keepAside(): Promise<void> {
    await this.options.store?.moveAside().catch((error: unknown) => {
      void log.warn("Moving the unreadable demo data aside failed:", errorName(error));
    });
    if (!this.notices.some((n) => n.code === STORAGE_CORRUPT)) {
      this.notices.push({
        code: STORAGE_CORRUPT,
        message:
          "The demo's saved data couldn't be read, so it started fresh. The old data was kept aside.",
      });
    }
  }

  /**
   * One `open`, raced against the panic hook: a panic inside the open's
   * task leaves its promise pending forever, so the hook fails it instead
   * (`TrapError`). A refusal is a `CoreCallError`.
   */
  private async openInstance(image: Uint8Array | null): Promise<void> {
    let fire: () => void = () => {};
    const trapped = new Promise<never>((_, reject) => {
      fire = () => reject(new TrapError("the module panicked while opening"));
    });
    trapped.catch(() => {});
    this.openTrapped = fire;
    let commits: number;
    try {
      commits = await Promise.race([
        (async () => this.module.open(this.options.bridge, image ?? undefined, this.onTrap))(),
        trapped,
      ]);
    } catch (error) {
      if (error instanceof TrapError) throw error;
      const rpc = refusal(error);
      if (rpc) throw new CoreCallError(rpc);
      throw new TrapError(errorName(error));
    } finally {
      this.openTrapped = null;
    }
    // The open's own commits (baseline, migrations, data steps) are kept
    // with the next change; until then the next start repeats them.
    this.storedCommits = commits;
    this.subscription = null;
    if (this.eventHandlers.size > 0) this.subscribe0();
  }

  private subscribe0(): void {
    if (this.subscription !== null) return;
    this.subscription = this.module.events((json) => {
      const event = JSON.parse(json) as CoreEvent;
      this.mailbox.post(() => {
        for (const handler of this.eventHandlers) handler(event);
      });
    });
  }

  /** Workspace events (`connectionClosed`, `storageChanged`). Returns the unsubscribe. */
  subscribe(handler: (event: CoreEvent) => void): () => void {
    this.eventHandlers.add(handler);
    if (!this.restarting) {
      try {
        this.subscribe0();
      } catch (error) {
        this.caught(error);
      }
    }
    return () => {
      this.eventHandlers.delete(handler);
    };
  }

  /** Runs after each restart, once the new instance is open. */
  onRestarted(handler: () => void): () => void {
    this.restartHandlers.add(handler);
    return () => {
      this.restartHandlers.delete(handler);
    };
  }

  /** One call: the `CoreResponse` JSON text. */
  call(body: Uint8Array): Promise<string> {
    return this.run((m) => m.call(body));
  }

  /** `ensureDemoConnection` (Decision 19): the row as `Seqd` JSON. */
  async ensureDemoConnection(): Promise<unknown> {
    return JSON.parse(await this.run((m) => m.ensureDemoConnection()));
  }

  /**
   * One stream: each event to `onEvent` in a later microtask, in order.
   * Resolves once every event was delivered; rejects with a `CoreCallError`
   * (a refused request, or `CORE_RESTARTED`).
   */
  stream(body: Uint8Array, onEvent: (event: CoreEvent) => void): Promise<number> {
    return this.run(
      (m) =>
        m.stream(body, (json) => {
          const event = JSON.parse(json) as CoreEvent;
          this.mailbox.post(() => onEvent(event));
        }),
      // The stream's end goes through the mailbox too, after its events.
      (count) => new Promise<number>((resolve) => this.mailbox.post(() => resolve(count))),
    );
  }

  /**
   * Calls one export under the trap guard: a refusal becomes a
   * `CoreCallError`, a trap restarts the module and fails the call with
   * `CORE_RESTARTED`. Waits for a restart in progress first.
   */
  async run<T>(
    fn: (m: BrowserModule) => Promise<T> | T,
    then: (value: T) => Promise<T> | T = (value) => value,
  ): Promise<T> {
    if (this.closed) throw new CoreCallError({ code: "NOT_OPEN", message: "Core was closed" });
    if (this.restarting) await this.restarting;
    if (this.fatal) throw failedError();
    return new Promise<T>((resolve, reject) => {
      let settled = false;
      const entry: InFlight = {
        fail: (error) => {
          if (settled) return;
          settled = true;
          reject(error);
        },
      };
      const finish = (ok: boolean, value: unknown) => {
        if (settled) return;
        settled = true;
        this.inFlight.delete(entry);
        if (ok) {
          Promise.resolve(then(value as T)).then(resolve, reject);
          this.scheduleSnapshot();
        } else {
          reject(value);
        }
      };
      this.inFlight.add(entry);
      let pending: Promise<T> | T;
      try {
        pending = fn(this.module);
      } catch (error) {
        this.inFlight.delete(entry);
        this.failWith(error, entry, finish);
        return;
      }
      Promise.resolve(pending).then(
        (value) => finish(true, value),
        (error: unknown) => this.failWith(error, entry, finish),
      );
    });
  }

  private failWith(
    error: unknown,
    entry: InFlight,
    finish: (ok: boolean, value: unknown) => void,
  ): void {
    const rpc = refusal(error);
    if (rpc) {
      finish(false, new CoreCallError(rpc));
      // A refused write may still have committed nothing; a refused stream
      // may have run statements that did. Check either way.
      this.scheduleSnapshot();
      return;
    }
    entry.fail(restartedError());
    this.inFlight.delete(entry);
    void this.trapped(error);
  }

  /** Something the module threw outside `run` (a synchronous export). */
  private caught(error: unknown): void {
    const rpc = refusal(error);
    if (rpc) {
      void log.warn("A Core call was refused:", rpc.code);
      return;
    }
    void this.trapped(error);
  }

  /** Restarts the module once per trap. Never rejects: past the cap it goes fatal. */
  private trapped(error: unknown): Promise<void> {
    if (this.restarting) return this.restarting;
    if (this.closed || this.fatal) return Promise.resolve();
    this.restarting = this.restart(error).finally(() => {
      this.restarting = null;
    });
    return this.restarting;
  }

  private async restart(error: unknown): Promise<void> {
    // At once, so the panic hook's own scheduled restart sees it's handled
    // and so saves still in flight belong to the old generation.
    this.generation += 1;
    void log.error(
      "Seaquel's core stopped; restarting it from the last saved state:",
      errorName(error),
    );
    for (const entry of this.inFlight) entry.fail(restartedError());
    this.inFlight.clear();
    if (this.snapshotTimer !== null) clearTimeout(this.snapshotTimer);
    this.snapshotTimer = null;
    this.saveAgain = false;
    // Saves in flight finish first: the newest that landed is the image.
    // One the store never answers is given up on after a few seconds; if it
    // lands later, its counter is the dead instance's and moves nothing.
    let timer: ReturnType<typeof setTimeout> | undefined;
    const finished = await Promise.race([
      Promise.allSettled(this.saves).then(() => true),
      new Promise<boolean>((resolve) => {
        timer = setTimeout(() => resolve(false), this.options.saveWaitMs ?? SAVE_WAIT_MS);
      }),
    ]);
    clearTimeout(timer);
    if (!finished) {
      void log.warn("A snapshot save didn't finish; restarting from the newest one that did");
      // The new instance saves without waiting on the stuck one (the store
      // still applies saves in call order).
      this.saving = null;
    }
    try {
      if (!this.mayRestart()) throw new TrapError("too many restarts");
      await this.reinstantiate();
      await this.openRecovering(this.lastImage);
    } catch (failure) {
      this.fatal = true;
      void log.error("Seaquel's core can't be restarted; giving up:", errorName(failure));
      for (const entry of this.inFlight) entry.fail(failedError());
      this.inFlight.clear();
      return;
    }
    for (const handler of this.restartHandlers) this.mailbox.post(handler);
  }

  // -------- The snapshot --------

  private scheduleSnapshot(): void {
    if (this.closed || this.snapshotTimer !== null) return;
    this.snapshotTimer = setTimeout(() => {
      this.snapshotTimer = null;
      this.flush();
    }, 0);
  }

  /** Takes and saves a snapshot if anything committed since the last one. */
  private flush(): void {
    if (this.saving) {
      this.saveAgain = true;
      return;
    }
    const taken = this.take();
    if (!taken) return;
    const saving: Promise<void> = this.save(taken.commits, taken.bytes).finally(() => {
      // A save a restart gave up on may land after a newer one started.
      if (this.saving !== saving) return;
      this.saving = null;
      if (this.saveAgain) {
        this.saveAgain = false;
        this.flush();
      }
    });
    this.saving = saving;
  }

  /** The counter and the file, read together; `null` if nothing new or busy. */
  private take(): { commits: number; bytes: Uint8Array } | null {
    if (this.restarting || this.closed || this.fatal) return null;
    try {
      const commits = this.module.commits();
      if (commits <= this.storedCommits) return null;
      return { commits, bytes: this.module.snapshot() };
    } catch (error) {
      // Busy (a call holds the file): that call's end schedules another.
      if (refusal(error)) return null;
      void this.trapped(error);
      return null;
    }
  }

  /**
   * Saves one snapshot. The counter it records is the instance's that took
   * it: a save from before a restart never moves the new instance's
   * counter (whose numbering starts again). The image it leaves for a
   * restart is the newest that landed, whichever instance took it.
   */
  private save(commits: number, bytes: Uint8Array): Promise<void> {
    const store = this.options.store;
    const generation = this.generation;
    const seq = ++this.saveSeq;
    const work = (store ? store.save(bytes) : Promise.resolve()).then(
      () => {
        if (generation === this.generation && commits > this.storedCommits) {
          this.storedCommits = commits;
        }
        if (seq > this.lastImageSeq) {
          this.lastImageSeq = seq;
          this.lastImage = bytes;
        }
      },
      (error: unknown) => {
        // Kept for the next change; nothing else to do.
        void log.warn("Saving the demo's data failed:", error instanceof Error ? error.name : "");
      },
    );
    this.saves.add(work);
    void work.finally(() => this.saves.delete(work));
    return work;
  }

  /**
   * Saves now (the page is going away): a snapshot taken at once and saved
   * even while another save is in flight (the store keeps call order).
   */
  flushNow(): void {
    if (this.snapshotTimer !== null) clearTimeout(this.snapshotTimer);
    this.snapshotTimer = null;
    const taken = this.take();
    if (taken) void this.save(taken.commits, taken.bytes);
  }

  /** Resolves once no snapshot is due, being taken or being saved (tests). */
  async settled(): Promise<void> {
    for (;;) {
      if (this.restarting) {
        await this.restarting.catch(() => {});
        continue;
      }
      if (this.snapshotTimer !== null) {
        await new Promise((resolve) => setTimeout(resolve, 0));
        continue;
      }
      const pending = [...this.saves, ...(this.saving ? [this.saving] : [])];
      if (pending.length === 0) {
        // Let the mailbox deliver what's queued.
        await new Promise((resolve) => setTimeout(resolve, 0));
        if (this.snapshotTimer === null && this.saves.size === 0 && !this.saving) return;
        continue;
      }
      await Promise.race(pending.map((p) => p.catch(() => {})));
    }
  }

  /** Stops snapshotting and events. The module stays as it is. */
  close(): void {
    this.closed = true;
    if (this.snapshotTimer !== null) clearTimeout(this.snapshotTimer);
    this.snapshotTimer = null;
    if (this.subscription !== null) {
      try {
        this.module.unsubscribe(this.subscription);
      } catch {
        // a trapped instance; nothing to unsubscribe
      }
    }
    this.subscription = null;
    this.eventHandlers.clear();
    this.restartHandlers.clear();
  }
}

function errorName(error: unknown): string {
  return error instanceof Error ? error.name : typeof error;
}
