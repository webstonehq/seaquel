/**
 * The demo's start on Core in the page (phase 8, Task 6; Decisions 6, 17,
 * 19 and 20): the whole page (`UseDatabase`) over the browser module (its
 * test build) and DuckDB-WASM's Node build, as `src/routes/(app)/+layout`
 * starts it. Each "page load" is a fresh module graph and a fresh DuckDB
 * (a reload's DuckDB starts empty), over one snapshot store, as IndexedDB
 * keeps it across reloads.
 */
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { AsyncDuckDB } from "@duckdb/duckdb-wasm";
import {
  bootDuckDb,
  loadTestModule,
  testModuleMissing,
  type TestModule,
} from "$lib/core/browser/testing/node";
import type { SnapshotStore } from "$lib/core/browser/snapshot-store";

const page = vi.hoisted(() => ({ toasts: [] as { kind: string; message: string }[] }));

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => false,
  isDemo: () => true,
  isBrowser: () => true,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
  initLogger: vi.fn(),
}));
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

/** IndexedDB as the page sees it across reloads: saves land in call order. */
class MemoryStore implements SnapshotStore {
  file: Uint8Array | null = null;
  private chain: Promise<void> = Promise.resolve();
  async load() {
    return this.file;
  }
  save(bytes: Uint8Array) {
    this.chain = this.chain.then(() => {
      this.file = bytes;
    });
    return this.chain;
  }
  async moveAside() {
    this.file = null;
  }
}

class FakeLocalStorage {
  readonly map = new Map<string, string>();
  readonly reads: string[] = [];
  get length() {
    return this.map.size;
  }
  key(i: number) {
    return [...this.map.keys()][i] ?? null;
  }
  getItem(key: string) {
    this.reads.push(key);
    return this.map.get(key) ?? null;
  }
  setItem(key: string, value: string) {
    this.map.set(key, value);
  }
  removeItem(key: string) {
    this.map.delete(key);
  }
  clear() {
    this.map.clear();
  }
}

let module: TestModule | null = null;
const ducks: AsyncDuckDB[] = [];

beforeAll(async () => {
  module = await loadTestModule();
  vi.stubEnv("VITE_BUILD_TARGET", "demo");
});

afterEach(() => {
  page.toasts.length = 0;
});

afterAll(async () => {
  vi.unstubAllEnvs();
  for (const duck of ducks) await duck.terminate();
});

/**
 * One page load of the demo: the page's Core over `store`, then the page
 * (`UseDatabase`) and the demo's start, as the layouts run them.
 */
async function loadPage(store: MemoryStore, localStorage: FakeLocalStorage | null = null) {
  vi.resetModules();
  const duck = await bootDuckDb();
  ducks.push(duck);
  const browser = await import("$lib/core/browser");
  const opened = await browser.openBrowserCore({
    module: module!,
    bridge: browser.makeDuckDbBridge(duck),
    store,
    localStorage: localStorage as unknown as Storage | null,
    window: null,
    document: null,
  });
  browser.useBrowserCore(opened);
  const { UseDatabase } = await import("$lib/hooks/database.svelte.js");
  const { startDemo } = await import("./init");
  const { getProvider } = await import("$lib/providers");
  const db = new UseDatabase();
  await startDemo(db, opened.core, await getProvider());
  return { db, opened };
}

/** Lets the page's debounced view-state save go out, then stores the snapshot. */
async function leave({ db, opened }: Awaited<ReturnType<typeof loadPage>>) {
  await db.flush();
  await opened.core.settled();
  opened.core.flushNow();
  await opened.core.settled();
  opened.close();
}

const tabNames = (db: Awaited<ReturnType<typeof loadPage>>["db"]) => {
  const projectId = db.state.activeProjectId!;
  return (db.state.queryTabsByProject[projectId] ?? []).map((t) => t.name);
};

