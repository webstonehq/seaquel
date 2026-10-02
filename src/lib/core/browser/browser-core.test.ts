/**
 * The browser module and its in-page transport (phase 8, Task 5), in Node:
 * the test build of `seaquel-browser` (`npm run wasm:build:browser-test`)
 * loaded from its bytes, over DuckDB-WASM's Node build. The snapshot store
 * and `localStorage` are fakes; IndexedDB itself is `snapshot-store.test.ts`.
 */
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { AsyncDuckDB } from "@duckdb/duckdb-wasm";
import type { CoreRequest } from "$lib/types/generated/CoreRequest";
import { VIEW_STATE_JOURNAL_KEY } from "$lib/storage/view-state-journal";
import type { AnyStreamEvent, CoreClient, StreamRequest, WorkspaceEvent } from "../client";
import { callDb } from "../client";
import { makeDuckDbBridge, type PageDuckDbBridge } from "./duckdb-bridge";
import { browserCoreClient } from "./client";
import { openBrowserCore, type OpenedBrowserCore, type OpenBrowserCoreOptions } from "./index";
import type { SnapshotStore } from "./snapshot-store";
import { BROWSER_MODULE_EXPORTS, type BrowserModule } from "./transport";
import { bootDuckDb, loadTestModule, testModuleMissing, type TestModule } from "./testing/node";

vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn() },
}));

const missing = testModuleMissing();
if (missing && process.env.CI) throw new Error(missing);

const LONG = "SELECT sum(range) FROM range(30000000000)";

/** A snapshot store that applies saves in the order they were made, as IndexedDB does. */
class FakeStore implements SnapshotStore {
  readonly files = new Map<string, Uint8Array>();
  readonly saved: Uint8Array[] = [];
  /** While set, each save waits for `release()`. */
  hold = false;
  private readonly gates: Array<() => void> = [];
  private chain: Promise<void> = Promise.resolve();
  savesStarted = 0;

  async load() {
    return this.files.get("meta.db") ?? null;
  }
  save(bytes: Uint8Array) {
    this.savesStarted += 1;
    const gate = this.hold
      ? new Promise<void>((resolve) => this.gates.push(resolve))
      : Promise.resolve();
    this.chain = this.chain
      .then(() => gate)
      .then(() => {
        this.files.set("meta.db", bytes);
        this.saved.push(bytes);
      });
    return this.chain;
  }
  async moveAside() {
    const file = this.files.get("meta.db");
    if (file) this.files.set("meta.db.unreadable", file);
    this.files.delete("meta.db");
  }
  /** Lets the oldest held save finish. */
  release() {
    this.gates.shift()?.();
  }
  get waiting() {
    return this.gates.length;
  }
}

class FakeLocalStorage implements Storage {
  private readonly map = new Map<string, string>();
  readonly reads: string[] = [];
  get length() {
    return this.map.size;
  }
  clear() {
    this.map.clear();
  }
  getItem(key: string) {
    this.reads.push(key);
    return this.map.get(key) ?? null;
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  setItem(key: string, value: string) {
    this.map.set(key, String(value));
  }
  keys() {
    return [...this.map.keys()];
  }
}

class FakeWindow {
  readonly listeners = new Map<string, Set<() => void>>();
  addEventListener(type: string, fn: () => void) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(fn);
  }
  removeEventListener(type: string, fn: () => void) {
    this.listeners.get(type)?.delete(fn);
  }
  fire(type: string) {
    for (const fn of this.listeners.get(type) ?? []) fn();
  }
}

/** Lets timers and IndexedDB-like promise chains run. */
const tick = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));

/**
 * Records whether the module is on the stack when a handler runs: inside an
 * export's synchronous part, or inside a callback the module is making
 * (`onEvent`, which it calls from its own task queue).
 */
