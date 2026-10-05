/**
 * The demo's assistant, the whole page over the
 * browser module (its test build) and DuckDB-WASM's Node build, as
 * `start.svelte.test.ts` loads it: the visitor's key lives in this page's
 * memory for the session, goes with each turn through the page's fetch
 * bridge to a local mock provider, and is in no snapshot, storage, URL or
 * log line. A reload forgets it; a trap restart keeps it in the page, which
 * sends it again. No real provider is called; the key is the fake test key.
 */
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { AsyncDuckDB } from "@duckdb/duckdb-wasm";
import {
  bootDuckDb,
  loadTestModule,
  testModuleMissing,
  type TestModule,
} from "$lib/core/browser/testing/node";
import {
  chunk,
  localFetch,
  MockProvider,
  openaiText,
} from "$lib/core/browser/testing/mock-provider";
import type { SnapshotStore } from "$lib/core/browser/snapshot-store";

const TEST_KEY = "test-key-not-real";

const page = vi.hoisted(() => ({
  toasts: [] as { kind: string; message: string }[],
  logged: [] as unknown[][],
}));

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => false,
  isDemo: () => true,
  isBrowser: () => true,
}));
vi.mock("$lib/utils/logger", () => {
  const record = (...args: unknown[]) => void page.logged.push(args);
  return {
    log: { debug: record, error: record, info: record, warn: record, trace: record },
    initLogger: vi.fn(),
  };
});
vi.mock("svelte-sonner", () => {
  const push = (kind: string) => (message: unknown) => {
    page.toasts.push({ kind, message: String(message) });
  };
  return {
    toast: { info: push("info"), success: push("success"), warning: push("warning") },
  };
});
vi.mock("$lib/utils/toast", () => ({
  errorToast: (message: unknown) => {
    page.toasts.push({ kind: "error", message: String(message) });
  },
}));
vi.mock("mode-watcher", () => ({ mode: { current: "light" } }));

const missing = testModuleMissing();
if (missing && process.env.CI) throw new Error(missing);

/** IndexedDB as the page sees it across reloads, keeping every save made. */
class MemoryStore implements SnapshotStore {
  file: Uint8Array | null = null;
  readonly saved: Uint8Array[] = [];
  private chain: Promise<void> = Promise.resolve();
  async load() {
    return this.file;
  }
  save(bytes: Uint8Array) {
    this.chain = this.chain.then(() => {
      this.file = bytes;
      this.saved.push(bytes);
    });
    return this.chain;
  }
  async moveAside() {
    this.file = null;
  }
}