describe.skipIf(missing)("the demo's start", () => {
  it("the demo starts on an empty store", async () => {
    const store = new MemoryStore();
    const loaded = await loadPage(store);
    const { db } = loaded;

    expect(db.state.connections.map((c) => [c.id, c.name, c.type, c.labelIds])).toEqual([
      ["demo-connection", "Demo Database", "duckdb", ["prod"]],
    ]);
    const demo = db.state.connections[0];
    expect(demo.providerConnectionId).toBeTruthy();
    expect(db.state.activeConnectionIdByProject[demo.projectId]).toBe("demo-connection");
    // The sample tables were seeded before the schema was read.
    expect((db.state.schemas["demo-connection"] ?? []).map((t) => `${t.schema}.${t.name}`)).toEqual(
      expect.arrayContaining(["demo.customers", "demo.orders", "demo.products"]),
    );
    expect(db.state.dashboardsByProject[demo.projectId].map((d) => d.name)).toEqual([
      "E-Commerce Overview",
    ]);
    expect(tabNames(db)).toEqual(["Query 1"]);
    expect(page.toasts.filter((t) => t.kind === "error")).toEqual([]);

    await leave(loaded);
    expect(store.file).not.toBeNull();
  }, 60_000);

  it("the demo starts clean with an old file in localStorage and deletes it", async () => {
    const local = new FakeLocalStorage();
    // An earlier demo's metadata file (Q2 C: never read), and keys that stay.
    local.setItem("seaquel_db", "U1FMaXRlIGZvcm1hdCAz");
    local.setItem("seaquel_db.backup", "x");
    local.setItem("seaquel-theme-cache", "{}");
    local.setItem("PARAGLIDE_LOCALE", "en");

    const loaded = await loadPage(new MemoryStore(), local);

    expect([...local.map.keys()].sort()).toEqual(["PARAGLIDE_LOCALE", "seaquel-theme-cache"]);
    expect(local.reads.filter((k) => k.startsWith("seaquel_db"))).toEqual([]);
    // A new metadata file: the demo connection, the default project, the sample.
    expect(loaded.db.state.connections.map((c) => c.id)).toEqual(["demo-connection"]);
    expect(loaded.db.state.projects).toHaveLength(1);
    const projectId = loaded.db.state.activeProjectId!;
    expect(loaded.db.state.queriesByProject[projectId] ?? []).toEqual([]);
    await leave(loaded);
  }, 60_000);

  it("a reload keeps a saved query and makes no second dashboard or tab", async () => {
    const store = new MemoryStore();
    const first = await loadPage(store);
    const projectId = first.db.state.activeProjectId!;
    const tabId = first.db.state.activeQueryTabIdByProject[projectId]!;
    await first.db.savedQueries.saveQuery("Top customers", "SELECT 1", tabId);
    const tabsBefore = tabNames(first.db);
    await leave(first);

    const second = await loadPage(store);
    const { db } = second;
    expect(db.state.activeProjectId).toBe(projectId);
    expect((db.state.queriesByProject[projectId] ?? []).map((q) => q.name)).toEqual([
      "Top customers",
    ]);
    expect(db.state.dashboardsByProject[projectId].map((d) => d.name)).toEqual([
      "E-Commerce Overview",
    ]);
    expect(tabNames(db)).toEqual(tabsBefore);
    expect(page.toasts.filter((t) => t.kind === "error")).toEqual([]);
    await leave(second);
  }, 60_000);

  it("a reload keeps the demo connection's query history", async () => {
    // Task 1 found today's demo dropping it on every load.
    const store = new MemoryStore();
    const first = await loadPage(store);
    const { getStorage } = await import("$lib/storage");
    await getStorage().queryHistory.append({
      id: "h1",
      query: "SELECT 42",
      timestamp: "2026-01-01T00:00:00.000Z",
      executionTime: 1,
      rowCount: 1,
      connectionId: "demo-connection",
      favorite: false,
      connectionLabelsSnapshot: null,
      connectionNameSnapshot: "Demo Database",
    });
    await leave(first);

    const second = await loadPage(store);
    expect(
      (second.db.state.queryHistoryByConnection["demo-connection"] ?? []).map((h) => h.query),
    ).toEqual(["SELECT 42"]);
    await leave(second);
  }, 60_000);
});
