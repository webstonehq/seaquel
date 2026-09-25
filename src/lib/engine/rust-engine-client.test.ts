import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EngineCall } from "$lib/types/generated/EngineCall";
import type { EngineResponse } from "$lib/types/generated/EngineResponse";
import type { CreateTableDefinition } from "$lib/types";
import { SqlDecimal } from "$lib/values";
import paginateFixture from "../../../crates/seaquel-engine-postgres/tests/fixtures/paginate.json";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

const env = vi.hoisted(() => ({ tauri: false }));
vi.mock("$lib/utils/environment", () => ({
  isTauri: () => env.tauri,
  isWeb: () => !env.tauri,
  isDemo: () => false,
}));

import { RustEngineClient, httpTransport, tauriTransport } from "./rust-engine-client";

/** A client whose transport records the call and answers `response`. */
function recording(response: EngineResponse, id: string | null = "pc-1") {
  const calls: EngineCall[] = [];
  const transport = vi.fn((call: EngineCall) => {
    // Round-trip through JSON, as both real transports do.
    calls.push(JSON.parse(JSON.stringify(call)) as EngineCall);
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
    expect(calls).toEqual([{ connection_id: "pc-1", request: { method: "listSchemas" } }]);
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

  it("buildUpdate sends snake_case params, tagged values and only the primary-key pairs", async () => {
    const { client, calls } = recording({
      kind: "sqlWithBindings",
      data: { sql: "UPDATE", bindValues: ["y", { $sq: "bigint", v: "9007199254740993" }] },
    });
    const out = await client.buildUpdate(
      "public",
      "t",
      "blob",
      new Uint8Array([1, 2, 255]),
      ["id", "k"],
      { k: new SqlDecimal("1.50"), blob: "old", id: 9007199254740993n },
      { id: "bigint" },
    );
    expect(out).toEqual({ sql: "UPDATE", bindValues: ["y", 9007199254740993n] });
    expect(calls[0].request).toEqual({
      method: "buildUpdate",
      params: {
        schema: "public",
        table: "t",
        column: "blob",
        value: { $sq: "bytes", v: "AQL/" },
        primary_keys: ["id", "k"],
        // Row order, not primary-key order.
        row: [
          ["k", { $sq: "decimal", v: "1.50" }],
          ["id", { $sq: "bigint", v: "9007199254740993" }],
        ],
        casts: { id: "bigint" },
      },
    });
  });

  it("buildUpdate omits casts when there are none and sends undefined as null", async () => {
    const { client, calls } = recording({ kind: "sqlWithBindings", data: { sql: "UPDATE" } });
    const out = await client.buildUpdate("s", "t", "c", undefined, ["id"], { id: 1 });
    expect(out).toEqual({ sql: "UPDATE" });
    expect(calls[0].request).toEqual({
      method: "buildUpdate",
      params: {
        schema: "s",
        table: "t",
        column: "c",
        value: null,
        primary_keys: ["id"],
        row: [["id", 1]],
      },
    });
  });

  it("buildUpdate tags a JSON object value", async () => {
    const { client, calls } = recording({ kind: "sqlWithBindings", data: { sql: "UPDATE" } });
    await client.buildUpdate("s", "t", "doc", { a: [1] }, ["id"], { id: 1 });
    expect((calls[0].request as { params: { value: unknown } }).params.value).toEqual({
      $sq: "json",
      v: { a: [1] },
    });
  });

  it("buildSetDefault", async () => {
    const { client, calls } = recording({
      kind: "sqlWithBindings",
      data: { sql: "SET DEFAULT", bindValues: [{ $sq: "bytes", v: "AQI=" }] },
    });
    const out = await client.buildSetDefault("s", "t", "c", ["id"], { c: 5, id: 2 });
    expect(out.bindValues).toEqual([new Uint8Array([1, 2])]);
    expect(calls[0].request).toEqual({
      method: "buildSetDefault",
      params: { schema: "s", table: "t", column: "c", primary_keys: ["id"], row: [["id", 2]] },
    });
    await client.buildSetDefault("s", "t", "c", ["id"], { id: "u" }, { id: "uuid" });
    expect((calls[1].request as { params: { casts: unknown } }).params.casts).toEqual({
      id: "uuid",
    });
  });

  it("buildInsert sends every value in order", async () => {
    const { client, calls } = recording({
      kind: "sqlWithBindings",
      data: { sql: "INSERT", bindValues: [{ $sq: "decimal", v: "12.50" }, null] },
    });
    const out = await client.buildInsert("s", "t", { b: 2n, a: null }, { b: "int8" });
    expect(out.bindValues).toEqual([new SqlDecimal("12.50"), null]);
    expect(calls[0].request).toEqual({
      method: "buildInsert",
      params: {
        schema: "s",
        table: "t",
        values: [
          ["b", { $sq: "bigint", v: "2" }],
          ["a", null],
        ],
        casts: { b: "int8" },
      },
    });
  });

  it("buildDelete", async () => {
    const { client, calls } = recording({
      kind: "sqlWithBindings",
      data: { sql: "DELETE", bindValues: [1] },
    });
    expect(await client.buildDelete("s", "t", ["id"], { id: 1, other: "x" })).toEqual({
      sql: "DELETE",
      bindValues: [1],
    });
    expect(calls[0].request).toEqual({
      method: "buildDelete",
      params: { schema: "s", table: "t", primary_keys: ["id"], row: [["id", 1]] },
    });
    await client.buildDelete("s", "t", ["id"], { id: "u" }, { id: "uuid" });
    expect(calls[1].request).toEqual({
      method: "buildDelete",
      params: {
        schema: "s",
        table: "t",
        primary_keys: ["id"],
        row: [["id", "u"]],
        casts: { id: "uuid" },
      },
    });
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

describe("RustEngineClient.paginate (computed locally)", () => {
  it.each(paginateFixture.cases)(
    "matches PostgresDialect::paginate: $name",
    async ({ input, output }) => {
      const { client, transport } = recording({ kind: "sql", data: "from the server" });
      expect(await client.paginate(input.sql, input.limit, input.offset)).toBe(output);
      expect(transport).not.toHaveBeenCalled();
    },
  );

  it("needs no connection", async () => {
    const { client, transport } = recording({ kind: "sql", data: "x" }, null);
    expect(await client.paginate("SELECT 1", 11, 20)).toBe("SELECT 1 LIMIT 11 OFFSET 20");
    expect(transport).not.toHaveBeenCalled();
  });

  it.each([
    [1.5, 0],
    [-1, 0],
    [10, Number.NaN],
    [10, 2 ** 53],
  ])("rejects limit %s / offset %s (not a non-negative safe integer)", async (limit, offset) => {
    const { client, transport } = recording({ kind: "sql", data: "x" });
    await expect(client.paginate("SELECT 1", limit, offset)).rejects.toThrow("INVALID_ARGUMENT");
    expect(transport).not.toHaveBeenCalled();
  });

  it("other methods still go through the transport", async () => {
    const { client, transport } = recording({ kind: "sql", data: "CREATE" });
    await client.paginate("SELECT 1", 1, 0);
    expect(await client.createTable(def)).toBe("CREATE");
    expect(transport).toHaveBeenCalledTimes(1);
  });
});

describe("RustEngineClient connection id", () => {
  it("reads the id on every call, so a reconnect is picked up", async () => {
    let id: string | undefined = "old";
    const transport = vi.fn((call: EngineCall) =>
      Promise.resolve<EngineResponse>({ kind: "schemas", data: [call.connection_id] }),
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

describe("tauriTransport", () => {
  beforeEach(() => {
    // Braces matter: a function returned from beforeEach runs as its teardown.
    invoke.mockReset();
  });

  it('invokes "db_engine" with { call }', async () => {
    invoke.mockResolvedValue({ kind: "schemas", data: ["public"] });
    const client = new RustEngineClient("postgres", () => "pc-1", tauriTransport);
    expect(await client.listSchemas()).toEqual(["public"]);
    expect(invoke).toHaveBeenCalledWith("db_engine", {
      call: { connection_id: "pc-1", request: { method: "listSchemas" } },
    });
  });

  it("maps a DbError rejection to CODE: message", async () => {
    invoke.mockRejectedValue({ code: "NOT_SUPPORTED", message: "no dialect" });
    const client = new RustEngineClient("postgres", () => "pc-1", tauriTransport);
    await expect(client.statistics()).rejects.toThrow("NOT_SUPPORTED: no dialect");
  });

  it("keeps a plain string rejection (argument deserialization failure)", async () => {
    invoke.mockRejectedValue("invalid args `call` for command `db_engine`");
    const client = new RustEngineClient("postgres", () => "pc-1", tauriTransport);
    await expect(client.statistics()).rejects.toThrow("invalid args `call`");
  });
});

describe("httpTransport", () => {
  const fetchMock = vi.fn<typeof fetch>();
  beforeEach(() => {
    fetchMock.mockReset();
    vi.stubGlobal("fetch", fetchMock);
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("POSTs the call as JSON to /api/db/engine", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ kind: "sql", data: "CREATE" })));
    const client = new RustEngineClient(
      "postgres",
      () => "pc-1",
      httpTransport("https://example.test"),
    );
    expect(await client.createTable(def)).toBe("CREATE");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("https://example.test/api/db/engine");
    expect(init?.method).toBe("POST");
    expect(JSON.parse(init?.body as string)).toEqual({
      connection_id: "pc-1",
      request: { method: "createTable", params: { definition: def } },
    });
  });

  it("uses same-origin URLs by default", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ kind: "schemas", data: [] })));
    await new RustEngineClient("postgres", () => "pc-1", httpTransport()).listSchemas();
    expect(fetchMock.mock.calls[0][0]).toBe("/api/db/engine");
  });

  it.each([
    [404, "CONNECTION_NOT_FOUND", "CONNECTION_NOT_FOUND: gone"],
    [501, "NOT_SUPPORTED", "NOT_SUPPORTED: no dialect"],
  ])("maps a %i DbError body to CODE: message", async (status, code, expected) => {
    const message = code === "NOT_SUPPORTED" ? "no dialect" : "gone";
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ code, message }), { status }));
    const client = new RustEngineClient("postgres", () => "pc-1", httpTransport(""));
    await expect(client.schemaTables()).rejects.toThrow(expected);
  });

  it("maps a non-JSON error body (axum 422, proxy 403) to HTTP_<status>", async () => {
    fetchMock.mockResolvedValue(
      new Response("Failed to deserialize the JSON body", {
        status: 422,
        statusText: "Unprocessable Entity",
      }),
    );
    const client = new RustEngineClient("postgres", () => "pc-1", httpTransport(""));
    await expect(client.schemaTables()).rejects.toThrow("HTTP_422: Unprocessable Entity");
  });

  it("maps a network failure to NETWORK_ERROR", async () => {
    fetchMock.mockRejectedValue(new TypeError("Failed to fetch"));
    const client = new RustEngineClient("postgres", () => "pc-1", httpTransport(""));
    await expect(client.schemaTables()).rejects.toThrow("NETWORK_ERROR: Failed to fetch");
  });
});

describe("default transport", () => {
  afterEach(() => {
    env.tauri = false;
    invoke.mockReset();
    vi.unstubAllGlobals();
  });

  it("uses Tauri IPC on desktop", async () => {
    env.tauri = true;
    invoke.mockResolvedValue({ kind: "schemas", data: [] });
    await new RustEngineClient("postgres", () => "pc-1").listSchemas();
    expect(invoke).toHaveBeenCalledWith("db_engine", expect.anything());
  });

  it("uses HTTP on web", async () => {
    const fetchMock = vi.fn<typeof fetch>(() =>
      Promise.resolve(new Response(JSON.stringify({ kind: "schemas", data: [] }))),
    );
    vi.stubGlobal("fetch", fetchMock);
    await new RustEngineClient("postgres", () => "pc-1").listSchemas();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(invoke).not.toHaveBeenCalled();
  });
});