function watched(module: TestModule): { module: BrowserModule; inside: () => boolean } {
  let depth = 0;
  const wrap =
    <A extends unknown[], R>(fn: (...args: A) => R) =>
    (...args: A): R => {
      depth += 1;
      try {
        return fn(...args);
      } finally {
        depth -= 1;
      }
    };
  const m = module;
  return {
    module: {
      open: wrap(m.open.bind(m)),
      call: wrap(m.call.bind(m)),
      stream: wrap((body: Uint8Array, onEvent: (json: string) => void) =>
        m.stream(body, wrap(onEvent)),
      ),
      events: wrap((onEvent: (json: string) => void) => m.events(wrap(onEvent))),
      unsubscribe: wrap(m.unsubscribe.bind(m)),
      snapshot: wrap(m.snapshot.bind(m)),
      commits: wrap(m.commits.bind(m)),
      ensureDemoConnection: wrap(m.ensureDemoConnection.bind(m)),
      __seaquel_reinstantiate: wrap(m.__seaquel_reinstantiate.bind(m)),
      __seaquel_isCurrentTrap: wrap(m.__seaquel_isCurrentTrap.bind(m)),
    },
    inside: () => depth > 0,
  };
}

describe.skipIf(missing !== null)("the browser module in the page", () => {
  let module: TestModule;
  let db: AsyncDuckDB;
  const log: string[] = [];
  let bridge: PageDuckDbBridge;

  beforeAll(async () => {
    module = (await loadTestModule())!;
    db = await bootDuckDb();
    bridge = makeDuckDbBridge(db, { note: (line) => log.push(line) });
  }, 60_000);

  // A test that fails half-way must not leave a counter that swallows a
  // later test's real trap or panics its open.
  afterEach(() => {
    const expected = globalThis.__seaquelExpectedTraps;
    const opens = globalThis.__seaquelTestPanicOpens;
    globalThis.__seaquelExpectedTraps = undefined;
    globalThis.__seaquelTestPanicOpens = undefined;
    expect(expected ?? 0, "__seaquelExpectedTraps left over").toBe(0);
    expect(opens ?? 0, "__seaquelTestPanicOpens left over").toBe(0);
  });

  afterAll(async () => {
    await db?.terminate();
  });

  async function start(
    overrides: Partial<OpenBrowserCoreOptions> = {},
  ): Promise<{ opened: OpenedBrowserCore; client: CoreClient; store: FakeStore }> {
    const store = (overrides.store as FakeStore | undefined) ?? new FakeStore();
    const opened = await openBrowserCore({
      module,
      bridge,
      store,
      localStorage: new FakeLocalStorage(),
      window: null,
      ...overrides,
    });
    return { opened, client: browserCoreClient(opened.core), store };
  }

  async function lib<T = unknown>(
    client: CoreClient,
    method: string,
    params?: unknown,
  ): Promise<T> {
    const inner = params === undefined ? { method } : { method, params };
    const response = (await client.call({ method: "library", params: inner } as CoreRequest)) as {
      result: { result: T };
    };
    return response.result.result;
  }

  async function connectDemo(client: CoreClient, opened: OpenedBrowserCore): Promise<string> {
    await opened.core.ensureDemoConnection();
    const { connectionId } = await callDb(client, "connect", {
      target: { type: "saved", id: "demo-connection" },
    } as never);
    return connectionId;
  }

  it("call_and_stream_round_trip", async () => {
    const { opened, client } = await start();
    const projects = await lib<{ value: { name: string }[] }>(client, "projectsList");
    expect(projects.value).toEqual([]);

    const connectionId = await connectDemo(client, opened);
    const connections = await lib<{ value: { id: string }[] }>(client, "connectionsList");
    expect(connections.value.map((c) => c.id)).toEqual(["demo-connection"]);

    const events: AnyStreamEvent[] = [];
    for await (const event of client.stream({
      method: "db",
      params: {
        method: "run",
        params: {
          streamId: "s1",
          connectionId,
          text: "SELECT range AS i FROM range(250) WHERE range >= {{min}}",
          target: { type: "all" },
          params: [{ name: "min", value: 10 }],
          pageSize: 100,
        },
      },
    } as unknown as StreamRequest)) {
      events.push(event);
    }
    const types = events.map((e) => e.type);
    expect(types[0]).toBe("statementStart");
    expect(types.at(-1)).toBe("done");
    const done = events.find((e) => e.type === "statementDone") as { totalRows: number };
    expect(done.totalRows).toBe(240);
    const rows = events
      .filter((e) => e.type === "batch")
      .flatMap((e) => (e as { rows: unknown[][] }).rows);
    expect(rows.length).toBe(100);
    expect(rows[0]).toEqual([10]);

    // A refused call is a CoreCallError with Core's code.
    await expect(lib(client, "projectRemove", { id: "nope" })).rejects.toMatchObject({
      code: "PROJECT_NOT_FOUND",
    });
    opened.close();
  });

  it("events_are_delivered_after_the_call_returns", async () => {
    const w = watched(module);
    const { opened, client } = await start({ module: w.module });
    const seen: { event: WorkspaceEvent; inside: boolean }[] = [];
    const followUps: Promise<unknown>[] = [];
    client.events((event) => {
      seen.push({ event, inside: w.inside() });
      // Calling back into the module from a handler must be safe.
      followUps.push(lib(client, "projectsList"));
    });
    const connectionId = await connectDemo(client, opened);
    await lib(client, "projectCreate", { project: { name: "Mine" } });
    await tick();
    expect(seen.length).toBeGreaterThanOrEqual(2);
    expect(seen.every((s) => !s.inside)).toBe(true);
    expect(seen.map((s) => s.event.type)).toContain("storageChanged");
    await Promise.all(followUps);

    // Stream events too: the consumer calls into the module for each one.
    let insideDuringStream = false;
    for await (const event of client.stream({
      method: "db",
      params: {
        method: "queryStream",
        params: { streamId: "s2", connectionId, sql: "SELECT range FROM range(20000)", params: [] },
      },
    } as unknown as StreamRequest)) {
      insideDuringStream ||= w.inside();
      await lib(client, "connectionsList");
      if (event.type === "done" || event.type === "error") break;
    }
    expect(insideDuringStream).toBe(false);
    opened.close();
  });

  it("aborting_a_stream_cancels_it_in_duckdb", async () => {
    const { opened, client } = await start();
    const connectionId = await connectDemo(client, opened);
    const controller = new AbortController();
    const before = log.length;
    const started = Date.now();
    setTimeout(() => controller.abort(), 300);
    const events: AnyStreamEvent[] = [];
    for await (const event of client.stream(
      {
        method: "db",
        params: {
          method: "queryStream",
          params: { streamId: "long", connectionId, sql: LONG, params: [] },
        },
      } as unknown as StreamRequest,
      { signal: controller.signal },
    )) {
      events.push(event);
    }
    expect(events.at(-1)).toMatchObject({ type: "error", code: "CANCELLED" });
    await tick(50);
    expect(log.slice(before).some((l) => l.startsWith("cancel"))).toBe(true);
    // DuckDB stopped: the next query on the connection answers at once.
    const next = Date.now();
    const result = await callDb(client, "query", {
      connectionId,
      sql: "SELECT 42 AS x",
      params: [],
    } as never);
    expect((result as { rows: unknown[][] }).rows).toEqual([[42]]);
    expect(Date.now() - next).toBeLessThan(2000);
    expect(Date.now() - started).toBeLessThan(5000);
    opened.close();
  });

  it("a_snapshot_is_stored_after_a_committing_call_and_not_otherwise", async () => {
    const { opened, client, store } = await start();
    await opened.core.settled();
    expect(store.savesStarted).toBe(0); // the open's own commits wait for a change

    await lib(client, "projectsList");
    await lib(client, "connectionsList");
    await opened.core.settled();
    expect(store.savesStarted).toBe(0);

    await lib(client, "projectCreate", { project: { name: "Kept" } });
    await lib(client, "projectCreate", { project: { name: "Kept too" } });
    await opened.core.settled();
    // Two writes in one macrotask: one snapshot holding both.
    expect(store.savesStarted).toBe(1);
    opened.close();

    // It reopens with both.
    const again = await start({ store });
    const names = (await lib<{ value: { name: string }[] }>(again.client, "projectsList")).value
      .map((p) => p.name)
      .sort();
    expect(names).toEqual(["Kept", "Kept too"]);
    again.opened.close();
  });

  it("the_newest_snapshot_wins_when_writes_overlap", async () => {
    const win = new FakeWindow();
    const store = new FakeStore();
    const { opened, client } = await start({ store, window: win as never });
    store.hold = true;

    await lib(client, "projectCreate", { project: { name: "A" } });
    await tick();
    expect(store.waiting).toBe(1); // A's snapshot is being written

    await lib(client, "projectCreate", { project: { name: "B" } });
    await tick();
    expect(store.waiting).toBe(1); // B waits for A's write, not beside it

    // The page goes away while A's write is in flight: C's snapshot is
    // written at once, queued behind A's.
    await lib(client, "projectCreate", { project: { name: "C" } });
    win.fire("pagehide");
    expect(store.waiting).toBe(2);

    store.hold = false;
    store.release();
    store.release();
    await opened.core.settled();
    while (store.waiting) {
      store.release();
      await opened.core.settled();
    }
    opened.close();

    const again = await start({ store });
    const names = (await lib<{ value: { name: string }[] }>(again.client, "projectsList")).value
      .map((p) => p.name)
      .sort();
    expect(names).toEqual(["A", "B", "C"]);
    again.opened.close();
  });

  it("the_old_keys_are_deleted_and_never_read", async () => {
    const storage = new FakeLocalStorage();
    storage.setItem("seaquel_db", "U1FMaXRlIGZvcm1hdCAz"); // the old file's base64
    storage.setItem("seaquel_db.backup", "x");
    storage.setItem("seaquel_dbx", "kept: not the old key");
    storage.setItem("seaquel-theme-cache", "{}");
    storage.setItem("PARAGLIDE_LOCALE", "en");
    const { opened, client } = await start({ localStorage: storage });
    expect(storage.keys().sort()).toEqual([
      "PARAGLIDE_LOCALE",
      "seaquel-theme-cache",
      "seaquel_dbx",
    ]);
    expect(storage.reads.filter((k) => k.startsWith("seaquel_db"))).toEqual([]);
    expect((await lib<{ value: unknown[] }>(client, "connectionsList")).value).toEqual([]);
    expect((await lib<{ value: unknown[] }>(client, "projectsList")).value).toEqual([]);
    opened.close();
  });

  it("an_unreadable_snapshot_is_kept_aside", async () => {
    const store = new FakeStore();
    const garbage = new TextEncoder().encode("not a database at all, just text ".repeat(200));
    store.files.set("meta.db", garbage);
    const { opened, client } = await start({ store });
    expect(opened.notices.map((n) => n.code)).toEqual(["STORAGE_CORRUPT"]);
    expect(store.files.get("meta.db.unreadable")).toEqual(garbage);
    expect(store.files.has("meta.db")).toBe(false);
    expect((await lib<{ value: unknown[] }>(client, "projectsList")).value).toEqual([]);
    // The new file is stored from the next write on.
    await lib(client, "projectCreate", { project: { name: "Fresh" } });
    await opened.core.settled();
    expect(store.files.has("meta.db")).toBe(true);
    opened.close();
  });

  it("indexeddb_unavailable_runs_in_memory_with_a_notice", async () => {
    const { opened, client } = await start({ store: undefined, indexedDB: null });
    expect(opened.notices.map((n) => n.code)).toEqual(["STORAGE_UNAVAILABLE"]);
    await lib(client, "projectCreate", { project: { name: "Not kept" } });
    await opened.core.settled();
    expect(
      (await lib<{ value: { name: string }[] }>(client, "projectsList")).value.map((p) => p.name),
    ).toEqual(["Not kept"]);
    opened.close();
  });

  it("a_trap_reopens_from_the_last_snapshot", async () => {
    const { opened, client, store } = await start();
    const resubscribed: boolean[] = [];
    client.onResubscribed(({ initial }) => resubscribed.push(initial));
    const closed: WorkspaceEvent[] = [];
    client.events((event) => {
      if (event.type === "connectionClosed") closed.push(event);
    });
    await tick();
    const connectionId = await connectDemo(client, opened);
    await lib(client, "projectCreate", { project: { name: "Stored" } });
    await opened.core.settled();
    expect(store.saved.length).toBeGreaterThan(0);

    // A stream in flight when the module traps ends with CORE_RESTARTED.
    const running = (async () => {
      const events: AnyStreamEvent[] = [];
      for await (const event of client.stream({
        method: "db",
        params: {
          method: "queryStream",
          params: { streamId: "doomed", connectionId, sql: LONG, params: [] },
        },
      } as unknown as StreamRequest)) {
        events.push(event);
      }
      return events;
    })();
    await tick(200);

    // A panic in a synchronous export.
    await expect(
      opened.core.run((m) => (m as TestModule).__test_trap("sync")),
    ).rejects.toMatchObject({
      code: "CORE_RESTARTED",
    });
    expect((await running).at(-1)).toMatchObject({ type: "error", code: "CORE_RESTARTED" });
    await opened.core.settled();
    expect(resubscribed).toEqual([true, false]);
    expect(closed).toEqual([
      expect.objectContaining({ type: "connectionClosed", connectionId, code: "CORE_RESTARTED" }),
    ]);
    // The reopened Core has what the last snapshot held, and works.
    const names = (await lib<{ value: { name: string }[] }>(client, "projectsList")).value.map(
      (p) => p.name,
    );
    expect(names).toContain("Stored");

    // A panic inside an async call, after an await: only the panic hook
    // can tell the page, and the call still fails instead of hanging. The
    // abort itself is uncaught in wasm-bindgen-futures' task queue; the
    // harness swallows exactly the one this test expects.
    globalThis.__seaquelExpectedTraps = 1;
    await expect(
      opened.core.run((m) => (m as TestModule).__test_trap("async")),
    ).rejects.toMatchObject({ code: "CORE_RESTARTED" });
    await opened.core.settled();
    expect(globalThis.__seaquelExpectedTraps).toBe(0);
    expect(resubscribed).toEqual([true, false, false]);
    const reconnected = await connectDemo(client, opened);
    const result = await callDb(client, "query", {
      connectionId: reconnected,
      sql: "SELECT 7 AS x",
      params: [],
    } as never);
    expect((result as { rows: unknown[][] }).rows).toEqual([[7]]);
    opened.close();
  });

  /** Waits until `check` holds, or fails after `ms`. */
  async function until(check: () => boolean, ms = 5000) {
    const end = Date.now() + ms;
    while (!check()) {
      if (Date.now() > end) throw new Error("timed out");
      await tick(10);
    }
  }

  it("a_save_in_flight_during_a_restart_is_neither_lost_nor_counted_against_the_new_instance", async () => {
    const win = new FakeWindow();
    const store = new FakeStore();
    const { opened, client } = await start({ store, window: win as never });
    store.hold = true;
    await lib(client, "projectCreate", { project: { name: "A" } });
    await until(() => store.waiting === 1); // A's snapshot is being written

    // The module traps while A's save is in flight.
    const trapped = expect(
      opened.core.run((m) => (m as TestModule).__test_trap("sync")),
    ).rejects.toMatchObject({ code: "CORE_RESTARTED" });
    await trapped;
    await tick(20);
    store.hold = false;
    store.release();
    await opened.core.settled();

    // The reopened instance has A (the restart waited for its save), and
    // its own counter isn't the old instance's.
    const names = (await lib<{ value: { name: string }[] }>(client, "projectsList")).value.map(
      (p) => p.name,
    );
    expect(names).toEqual(["A"]);
    await lib(client, "projectCreate", { project: { name: "B" } });
    win.fire("pagehide");
    await opened.core.settled();
    opened.close();

    const again = await start({ store });
    const kept = (await lib<{ value: { name: string }[] }>(again.client, "projectsList")).value
      .map((p) => p.name)
      .sort();
    expect(kept).toEqual(["A", "B"]);
    again.opened.close();
  });

  it("a_panic_during_the_first_open_restarts_it_instead_of_hanging", async () => {
    globalThis.__seaquelExpectedTraps = 1;
    globalThis.__seaquelTestPanicOpens = 1;
    const { opened, client } = await start();
    expect(globalThis.__seaquelTestPanicOpens).toBe(0);
    expect(globalThis.__seaquelExpectedTraps).toBe(0);
    expect(opened.notices).toEqual([]);
    expect((await lib<{ value: unknown[] }>(client, "projectsList")).value).toEqual([]);
    opened.close();
  });

  it("a_snapshot_that_makes_core_panic_is_kept_aside_and_core_starts_empty", async () => {
    const store = new FakeStore();
    const poison = new TextEncoder().encode("SEAQUEL_TEST_PANIC and then anything");
    store.files.set("meta.db", poison);
    globalThis.__seaquelExpectedTraps = 1;
    const { opened, client } = await start({ store });
    expect(globalThis.__seaquelExpectedTraps).toBe(0);
    expect(opened.notices.map((n) => n.code)).toEqual(["STORAGE_CORRUPT"]);
    expect(store.files.get("meta.db.unreadable")).toEqual(poison);
    expect(store.files.has("meta.db")).toBe(false);
    expect((await lib<{ value: unknown[] }>(client, "projectsList")).value).toEqual([]);
    opened.close();
  });

  it("past three restarts a minute every call fails with CORE_FAILED, never hangs", async () => {
    // Every open panics: the first start gives up after its restarts.
    globalThis.__seaquelExpectedTraps = 4;
    globalThis.__seaquelTestPanicOpens = 100;
    await expect(start()).rejects.toMatchObject({ code: "CORE_FAILED" });
    expect(globalThis.__seaquelExpectedTraps).toBe(0);
    globalThis.__seaquelTestPanicOpens = 0;

    // A running Core whose reopen keeps panicking: the trap's restart,
    // then the cap, then every call fails at once.
    globalThis.__seaquelTestPanicOpens = 0;
    // The instance the last open trapped in is dead; in the page, a
    // CORE_FAILED start is the end until a reload.
    module.__seaquel_reinstantiate();
    const { opened, client } = await start();
    globalThis.__seaquelExpectedTraps = 3;
    globalThis.__seaquelTestPanicOpens = 100;
    await expect(
      opened.core.run((m) => (m as TestModule).__test_trap("sync")),
    ).rejects.toMatchObject({ code: "CORE_RESTARTED" });
    await opened.core.settled();
    expect(globalThis.__seaquelExpectedTraps).toBe(0);
    globalThis.__seaquelTestPanicOpens = 0;
    await expect(lib(client, "projectsList")).rejects.toMatchObject({ code: "CORE_FAILED" });
    await expect(lib(client, "projectsList")).rejects.toMatchObject({ code: "CORE_FAILED" });
    opened.close();
    globalThis.__seaquelTestPanicOpens = 0;
    module.__seaquel_reinstantiate(); // for the tests after this one
  });

  it("a_snapshot_the_store_can_t_read_runs_in_memory_and_is_never_overwritten", async () => {
    const store = new FakeStore();
    store.load = async () => {
      throw new Error("DataError");
    };
    const { opened, client } = await start({ store });
    expect(opened.notices.map((n) => n.code)).toEqual(["STORAGE_UNAVAILABLE"]);
    await lib(client, "projectCreate", { project: { name: "Not stored" } });
    await opened.core.settled();
    expect(store.savesStarted).toBe(0);
    opened.close();
  });

  it("a_save_that_never_finishes_doesn_t_stop_a_restart", async () => {
    const win = new FakeWindow();
    const store = new FakeStore();
    const { opened, client } = await start({ store, window: win as never, saveWaitMs: 100 });
    // Saved writes first, so the dead instance's counter ends well past
    // where the new one's starts.
    for (const name of ["P1", "P2", "P3", "P4", "P5"]) {
      await lib(client, "projectCreate", { project: { name } });
      await opened.core.settled();
    }
    store.hold = true;
    await lib(client, "projectCreate", { project: { name: "A" } });
    await until(() => store.waiting === 1); // A's save is stuck

    await expect(
      opened.core.run((m) => (m as TestModule).__test_trap("sync")),
    ).rejects.toMatchObject({ code: "CORE_RESTARTED" });
    // The restart gives up waiting and reopens on what had landed (nothing):
    // calls work while A's save is still stuck.
    const [names, ms] = await (async () => {
      const t0 = Date.now();
      const r = await lib<{ value: { name: string }[] }>(client, "projectsList");
      return [r.value.map((p) => p.name), Date.now() - t0] as const;
    })();
    expect(names.sort()).toEqual(["P1", "P2", "P3", "P4", "P5"]);
    expect(ms).toBeLessThan(2000);
    await lib(client, "projectCreate", { project: { name: "B" } });

    // The stuck save lands late. Its counter is the dead instance's, so it
    // must not stop the new instance's next change from being saved.
    store.hold = false;
    while (store.waiting) store.release();
    await opened.core.settled();
    await lib(client, "projectCreate", { project: { name: "C" } });
    win.fire("pagehide");
    await opened.core.settled();
    opened.close();

    const again = await start({ store });
    const kept = (await lib<{ value: { name: string }[] }>(again.client, "projectsList")).value
      .map((p) => p.name)
      .sort();
    expect(kept).toEqual(["B", "C", "P1", "P2", "P3", "P4", "P5"]);
    again.opened.close();
  });

  it("a_snapshot_read_that_never_answers_runs_in_memory_and_writes_nothing", async () => {
    const store = new FakeStore();
    store.files.set("meta.db", new Uint8Array([1, 2, 3]));
    store.load = () => new Promise(() => {});
    const t0 = Date.now();
    const { opened, client } = await start({ store, loadTimeoutMs: 100 });
    expect(Date.now() - t0).toBeLessThan(5000);
    expect(opened.notices.map((n) => n.code)).toEqual(["STORAGE_UNAVAILABLE"]);
    await lib(client, "projectCreate", { project: { name: "Not stored" } });
    await opened.core.settled();
    expect(store.savesStarted).toBe(0);
    expect(store.files.get("meta.db")).toEqual(new Uint8Array([1, 2, 3]));
    opened.close();
  });

  it("a_lone_surrogate_in_a_call_reaches_core_well_formed", async () => {
    const { opened, client } = await start();
    const connectionId = await connectDemo(client, opened);
    await callDb(client, "execute", {
      connectionId,
      sql: "CREATE TABLE lone (v VARCHAR)",
      params: [],
    } as never);
    // A grid edit's value, as the probe typed it.
    await callDb(client, "execute", {
      connectionId,
      sql: "INSERT INTO lone VALUES (?)",
      params: ["a\ud800b"],
    } as never);
    const r = (await callDb(client, "query", {
      connectionId,
      sql: "SELECT v FROM lone",
      params: [],
    } as never)) as { rows: unknown[][] };
    expect(r.rows).toEqual([["a\ufffdb"]]);
    await lib(client, "projectCreate", { project: { name: "x\udc00" } });
    opened.close();
  });

  it("the_view_state_journal_is_replayed_after_the_open_and_only_a_newer_rev_is_kept", async () => {
    const store = new FakeStore();
    const view = (marker: string) => ({
      queryTabs: [],
      schemaTabs: [],
      explainTabs: [],
      erdTabs: [],
      tabOrder: [],
      activeView: "query",
      activeQueryTabId: marker,
    });
    const ui = async (client: CoreClient, method: string, params: unknown) =>
      (
        (await client.call({ method: "ui", params: { method, params } } as CoreRequest)) as {
          result: { result: { value: Json } };
        }
      ).result.result.value;
    type Json = Record<string, unknown>;
    const journal = (rev: number, marker: string) =>
      JSON.stringify({
        method: "ui",
        params: {
          method: "windowStateSave",
          params: { windowId: "demo", projectId: "default-seaquel", rev, state: view(marker) },
        },
      });

    const first = await start({ store });
    await lib(first.client, "projectEnsureDefault");
    await ui(first.client, "windowStateSave", {
      windowId: "demo",
      projectId: "default-seaquel",
      rev: 5,
      state: view("stored"),
    });
    await first.opened.core.settled();
    first.opened.close();

    // The page went away with a newer save only in the journal.
    const storage = new FakeLocalStorage();
    storage.setItem(VIEW_STATE_JOURNAL_KEY, journal(6, "journal"));
    const second = await start({ store, localStorage: storage });
    const loaded = await ui(second.client, "windowStateLoad", {
      windowId: "demo",
      projectId: "default-seaquel",
    });
    expect([loaded.rev, (loaded.state as Json).activeQueryTabId]).toEqual([6, "journal"]);
    expect(storage.getItem(VIEW_STATE_JOURNAL_KEY)).toBeNull();
    await second.opened.core.settled();
    second.opened.close();

    // An older journal (a stale tab) is replayed and refused as stale.
    storage.setItem(VIEW_STATE_JOURNAL_KEY, journal(4, "older"));
    const third = await start({ store, localStorage: storage });
    const kept = await ui(third.client, "windowStateLoad", {
      windowId: "demo",
      projectId: "default-seaquel",
    });
    expect([kept.rev, (kept.state as Json).activeQueryTabId]).toEqual([6, "journal"]);
    expect(storage.getItem(VIEW_STATE_JOURNAL_KEY)).toBeNull();
    third.opened.close();
  });

  it("noticeTrap restarts only for a trap in this module", async () => {
    const { opened } = await start();
    // Another module's trap (the editor module's, DuckDB's): never ours.
    expect(opened.core.noticeTrap(new WebAssembly.RuntimeError("unreachable"))).toBe(false);
    expect(opened.core.noticeTrap(new Error("seaquel_browser"))).toBe(false);
    opened.close();
  });

  it("noticeTrap attributes this module's trap without a URL in its stack (WebKit), once", async () => {
    const { opened, client } = await start();
    const resubscribed: boolean[] = [];
    client.onResubscribed(({ initial }) => resubscribed.push(initial));
    client.events(() => {});
    await tick();
    const noticed: boolean[] = [];
    let trapError: unknown = null;
    globalThis.__seaquelOnSwallowedTrap = (error) => {
      trapError = error;
      // WebKit's stack: wasm frames only, no module URL.
      (error as Error).stack = "@wasm-function[25006]\n@wasm-function[24921]";
      noticed.push(opened.core.noticeTrap(error));
    };
    globalThis.__seaquelExpectedTraps = 1;
    try {
      await expect(
        opened.core.run((m) => (m as TestModule).__test_trap("async")),
      ).rejects.toMatchObject({ code: "CORE_RESTARTED" });
      await opened.core.settled();
    } finally {
      globalThis.__seaquelOnSwallowedTrap = undefined;
    }
    expect(noticed).toEqual([true]);
    // The panic hook and the noticed error restart once between them.
    expect(resubscribed).toEqual([true, false]);
    // Noticed again after the restart: that instance is gone, nothing to do.
    expect(opened.core.noticeTrap(trapError)).toBe(false);
    await opened.core.settled();
    expect(resubscribed).toEqual([true, false]);
    opened.close();
  });

  it("the module exports every function BrowserModule names", () => {
    for (const [name, arity] of Object.entries(BROWSER_MODULE_EXPORTS)) {
      const fn = (module as unknown as Record<string, unknown>)[name];
      expect(typeof fn, name).toBe("function");
      expect((fn as (...a: unknown[]) => unknown).length, name).toBe(arity);
    }
  });
});
