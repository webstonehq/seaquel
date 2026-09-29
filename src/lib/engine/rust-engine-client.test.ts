import { afterEach, describe, expect, it, vi } from "vitest";
import type { EngineRequest } from "$lib/types/generated/EngineRequest";
import type { EngineResponse } from "$lib/types/generated/EngineResponse";
import type { CreateTableDefinition } from "$lib/types";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const env = vi.hoisted(() => ({ tauri: false }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => env.tauri,
  isWeb: () => !env.tauri,
  isDemo: () => false,
}));

import { RustEngineClient, coreEngineTransport } from "./rust-engine-client";

interface Call {
  connectionId: string;
  request: EngineRequest;
}

/** A client whose transport records the call and answers `response`. */
function recording(response: EngineResponse, id: string | null = "pc-1") {
  const calls: Call[] = [];
  const transport = vi.fn((connectionId: string, request: EngineRequest) => {
    // Round-trip through JSON, as both real transports do.
    calls.push(JSON.parse(JSON.stringify({ connectionId, request })) as Call);
    return Promise.resolve(JSON.parse(JSON.stringify(response)) as EngineResponse);
  });
  return {
    client: new RustEngineClient("postgres", () => id ?? undefined, transport),
    calls,
    transport,
  };
}

const def: CreateTableDefinition = {
  tableName: "t",
  schemaName: "public",
  columns: [],
  indexes: [],
  foreignKeys: [],
};

describe("RustEngineClient request shapes and unwrapping", () => {
  it("listSchemas", async () => {
    const { client, calls } = recording({ kind: "schemas", data: ["public"] });
    expect(await client.listSchemas()).toEqual(["public"]);
    expect(calls).toEqual([{ connectionId: "pc-1", request: { method: "listSchemas" } }]);
  });

  it("schemaTables", async () => {
    const table = { name: "t", schema: "public", type: "table", columns: [], indexes: [] };
    const { client, calls } = recording({ kind: "tables", data: [table] as never });
    expect(await client.schemaTables()).toEqual([table]);
    expect(calls[0].request).toEqual({ method: "schemaTables" });
  });

  it("tableMetadata", async () => {
    const data = {
      columns: [],
      indexes: [{ name: "i", columns: ["a"], unique: true, type: "btree" }],
    };
    const { client, calls } = recording({ kind: "tableMetadata", data });
    expect(await client.tableMetadata("public", "order items")).toEqual(data);
    expect(calls[0].request).toEqual({
      method: "tableMetadata",
      params: { schema: "public", table: "order items" },
    });
  });

  it("statistics", async () => {
    const data = {
      overview: { databaseName: "db", totalSize: "1 MB", tableCount: 1, indexCount: 0 },
      tableSizes: [],
      indexUsage: [],
    };
    const { client, calls } = recording({ kind: "statistics", data });
    expect(await client.statistics()).toEqual(data);
    expect(calls[0].request).toEqual({ method: "statistics" });
  });

  it("explain encodes its params", async () => {
    const data = {
      plan: { id: "0", nodeType: "Result", children: [] },
      planningTime: 0.1,
      isAnalyze: true,
    };
    const { client, calls } = recording({ kind: "explain", data });
    expect(await client.explain("SELECT $1, $2", [10n, "x"], true)).toEqual(data);
    expect(calls[0].request).toEqual({
      method: "explain",
      params: {
        sql: "SELECT $1, $2",
        params: [{ $sq: "bigint", v: "10" }, "x"],
        analyze: true,
      },
    });
  });

  it("explain without params sends an empty list", async () => {
    const data = {
      plan: { id: "0", nodeType: "Result", children: [] },
      planningTime: 0,
      isAnalyze: false,
    };
    const { client, calls } = recording({ kind: "explain", data });
    await client.explain("SELECT 1", undefined, false);
    expect(calls[0].request).toEqual({
      method: "explain",
      params: { sql: "SELECT 1", params: [], analyze: false },
    });
  });

  it("columnTypes", async () => {
    const data = [{ name: "int4", category: "Numeric" }] as never;
    const { client, calls } = recording({ kind: "columnTypes", data });
    expect(await client.columnTypes()).toEqual(data);
    expect(calls[0].request).toEqual({ method: "columnTypes" });
  });

  it("createTable and alterTable", async () => {
    const create = recording({ kind: "sql", data: "CREATE" });
    expect(await create.client.createTable(def)).toBe("CREATE");
    expect(create.calls[0].request).toEqual({ method: "createTable", params: { definition: def } });

    const to = { ...def, tableName: "u" };
    const alter = recording({ kind: "sql", data: "ALTER" });
    expect(await alter.client.alterTable(def, to)).toBe("ALTER");
    expect(alter.calls[0].request).toEqual({ method: "alterTable", params: { from: def, to } });
  });

  it("rejects a response of the wrong kind", async () => {
    const { client } = recording({ kind: "sql", data: "x" });
    await expect(client.schemaTables()).rejects.toThrow(
      'ENGINE_PROTOCOL: expected "tables" response, got "sql"',
    );
  });

  it("throws without a provider connection id and never calls the transport", async () => {
    const { client, transport } = recording({ kind: "schemas", data: [] }, null);
    await expect(client.listSchemas()).rejects.toThrow("No connection established");
    expect(transport).not.toHaveBeenCalled();
  });
});

