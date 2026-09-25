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

import { getEngineClient, RustEngineClient, TsEngineClient } from "./index";
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
    ["desktop", { tauri: true, web: false }],
    ["web", { tauri: false, web: true }],
  ])("uses Rust for Postgres on %s", (_mode, flags) => {
    Object.assign(env, flags);
    expect(getEngineClient(conn("postgres"))).toBeInstanceOf(RustEngineClient);
  });

  it("uses TypeScript for Postgres in the demo", () => {
    expect(getEngineClient(conn("postgres"))).toBeInstanceOf(TsEngineClient);
  });

  it.each(["mysql", "mariadb", "sqlite", "mssql", "duckdb"] as const)(
    "uses TypeScript for %s on desktop and web",
    (type) => {
      env.tauri = true;
      expect(getEngineClient(conn(type))).toBeInstanceOf(TsEngineClient);
      env.tauri = false;
      env.web = true;
      expect(getEngineClient(conn(type))).toBeInstanceOf(TsEngineClient);
    },
  );
});

describe("getEngineClient and the TypeScript Postgres adapter", () => {
  afterEach(() => {
    env.tauri = false;
    env.web = false;
    vi.mocked(getAdapter).mockClear();
  });

  it("has no Postgres adapter any more", () => {
    expect(() => getAdapter("postgres")).toThrow('Database type "postgres" is not supported yet');
  });

  // Desktop and web only. In the demo a Postgres connection still gets a
  // TsEngineClient (whose adapter lookup would throw on first use), which is
  // fine: the demo has no Rust core and can't create Postgres connections.
  it.each([
    ["desktop", { tauri: true, web: false }],
    ["web", { tauri: false, web: true }],
  ])("never asks getAdapter for postgres on %s", async (_mode, flags) => {
    Object.assign(env, flags);
    const reply = { kind: "schemas", data: ["public"] };
    vi.mocked(invoke).mockResolvedValue(reply);
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify(reply)))),
    );
    try {
      const client = getEngineClient(conn("postgres"));
      expect(await client.listSchemas()).toEqual(["public"]);
      expect(vi.mocked(getAdapter)).not.toHaveBeenCalledWith("postgres");
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
