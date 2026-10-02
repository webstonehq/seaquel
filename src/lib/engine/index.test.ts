import { afterEach, describe, expect, it, vi } from "vitest";

const env = vi.hoisted(() => ({ tauri: false, web: false }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => env.tauri,
  isWeb: () => env.web,
  isDemo: () => !env.tauri && !env.web,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

import { getEngineClient, RustEngineClient } from "./index";
import { invoke } from "@tauri-apps/api/core";

const conn = (
  type: "postgres" | "mysql" | "sqlite" | "mssql" | "duckdb" | "mariadb",
): { id: string; type: typeof type; name: string; providerConnectionId?: string } => ({
  id: "conn-1",
  type,
  name: "c",
  providerConnectionId: "pc-1",
});

const TYPES = ["postgres", "mysql", "mariadb", "sqlite", "mssql", "duckdb"] as const;
const BUILDS = [
  ["desktop", { tauri: true, web: false }],
  ["web", { tauri: false, web: true }],
  // Phase 8: the demo's Core runs in the page.
  ["the demo", { tauri: false, web: false }],
] as const;
const CASES = TYPES.flatMap((type) => BUILDS.map(([mode, flags]) => [type, mode, flags] as const));

describe("getEngineClient", () => {
  afterEach(() => {
    env.tauri = false;
    env.web = false;
  });

  it.each(CASES)("uses Core for %s on %s", (type, _mode, flags) => {
    Object.assign(env, flags);
    expect(getEngineClient(conn(type))).toBeInstanceOf(RustEngineClient);
  });

  it.each(CASES)("answers %s on %s through Core", async (type, _mode, flags) => {
    Object.assign(env, flags);
    const reply = {
      method: "db",
      result: { method: "engine", result: { kind: "schemas", data: ["public"] } },
    };
    vi.mocked(invoke).mockResolvedValue(reply);
    vi.stubGlobal(
      "fetch",
      vi.fn(() => Promise.resolve(new Response(JSON.stringify(reply)))),
    );
    try {
      expect(await getEngineClient(conn(type)).listSchemas()).toEqual(["public"]);
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
    vi.mocked(invoke).mockResolvedValue({
      method: "db",
      result: { method: "engine", result: { kind: "schemas", data: [] } },
    });
    const state = {
      connections: [{ ...conn("postgres"), providerConnectionId: "old" }] as ReturnType<
        typeof conn
      >[],
    };
    const client = getEngineClient(state.connections[0], state);

    // What reconnect() does: a new object with the new id replaces the old one.
    state.connections = state.connections.map((c) => ({ ...c, providerConnectionId: "new" }));
    await client.listSchemas();
    const body = vi.mocked(invoke).mock.calls[0][1] as Uint8Array;
    expect(JSON.parse(new TextDecoder().decode(body))).toEqual({
      method: "db",
      params: {
        method: "engine",
        params: { connectionId: "new", request: { method: "listSchemas" } },
      },
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