describe("RustEngineClient.qualifiedTable (computed locally)", () => {
  // DuckDB: `quote_schema`/`qualified_table` in crates/seaquel-engine-duckdb/src/dialect.rs.
  it.each([
    ["main", "users", '"main"."users"'],
    ['"a.b"', "t", '"a.b"."t"'],
    ["fx_aux.main", "users", '"fx_aux"."main"."users"'],
    ['"fx.we""ird".main', "items", '"fx.we""ird"."main"."items"'],
    ['"fx_aux.main"', "users", '"fx_aux.main"."users"'],
    ["main", 'it\'s "x"', '"main"."it\'s ""x"""'],
    ["a.b.c", "t", '"a.b.c"."t"'],
  ])("DuckDB: %s . %s", (schema, table, expected) => {
    const transport = vi.fn();
    const client = new RustEngineClient("duckdb", () => undefined, transport);
    expect(client.qualifiedTable(schema, table)).toBe(expected);
    expect(transport).not.toHaveBeenCalled();
  });

  // The other engines keep the data tab's quoting as it was.
  it.each([
    ["postgres", '"public"."order items"'],
    ["sqlite", '"public"."order items"'],
    ["mysql", "`public`.`order items`"],
    ["mariadb", "`public`.`order items`"],
    ["mssql", "[public].[order items]"],
  ] as const)("%s keeps its quoting", (engine, expected) => {
    const client = new RustEngineClient(engine, () => undefined, vi.fn());
    expect(client.qualifiedTable("public", "order items")).toBe(expected);
  });
});

describe("RustEngineClient connection id", () => {
  it("reads the id on every call, so a reconnect is picked up", async () => {
    let id: string | undefined = "old";
    const transport = vi.fn((connectionId: string) =>
      Promise.resolve<EngineResponse>({ kind: "schemas", data: [connectionId] }),
    );
    const client = new RustEngineClient("postgres", () => id, transport);
    expect(await client.listSchemas()).toEqual(["old"]);
    id = undefined; // disconnected
    await expect(client.listSchemas()).rejects.toThrow("No connection established");
    id = "new"; // reconnected
    expect(await client.listSchemas()).toEqual(["new"]);
    expect(transport).toHaveBeenCalledTimes(2);
  });
});

/** The request bytes `core_call` or `POST /api/rpc` got, parsed. */
function decode(body: unknown): unknown {
  return JSON.parse(new TextDecoder().decode(body as Uint8Array));
}

describe("coreEngineTransport", () => {
  afterEach(() => {
    env.tauri = false;
    invoke.mockReset();
    vi.unstubAllGlobals();
  });

  it("sends db.engine on desktop through core_call", async () => {
    env.tauri = true;
    invoke.mockResolvedValue({
      method: "db",
      result: { method: "engine", result: { kind: "schemas", data: ["public"] } },
    });
    expect(await new RustEngineClient("postgres", () => "pc-1").listSchemas()).toEqual(["public"]);
    expect(invoke).toHaveBeenCalledOnce();
    const [cmd, body] = invoke.mock.calls[0] as [string, unknown];
    expect(cmd).toBe("core_call");
    expect(decode(body)).toEqual({
      method: "db",
      params: {
        method: "engine",
        params: { connectionId: "pc-1", request: { method: "listSchemas" } },
      },
    });
  });

  it("sends db.engine on web through POST /api/rpc", async () => {
    const fetchMock = vi.fn<typeof fetch>(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            method: "db",
            result: { method: "engine", result: { kind: "sql", data: "CREATE" } },
          }),
        ),
      ),
    );
    vi.stubGlobal("fetch", fetchMock);
    const client = new RustEngineClient("postgres", () => "pc-1", coreEngineTransport());
    expect(await client.createTable(def)).toBe("CREATE");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/rpc");
    expect(decode(init?.body)).toEqual({
      method: "db",
      params: {
        method: "engine",
        params: {
          connectionId: "pc-1",
          request: { method: "createTable", params: { definition: def } },
        },
      },
    });
    expect(invoke).not.toHaveBeenCalled();
  });

  it("maps an RpcError to CODE: message", async () => {
    env.tauri = true;
    invoke.mockRejectedValue({ code: "CONNECTION_NOT_FOUND", message: "gone" });
    await expect(new RustEngineClient("postgres", () => "pc-1").statistics()).rejects.toThrow(
      "CONNECTION_NOT_FOUND: gone",
    );
  });

  it("refuses a response for another method", async () => {
    env.tauri = true;
    invoke.mockResolvedValue({ method: "db", result: { method: "query", result: {} } });
    await expect(new RustEngineClient("postgres", () => "pc-1").statistics()).rejects.toThrow(
      "PROTOCOL_ERROR",
    );
  });
});
