import { afterEach, describe, expect, it, vi } from "vitest";
import type { EngineRequest } from "$lib/types/generated/EngineRequest";
import type { EngineResponse } from "$lib/types/generated/EngineResponse";
import type { CreateTableDefinition } from "$lib/types";
import { SqlDecimal } from "$lib/values";
import paginateFixture from "../../../crates/seaquel-engine-postgres/tests/fixtures/paginate.json";
import mysqlPaginateFixture from "../../../crates/seaquel-engine-mysql/tests/fixtures/mysql/paginate.json";
import sqlitePaginateFixture from "../../../crates/seaquel-engine-sqlite/tests/fixtures/paginate.json";
import mssqlPaginateFixture from "../../../crates/seaquel-engine-mssql/tests/fixtures/paginate.json";
import mssqlBugfixes from "../../../crates/seaquel-engine-mssql/tests/fixtures/bugfixes.json";
import duckdbPaginateFixture from "../../../crates/seaquel-engine-duckdb/tests/fixtures/paginate.json";
import duckdbBugfixes from "../../../crates/seaquel-engine-duckdb/tests/fixtures/bugfixes.json";

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
    // SQLite: the column's default expression goes along as column_default.
    await client.buildSetDefault("s", "t", "c", ["id"], { id: 2 }, undefined, "CURRENT_TIMESTAMP");
    expect(calls[2].request).toEqual({
      method: "buildSetDefault",
      params: {
        schema: "s",
        table: "t",
        column: "c",
        column_default: "CURRENT_TIMESTAMP",
        primary_keys: ["id"],
        row: [["id", 2]],
      },
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

  // MariaDB connections use the MySQL engine, so both follow MysqlDialect::paginate.
  it.each(
    (["mysql", "mariadb"] as const).flatMap((engine) =>
      mysqlPaginateFixture.cases.map((c) => ({ engine, ...c })),
    ),
  )("matches MysqlDialect::paginate for $engine: $name", async ({ engine, input, output }) => {
    const transport = vi.fn();
    const client = new RustEngineClient(engine, () => "pc-1", transport);
    expect(await client.paginate(input.sql, input.limit, input.offset)).toBe(output);
    expect(transport).not.toHaveBeenCalled();
  });

  it.each(sqlitePaginateFixture.cases)(
    "matches SqliteDialect::paginate: $name",
    async ({ input, output }) => {
      const transport = vi.fn();
      const client = new RustEngineClient("sqlite", () => "pc-1", transport);
      expect(await client.paginate(input.sql, input.limit, input.offset)).toBe(output);
      expect(transport).not.toHaveBeenCalled();
    },
  );

  // As tests/dialect_parity.rs reads them: the recorded cases, with the
  // bug-fix outputs of fixes 8 and 14 swapped in, plus the hand-written ones.
  const mssqlPaginateCases = (() => {
    const fixes = mssqlBugfixes.cases.filter((c) => c.kind === "paginate");
    const replaced = new Map(
      fixes.flatMap((c) =>
        c.replaces?.startsWith("paginate.json: ")
          ? [[c.replaces.slice("paginate.json: ".length), c.output as string] as const]
          : [],
      ),
    );
    const recorded = mssqlPaginateFixture.cases.map((c) => ({
      name: c.name,
      input: c.input,
      output: replaced.get(c.name) ?? c.output,
      recorded: c.output,
    }));
    const handWritten = fixes
      .filter((c) => !c.replaces)
      .map((c) => ({
        name: `[fix ${c.fix}] ${c.name}`,
        input: c.input as unknown as { sql: string; limit: number; offset: number },
        output: c.output as string,
        recorded: undefined as string | undefined,
      }));
    return { recorded, handWritten, replacedCount: replaced.size };
  })();

  it("reads every MSSQL pagination case", () => {
    const { recorded, handWritten, replacedCount } = mssqlPaginateCases;
    expect(recorded).toHaveLength(14);
    expect(replacedCount).toBe(7);
    expect(recorded.filter((c) => c.output !== c.recorded)).toHaveLength(7);
    expect(handWritten).toHaveLength(18);
  });

  it.each([...mssqlPaginateCases.recorded, ...mssqlPaginateCases.handWritten])(
    "matches MssqlDialect::paginate: $name",
    async ({ input, output }) => {
      const transport = vi.fn();
      const client = new RustEngineClient("mssql", () => "pc-1", transport);
      expect(await client.paginate(input.sql, input.limit, input.offset)).toBe(output);
      expect(transport).not.toHaveBeenCalled();
    },
  );

  // dialect.rs unit tests: code points, not UTF-16 units; unclosed strings and comments.
  it.each([
    [
      "SELECT [名前] FROM t; -- é",
      5,
      0,
      "SELECT [名前] FROM t ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 5 ROWS ONLY",
    ],
    [
      "SELECT 名 FROM t ORDER BY 名",
      5,
      10,
      "SELECT 名 FROM t ORDER BY 名 OFFSET 10 ROWS FETCH NEXT 5 ROWS ONLY",
    ],
    [
      "SELECT '🚀' AS [x🚀]\u0085;\u00a0",
      1,
      0,
      "SELECT '🚀' AS [x🚀] ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 1 ROWS ONLY",
    ],
    [
      "SELECT 'ORDER BY",
      1,
      0,
      "SELECT 'ORDER BY ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 1 ROWS ONLY",
    ],
    [
      "SELECT 1 /* ORDER BY",
      1,
      0,
      "SELECT 1 ORDER BY (SELECT NULL) OFFSET 0 ROWS FETCH NEXT 1 ROWS ONLY",
    ],
  ])("MSSQL pagination of %j", async (sql, limit, offset, output) => {
    const client = new RustEngineClient("mssql", () => "pc-1", vi.fn());
    expect(await client.paginate(sql, limit, offset)).toBe(output);
  });

  // DuckDB has no paginate bug fixes.
  it("has no DuckDB paginate bug fixes", () => {
    expect(duckdbBugfixes.cases.filter((c) => c.kind === "paginate")).toHaveLength(0);
  });

  it.each(duckdbPaginateFixture.cases)(
    "matches DuckdbDialect::paginate: $name",
    async ({ input, output }) => {
      const transport = vi.fn();
      const client = new RustEngineClient("duckdb", () => "pc-1", transport);
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
