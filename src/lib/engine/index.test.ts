import { afterEach, describe, expect, it, vi } from "vitest";

const env = vi.hoisted(() => ({ tauri: false, web: false }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => env.tauri,
  isWeb: () => env.web,
  isDemo: () => !env.tauri && !env.web,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
// The real registry, with getAdapter wrapped in a spy.
vi.mock("$lib/db", async (importOriginal) => {
  const actual = await importOriginal<typeof import("$lib/db")>();
  return { ...actual, getAdapter: vi.fn(actual.getAdapter) };
});

import { getEngineClient, RustEngineClient, TsEngineClient, usesRustEngine } from "./index";
import { invoke } from "@tauri-apps/api/core";
import { getAdapter } from "$lib/db";

const conn = (
  type: "postgres" | "mysql" | "sqlite" | "mssql" | "duckdb" | "mariadb",
): { id: string; type: typeof type; name: string; providerConnectionId?: string } => ({
  id: "conn-1",
  type,
  name: "c",
  providerConnectionId: "pc-1",
});

describe("getEngineClient", () => {
  afterEach(() => {
    env.tauri = false;
    env.web = false;
  });

  it.each([
    ["postgres", "desktop", { tauri: true, web: false }],
    ["postgres", "web", { tauri: false, web: true }],
    ["mysql", "desktop", { tauri: true, web: false }],
    ["mysql", "web", { tauri: false, web: true }],
    ["mariadb", "desktop", { tauri: true, web: false }],
    ["mariadb", "web", { tauri: false, web: true }],
    ["sqlite", "desktop", { tauri: true, web: false }],
    ["sqlite", "web", { tauri: false, web: true }],
    ["mssql", "desktop", { tauri: true, web: false }],
    ["mssql", "web", { tauri: false, web: true }],
    ["duckdb", "desktop", { tauri: true, web: false }],
    ["duckdb", "web", { tauri: false, web: true }],
  ] as const)("uses Rust for %s on %s", (type, _mode, flags) => {
    Object.assign(env, flags);
    expect(getEngineClient(conn(type))).toBeInstanceOf(RustEngineClient);
    expect(usesRustEngine(conn(type))).toBe(true);
  });

  // The demo has no Rust core; its DuckDB-WASM keeps duckdb.ts.
  it.each(["postgres", "mysql", "mariadb", "sqlite", "mssql", "duckdb"] as const)(
    "uses TypeScript for %s in the demo",
    (type) => {
      expect(getEngineClient(conn(type))).toBeInstanceOf(TsEngineClient);
      expect(usesRustEngine(conn(type))).toBe(false);
    },
  );
});

describe("getEngineClient and the TypeScript adapters", () => {
  afterEach(() => {
    env.tauri = false;
    env.web = false;
    vi.mocked(getAdapter).mockClear();
  });

  // Only the demo's DuckDB keeps a TypeScript adapter.
  it.each(["postgres", "mysql", "mariadb", "sqlite", "mssql"] as const)(
    "has no %s adapter any more",
    (type) => {
      expect(() => getAdapter(type)).toThrow(`Database type "${type}" is not supported yet`);
    },
  );

  it("keeps the DuckDB adapter for the demo", () => {
    expect(getAdapter("duckdb").paginateQuery("SELECT 1", 10, 0)).toBe(
      "SELECT 1 LIMIT 10 OFFSET 0",
    );
  });

  // Desktop and web only. In the demo any connection still gets a
  // TsEngineClient (whose adapter lookup throws on first use for anything
  // but DuckDB), which is fine: the demo can only create DuckDB connections.
  it.each([
    ["postgres", "desktop", { tauri: true, web: false }],
    ["postgres", "web", { tauri: false, web: true }],
    ["mysql", "desktop", { tauri: true, web: false }],
    ["mysql", "web", { tauri: false, web: true }],
    ["mariadb", "desktop", { tauri: true, web: false }],
    ["mariadb", "web", { tauri: false, web: true }],
    ["sqlite", "desktop", { tauri: true, web: false }],
    ["sqlite", "web", { tauri: false, web: true }],
    ["mssql", "desktop", { tauri: true, web: false }],
    ["mssql", "web", { tauri: false, web: true }],
    ["duckdb", "desktop", { tauri: true, web: false }],
    ["duckdb", "web", { tauri: false, web: true }],
  ] as const)("never asks getAdapter for %s on %s", async (type, _mode, flags) => {
    Object.assign(env, flags);
    const reply = { kind: "schemas", data: ["public"] };
    vi.mocked(invoke).mockResolvedValue(reply);
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify(reply)))),
    );
    try {
      const client = getEngineClient(conn(type));
      expect(client).not.toBeInstanceOf(TsEngineClient);
      expect(await client.listSchemas()).toEqual(["public"]);
      expect(vi.mocked(getAdapter)).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllGlobals();
      vi.mocked(invoke).mockReset();
    }
  });
});

describe("getEngineClient connection id", () => {
  afterEach(() => {
    env.tauri = false;
    vi.mocked(invoke).mockReset();
  });

  it("follows the connection in state across a reconnect (the object is replaced)", async () => {
    env.tauri = true;
    vi.mocked(invoke).mockResolvedValue({ kind: "schemas", data: [] });
    const state = {
      connections: [{ ...conn("postgres"), providerConnectionId: "old" }] as ReturnType<
        typeof conn
      >[],
    };
    const client = getEngineClient(state.connections[0], state);

    // What reconnect() does: a new object with the new id replaces the old one.
    state.connections = state.connections.map((c) => ({ ...c, providerConnectionId: "new" }));
    await client.listSchemas();
    expect(vi.mocked(invoke).mock.calls[0][1]).toEqual({
      call: { connection_id: "new", request: { method: "listSchemas" } },
    });

    // disconnect() leaves the id undefined.
    state.connections = state.connections.map((c) => ({ ...c, providerConnectionId: undefined }));
    await expect(client.listSchemas()).rejects.toThrow("No connection established");
  });

  it("without state, reads the object it was given", async () => {
    const c = { ...conn("sqlite"), providerConnectionId: undefined };
    const client = getEngineClient(c);
    await expect(client.schemaTables()).rejects.toThrow("No connection established");
  });
});