/** `localStorage`/`sessionStorage` the test can search. */
class SearchableStorage {
  readonly map = new Map<string, string>();
  get length() {
    return this.map.size;
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  getItem(key: string) {
    return this.map.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.map.set(key, String(value));
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  clear() {
    this.map.clear();
  }
  text() {
    return JSON.stringify([...this.map.entries()]);
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

let module: TestModule | null = null;
const ducks: AsyncDuckDB[] = [];
const mock = new MockProvider();
const local = new SearchableStorage();
const session = new SearchableStorage();
const consoleLines: unknown[][] = [];
/** Every request id the page's fetch bridge was asked to abort. */
const aborted: number[] = [];

beforeAll(async () => {
  module = await loadTestModule();
  await mock.start();
  vi.stubEnv("VITE_BUILD_TARGET", "demo");
  vi.stubGlobal("localStorage", local);
  vi.stubGlobal("sessionStorage", session);
  for (const level of ["log", "info", "warn", "error", "debug"] as const) {
    vi.spyOn(console, level).mockImplementation((...args: unknown[]) => {
      consoleLines.push(args);
    });
  }
});

afterEach(() => {
  page.toasts.length = 0;
});

afterAll(async () => {
  vi.unstubAllEnvs();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  await mock.stop();
  for (const duck of ducks) await duck.terminate();
});

/** One page load of the demo, its fetch bridge limited to the mock. */
async function loadPage(store: MemoryStore) {
  vi.resetModules();
  const duck = await bootDuckDb();
  ducks.push(duck);
  const browser = await import("$lib/core/browser");
  const { makeFetchBridge } = await import("$lib/core/browser/fetch-bridge");
  const fetch = makeFetchBridge(localFetch(mock));
  const abort = fetch.abort.bind(fetch);
  fetch.abort = (id) => {
    aborted.push(id);
    abort(id);
  };
  const win = new FakeWindow();
  const opened = await browser.openBrowserCore({
    module: module!,
    bridge: browser.makeDuckDbBridge(duck),
    store,
    localStorage: local as unknown as Storage,
    window: win as unknown as Window,
    document: null,
    fetch,
  });
  browser.useBrowserCore(opened);
  const { UseDatabase } = await import("$lib/hooks/database.svelte.js");
  const { startDemo } = await import("./init");
  const { getProvider } = await import("$lib/providers");
  const { aiSettingsStore } = await import("$lib/stores/ai-settings.svelte");
  const db = new UseDatabase();
  await startDemo(db, opened.core, await getProvider());
  await aiSettingsStore.initialize();
  return { db, opened, win, ai: aiSettingsStore };
}

type Loaded = Awaited<ReturnType<typeof loadPage>>;

/** Waits until `check` holds, or fails after `ms`. */
async function until(check: () => boolean, ms = 10_000) {
  const end = Date.now() + ms;
  while (!check()) {
    if (Date.now() > end) throw new Error("timed out");
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

/** A provider at the mock with the visitor's key, chosen on the demo connection. */
async function useMock({ db, ai }: Loaded, apiKey?: string) {
  const id = await ai.addProvider(
    { name: "Mock", type: "openai-compatible", baseUrl: `${mock.url}/v1` },
    apiKey,
  );
  await db.setConnectionAIModel("demo-connection", id, "model-1");
  return id;
}

/** Sends a message and waits for the turn to end; the reply's text. */
async function ask({ db }: Loaded, content: string) {
  expect(await db.ui.sendAIMessage(content)).toBe(true);
  await until(() => !db.state.isAIStreaming);
  const chatId = db.state.activeAIChatId!;
  const reply = (db.state.aiMessagesByChat[chatId] ?? []).at(-1)!;
  return reply;
}

/** The page goes away: `pagehide` (the view-state journal, the snapshot), then settled. */
async function leave({ db, opened, win }: Loaded) {
  db.saveOnPageHide();
  win.fire("pagehide");
  await opened.core.settled();
  opened.close();
}

const latin1 = (bytes: Uint8Array) => Buffer.from(bytes).toString("latin1");

describe.skipIf(missing !== null)("the demo's assistant", () => {
  it("a turn runs with the session key, and the key is stored nowhere", async () => {
    const store = new MemoryStore();
    const loaded = await loadPage(store);
    const providerId = await useMock(loaded, TEST_KEY);
    expect(loaded.ai.available).toBe(true);
    expect(await loaded.ai.hasKey(providerId)).toBe(true);

    mock.scripts.push({ events: openaiText("Three rows.") });
    const reply = await ask(loaded, "How many rows?");
    expect(reply.content).toBe("Three rows.");
    expect(reply.error).toBeFalsy();
    const request = mock.seen.at(-1)!;
    expect(request.path).toBe("/v1/chat/completions");
    expect(request.headers.authorization).toBe(`Bearer ${TEST_KEY}`);
    expect(request.path).not.toContain(TEST_KEY);

    // A view change pending at `pagehide` goes into the journal.
    loaded.db.queryTabs.add();
    await leave(loaded);
    expect(local.map.size).toBeGreaterThan(0);

    expect(store.saved.length).toBeGreaterThan(0);
    for (const snapshot of store.saved) expect(latin1(snapshot)).not.toContain(TEST_KEY);
    expect(local.text()).not.toContain(TEST_KEY);
    expect(session.text()).not.toContain(TEST_KEY);
    expect(JSON.stringify(page.logged)).not.toContain(TEST_KEY);
    expect(JSON.stringify(consoleLines)).not.toContain(TEST_KEY);
    expect(JSON.stringify(page.toasts)).not.toContain(TEST_KEY);
    for (const seen of mock.seen) expect(seen.path).not.toContain(TEST_KEY);
  }, 90_000);

  it("a reload forgets the key; the provider stays and sends none", async () => {
    const store = new MemoryStore();
    const first = await loadPage(store);
    const providerId = await useMock(first, TEST_KEY);
    await leave(first);

    const second = await loadPage(store);
    expect(second.ai.getProvider(providerId)?.name).toBe("Mock");
    expect(await second.ai.hasKey(providerId)).toBe(false);
    mock.scripts.push({ events: openaiText("No key here.") });
    const reply = await ask(second, "Hello?");
    expect(reply.content).toBe("No key here.");
    expect(mock.seen.at(-1)!.headers.authorization).toBeUndefined();
    await leave(second);
  }, 90_000);

  it("after a trap restart the page sends the key again and turns still work", async () => {
    const loaded = await loadPage(new MemoryStore());
    await useMock(loaded, TEST_KEY);
    mock.scripts.push({ events: openaiText("Before.") });
    expect((await ask(loaded, "One")).content).toBe("Before.");

    const before = loaded.db.state.connections[0].providerConnectionId;
    await expect(
      loaded.opened.core.run((m) => (m as TestModule).__test_trap("sync")),
    ).rejects.toMatchObject({ code: "CORE_RESTARTED" });
    await loaded.opened.core.settled();
    // The page reconnects the demo connection at once (CORE_RESTARTED).
    await until(() => {
      const id = loaded.db.state.connections[0].providerConnectionId;
      return !!id && id !== before && loaded.db.state.activeConnectionId === "demo-connection";
    });

    mock.scripts.push({ events: openaiText("After.") });
    const reply = await ask(loaded, "Two");
    expect(reply.error).toBeFalsy();
    expect(reply.content).toBe("After.");
    expect(mock.seen.at(-1)!.headers.authorization).toBe(`Bearer ${TEST_KEY}`);
    await leave(loaded);
  }, 90_000);

  it("Stop aborts the provider's request and keeps what streamed", async () => {
    const loaded = await loadPage(new MemoryStore());
    await useMock(loaded, TEST_KEY);
    mock.scripts.push({
      events: [chunk({ choices: [{ index: 0, delta: { content: "Partial" } }] })],
      stall: true,
    });
    const goneBefore = mock.gone;
    const abortedBefore = aborted.length;
    expect(await loaded.db.ui.sendAIMessage("Go on")).toBe(true);
    const chatId = loaded.db.state.activeAIChatId!;
    await until(() =>
      (loaded.db.state.aiMessagesByChat[chatId] ?? []).some(
        (m) => m.role === "assistant" && JSON.stringify(m).includes("Partial"),
      ),
    );
    loaded.db.ui.cancelAIStream();
    await until(() => aborted.length > abortedBefore);
    await expect.poll(() => mock.gone).toBeGreaterThan(goneBefore);
    await leave(loaded);
  }, 90_000);
  it("a trap during a streaming turn aborts its model request", async () => {
    const loaded = await loadPage(new MemoryStore());
    await useMock(loaded, TEST_KEY);
    mock.scripts.push({
      events: [chunk({ choices: [{ index: 0, delta: { content: "Streaming" } }] })],
      stall: true,
    });
    const goneBefore = mock.gone;
    expect(await loaded.db.ui.sendAIMessage("Go on")).toBe(true);
    const chatId = loaded.db.state.activeAIChatId!;
    await until(() =>
      (loaded.db.state.aiMessagesByChat[chatId] ?? []).some(
        (m) => m.role === "assistant" && JSON.stringify(m).includes("Streaming"),
      ),
    );
    await expect(
      loaded.opened.core.run((m) => (m as TestModule).__test_trap("sync")),
    ).rejects.toMatchObject({ code: "CORE_RESTARTED" });
    await loaded.opened.core.settled();
    // The dead instance can't drop its request; the restart aborts it.
    await expect.poll(() => mock.gone, { timeout: 3000 }).toBeGreaterThan(goneBefore);
    await until(() => !loaded.db.state.isAIStreaming);
    await leave(loaded);
  }, 90_000);
});
