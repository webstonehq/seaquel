/**
 * The tutorial's database in the browser is its own DuckDB-WASM instance,
 * apart from the demo's (Task 6 review, I1): Learn's tables never show in
 * the demo connection, and the demo's never in the tutorial, so neither
 * can break or drop the other's. DuckDB-WASM's Node build stands in for
 * the page's (`startDuckDb` gives a new instance each time, as in the page);
 * the demo connection runs through the browser module's test build.
 */
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import type { AsyncDuckDB } from "@duckdb/duckdb-wasm";
import { loadTestModule, testModuleMissing, type TestModule } from "$lib/core/browser/testing/node";
import type { SnapshotStore } from "$lib/core/browser/snapshot-store";
import type { OpenedBrowserCore } from "$lib/core/browser";
import type { EngineClient } from "$lib/engine";

const ducks = vi.hoisted(() => [] as AsyncDuckDB[]);

vi.mock("$lib/utils/environment", () => ({
  isTauri: () => false,
  isWeb: () => false,
  isDemo: () => true,
}));
vi.mock("$lib/utils/logger", () => ({
  log: { debug: vi.fn(), error: vi.fn(), info: vi.fn(), warn: vi.fn(), trace: vi.fn() },
}));
vi.mock("$lib/providers/duckdb-start", async () => {
  const { bootDuckDb: boot } = await import("$lib/core/browser/testing/node");
  return {
    startDuckDb: async () => {
      const db = await boot();
      ducks.push(db);
      return db;
    },
  };
});

const missing = testModuleMissing();

class MemoryStore implements SnapshotStore {
  async load() {
    return null;
  }
  async save() {}
  async moveAside() {}
}

let module: TestModule | null = null;
let opened: OpenedBrowserCore;
let demo: EngineClient;

beforeAll(async () => {
  // The demo build: the tutorial runs on DuckDB-WASM in the page.
  vi.stubEnv("VITE_BUILD_TARGET", "demo");
  module = await loadTestModule();
  if (!module) return;
  const { openBrowserCore, makeDuckDbBridge } = await import("$lib/core/browser");
  const { setCoreClient } = await import("$lib/core");
  const { pageDuckDb } = await import("$lib/providers/duckdb-wasm");
  const { CoreProvider } = await import("$lib/providers/core-provider");
  const { getEngineClient } = await import("$lib/engine");
  // The demo's Core on the page's DuckDB, as `$lib/demo/core` opens it.
  opened = await openBrowserCore({
    module,
    bridge: makeDuckDbBridge(await pageDuckDb()),
    store: new MemoryStore(),
    localStorage: null,
    window: null,
    document: null,
  });
  setCoreClient(opened.client);
  await opened.core.ensureDemoConnection();
  const provider = new CoreProvider(() => opened.client);
  const coreId = await provider.connect({ target: { type: "saved", id: "demo-connection" } });
  await provider.execute(coreId, "CREATE SCHEMA demo");
  await provider.execute(coreId, "CREATE TABLE demo.customers (id INTEGER)");
  demo = getEngineClient({ id: "demo-connection", type: "duckdb", providerConnectionId: coreId });
}, 60_000);

afterAll(async () => {
  vi.unstubAllEnvs();
  const { setCoreClient } = await import("$lib/core");
  setCoreClient(null);
  opened?.close();
  for (const db of ducks) await db.terminate();
});

describe.skipIf(missing)("the tutorial's database", () => {
  it("is a DuckDB of its own: neither side sees the other's tables", async () => {
    const { executeQuery } = await import("./database");
    // The first query seeds the tutorial (`initializeTutorialDatabase`).
    await executeQuery("SELECT 1");

    const demoTables = (await demo.schemaTables()).map((t) => `${t.schema}.${t.name}`);
    expect(demoTables).toEqual(["demo.customers"]);
    expect(await demo.listSchemas()).not.toContain("tutorial");

    const tutorialTables = (
      await executeQuery(
        "SELECT table_schema || '.' || table_name AS t FROM information_schema.tables ORDER BY t",
      )
    ).map((r) => String(r.t));
    expect(tutorialTables.length).toBeGreaterThan(0);
    expect(tutorialTables).not.toContain("demo.customers");
    expect(tutorialTables.some((t) => t.startsWith("demo."))).toBe(false);
  }, 60_000);
});
