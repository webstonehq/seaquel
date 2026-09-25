import { describe, expect, it, vi } from "vitest";
import type { DatabaseAdapter } from "$lib/db";
import type { DatabaseProvider } from "$lib/providers/types";
import type { CreateTableDefinition } from "$lib/types";
import { TsEngineClient } from "./ts-engine-client";

type Rows = Record<string, unknown>[];

/** Provider fake: `select` answers from a SQL → rows table and records every call. */
function fakeProvider(answers: Record<string, Rows | Error> = {}) {
  const select = vi.fn((_id: string, sql: string, _params?: unknown[]) => {
    const answer = answers[sql];
    if (answer instanceof Error) return Promise.reject(answer);
    return Promise.resolve(answer ?? []);
  });
  const provider = { select } as unknown as DatabaseProvider;
  return { provider, select };
}

/** The CRUD builders as standalone spies (so assertions don't reference unbound methods). */
function builderSpies() {
  return {
    buildUpdateSql: vi.fn<DatabaseAdapter["buildUpdateSql"]>(() => ({
      sql: "UPDATE",
      bindValues: [1],
    })),
    buildSetDefaultSql: vi.fn<DatabaseAdapter["buildSetDefaultSql"]>(() => ({
      sql: "SET DEFAULT",
      bindValues: [2],
    })),
    buildInsertSql: vi.fn<DatabaseAdapter["buildInsertSql"]>(() => ({
      sql: "INSERT",
      bindValues: [3],
    })),
    buildDeleteSql: vi.fn<DatabaseAdapter["buildDeleteSql"]>(() => ({ sql: "DELETE" })),
  };
}

/** Adapter fake: every query getter returns a recognisable string; parsers echo their input. */
function fakeAdapter(overrides: Partial<DatabaseAdapter> = {}): DatabaseAdapter {
  const base: DatabaseAdapter = {
    getSchemaQuery: () => "SCHEMA",
    getColumnsQuery: (t, s) => `COLUMNS ${s}.${t}`,
    getIndexesQuery: (t, s) => `INDEXES ${s}.${t}`,
    getExplainQuery: (q, analyze) => `EXPLAIN${analyze ? " ANALYZE" : ""} ${q}`,
    parseExplainResult: (rows, analyze) => ({
      plan: { id: "0", nodeType: `rows:${rows.length}`, children: [] },
      planningTime: 1,
      isAnalyze: analyze,
    }),
    parseSchemaResult: (rows) => rows as never,
    parseColumnsResult: (rows, fks) => [{ rows, fks }] as never,
    parseIndexesResult: (rows) => [{ rows }] as never,
    quoteIdentifier: (id) => `"${id}"`,
    paginateQuery: (q, l, o) => `${q} LIMIT ${l} OFFSET ${o}`,
    ...builderSpies(),
  };
  return { ...base, ...overrides };
}

function client(
  adapter: DatabaseAdapter,
  provider: DatabaseProvider,
  opts: { type?: "postgres" | "sqlite" | "mssql" | "duckdb" | "mysql"; id?: string | null } = {},
) {
  return new TsEngineClient({
    type: opts.type ?? "postgres",
    connectionName: "My DB",
    getConnectionId: () => (opts.id === null ? undefined : (opts.id ?? "pc-1")),
    adapter,
    getProvider: () => Promise.resolve(provider),
  });
}

const def: CreateTableDefinition = {
  tableName: "t",
  schemaName: "public",
  columns: [],
  indexes: [],
  foreignKeys: [],
};

describe("TsEngineClient introspection", () => {
  it("schemaTables selects the schema query and parses it", async () => {
    const rows = [{ name: "a" }];
    const { provider, select } = fakeProvider({ SCHEMA: rows });
    const tables = await client(fakeAdapter(), provider).schemaTables();
    expect(select).toHaveBeenCalledWith("pc-1", "SCHEMA");
    expect(tables).toEqual(rows);
  });

  it("listSchemas runs getSchemasQuery and maps schema_name", async () => {
    const { provider, select } = fakeProvider({
      SCHEMAS: [{ schema_name: "public" }, { schema_name: "app" }],
    });
    const c = client(fakeAdapter({ getSchemasQuery: () => "SCHEMAS" }), provider);
    expect(await c.listSchemas()).toEqual(["public", "app"]);
    expect(select).toHaveBeenCalledWith("pc-1", "SCHEMAS");
  });

  it("listSchemas is empty without getSchemasQuery or with an empty query", async () => {
    const { provider, select } = fakeProvider();
    expect(await client(fakeAdapter(), provider).listSchemas()).toEqual([]);
    expect(
      await client(fakeAdapter({ getSchemasQuery: () => "" }), provider).listSchemas(),
    ).toEqual([]);
    expect(select).not.toHaveBeenCalled();
  });

  it("tableMetadata runs columns, then indexes, without a foreign-key query", async () => {
    const { provider, select } = fakeProvider({
      "COLUMNS public.t": [{ c: 1 }],
      "INDEXES public.t": [{ i: 1 }],
    });
    const meta = await client(fakeAdapter(), provider).tableMetadata("public", "t");
    expect(select.mock.calls.map((c) => c[1])).toEqual(["COLUMNS public.t", "INDEXES public.t"]);
    expect(meta).toEqual({
      columns: [{ rows: [{ c: 1 }], fks: undefined }],
      indexes: [{ rows: [{ i: 1 }] }],
    });
  });

  it("tableMetadata passes foreign-key rows to parseColumnsResult when the adapter has them", async () => {
    const { provider, select } = fakeProvider({ "FKS main.t": [{ fk: 1 }] });
    const adapter = fakeAdapter({ getForeignKeysQuery: (t, s) => `FKS ${s}.${t}` });
    const meta = await client(adapter, provider).tableMetadata("main", "t");
    expect(select.mock.calls.map((c) => c[1])).toEqual([
      "COLUMNS main.t",
      "INDEXES main.t",
      "FKS main.t",
    ]);
    expect(meta.columns).toEqual([{ rows: [], fks: [{ fk: 1 }] }]);
  });

  it("columnTypes returns the adapter list, or [] without one", async () => {
    const { provider } = fakeProvider();
    const types = [{ name: "int", category: "Numeric" }] as never;
    expect(await client(fakeAdapter({ getColumnTypes: () => types }), provider).columnTypes()).toBe(
      types,
    );
    expect(await client(fakeAdapter(), provider).columnTypes()).toEqual([]);
  });

  it("throws when the connection has no provider connection id", async () => {
    const { provider } = fakeProvider();
    await expect(client(fakeAdapter(), provider, { id: null }).schemaTables()).rejects.toThrow(
      "No connection established",
    );
  });

  it("propagates provider errors", async () => {
    const { provider } = fakeProvider({ SCHEMA: new Error("QUERY_ERROR: boom") });
    await expect(client(fakeAdapter(), provider).schemaTables()).rejects.toThrow(
      "QUERY_ERROR: boom",
    );
  });
});

describe("TsEngineClient.statistics", () => {
  const statsAdapter = (extra: Partial<DatabaseAdapter> = {}) =>
    fakeAdapter({
      getTableSizesQuery: () => "SIZES",
      getIndexUsageQuery: () => "USAGE",
      getDatabaseOverviewQuery: () => "OVERVIEW",
      parseTableSizesResult: (rows) => rows as never,
      parseIndexUsageResult: (rows) => rows as never,
      parseDatabaseOverviewResult: (rows) => ({ ...(rows[0] as object) }) as never,
      ...extra,
    });

  it("runs the three queries and parses each", async () => {
    const { provider, select } = fakeProvider({
      SIZES: [{ name: "a", schema: "s", rowCount: 5 }],
      USAGE: [{ indexName: "i" }],
      OVERVIEW: [{ databaseName: "db", totalSize: "1 MB", tableCount: 1, indexCount: 1 }],
    });
    const stats = await client(statsAdapter(), provider).statistics();
    expect(select.mock.calls.map((c) => c.slice(0, 2))).toEqual([
      ["pc-1", "SIZES"],
      ["pc-1", "USAGE"],
      ["pc-1", "OVERVIEW"],
    ]);
    expect(stats).toEqual({
      overview: { databaseName: "db", totalSize: "1 MB", tableCount: 1, indexCount: 1 },
      tableSizes: [{ name: "a", schema: "s", rowCount: 5 }],
      indexUsage: [{ indexName: "i" }],
    });
  });

  it("falls back to the connection name and empty lists without statistics support", async () => {
    const { provider, select } = fakeProvider();
    expect(await client(fakeAdapter(), provider).statistics()).toEqual({
      overview: { databaseName: "My DB", totalSize: "N/A", tableCount: 0, indexCount: 0 },
      tableSizes: [],
      indexUsage: [],
    });
    expect(select).not.toHaveBeenCalled();
  });

  it("fills row counts per table when the adapter has getTableRowCountQuery", async () => {
    const { provider } = fakeProvider({
      SIZES: [
        { name: "a", schema: "main", rowCount: 0 },
        { name: "b", schema: "main", rowCount: 0 },
        { name: "c", schema: "main", rowCount: 7 },
      ],
      "COUNT main.a": [{ row_count: 42 }],
      "COUNT main.b": [{ row_count: "not a number" }],
      "COUNT main.c": new Error("no such table"),
    });
    const adapter = statsAdapter({ getTableRowCountQuery: (t, s) => `COUNT ${s}.${t}` });
    const stats = await client(adapter, provider, { type: "sqlite" }).statistics();
    expect(stats.tableSizes).toEqual([
      { name: "a", schema: "main", rowCount: 42 },
      { name: "b", schema: "main", rowCount: 0 },
      // A failed count keeps the original entry.
      { name: "c", schema: "main", rowCount: 7 },
    ]);
  });
});

describe("TsEngineClient.explain", () => {
  it("selects the EXPLAIN query with the bind values and parses it", async () => {
    const { provider, select } = fakeProvider({ "EXPLAIN SELECT $1": [{ plan: 1 }] });
    const result = await client(fakeAdapter(), provider).explain("SELECT $1", [5], false);
    expect(select).toHaveBeenCalledTimes(1);
    expect(select).toHaveBeenCalledWith("pc-1", "EXPLAIN SELECT $1", [5]);
    expect(result).toEqual({
      plan: { id: "0", nodeType: "rows:1", children: [] },
      planningTime: 1,
      isAnalyze: false,
    });
  });

  it.each(["mssql", "duckdb"] as const)("%s drops the bind values", async (type) => {
    const { provider, select } = fakeProvider();
    await client(fakeAdapter(), provider, { type }).explain("SELECT ?", [5], true);
    expect(select).toHaveBeenCalledWith("pc-1", "EXPLAIN ANALYZE SELECT ?", undefined);
  });

  it("SQLite ANALYZE runs the query first and records its timing on the root", async () => {
    const { provider, select } = fakeProvider({ "SELECT ?": [{ a: 1 }, { a: 2 }, { a: 3 }] });
    const now = vi.spyOn(performance, "now").mockReturnValueOnce(100).mockReturnValueOnce(112.5);
    const result = await client(fakeAdapter(), provider, { type: "sqlite" }).explain(
      "SELECT ?",
      [1],
      true,
    );
    now.mockRestore();
    expect(select.mock.calls).toEqual([
      ["pc-1", "SELECT ?", [1]],
      ["pc-1", "EXPLAIN ANALYZE SELECT ?", [1]],
    ]);
    expect(result.plan.actualRows).toBe(3);
    expect(result.plan.actualTotalTime).toBe(12.5);
    expect(result.executionTime).toBe(12.5);
    expect(result.plan.planRows).toBeUndefined();
  });

  it("SQLite plain EXPLAIN runs only the EXPLAIN", async () => {
    const { provider, select } = fakeProvider();
    await client(fakeAdapter(), provider, { type: "sqlite" }).explain("SELECT 1", undefined, false);
    expect(select.mock.calls).toEqual([["pc-1", "EXPLAIN SELECT 1", undefined]]);
  });

  it("throws without a provider connection id", async () => {
    const { provider } = fakeProvider();
    await expect(
      client(fakeAdapter(), provider, { id: null }).explain("SELECT 1", undefined, false),
    ).rejects.toThrow("No connection established");
  });
});

describe("TsEngineClient SQL builders", () => {
  it("paginate calls paginateQuery", async () => {
    const { provider } = fakeProvider();
    expect(await client(fakeAdapter(), provider).paginate("SELECT 1", 101, 200)).toBe(
      "SELECT 1 LIMIT 101 OFFSET 200",
    );
  });

  it("buildUpdate passes everything through and turns the cast map into a lookup", async () => {
    const { provider } = fakeProvider();
    const spies = builderSpies();
    const adapter = fakeAdapter(spies);
    const row = { id: 10n, name: "x" };
    const out = await client(adapter, provider).buildUpdate(
      "public",
      "t",
      "name",
      "y",
      ["id"],
      row,
      { id: "bigint" },
    );
    expect(out).toEqual({ sql: "UPDATE", bindValues: [1] });
    const call = spies.buildUpdateSql.mock.calls[0];
    expect(call.slice(0, 6)).toEqual(["public", "t", "name", "y", ["id"], row]);
    const lookup = call[6]!;
    expect(lookup("id")).toBe("bigint");
    expect(lookup("name")).toBeUndefined();
    // Inherited keys are not casts.
    expect(lookup("toString")).toBeUndefined();
  });

  it("buildUpdate without casts passes no lookup", async () => {
    const { provider } = fakeProvider();
    const spies = builderSpies();
    await client(fakeAdapter(spies), provider).buildUpdate("s", "t", "c", 1, ["id"], { id: 1 });
    expect(spies.buildUpdateSql.mock.calls[0][6]).toBeUndefined();
  });

  it("buildSetDefault, buildInsert and buildDelete call their adapter builders", async () => {
    const { provider } = fakeProvider();
    const spies = builderSpies();
    const c = client(fakeAdapter(spies), provider);
    expect(await c.buildSetDefault("s", "t", "c", ["id"], { id: 1 })).toEqual({
      sql: "SET DEFAULT",
      bindValues: [2],
    });
    expect(spies.buildSetDefaultSql).toHaveBeenCalledWith("s", "t", "c", ["id"], { id: 1 });

    expect(await c.buildInsert("s", "t", { a: 1 }, { a: "int" })).toEqual({
      sql: "INSERT",
      bindValues: [3],
    });
    const insertCall = spies.buildInsertSql.mock.calls[0];
    expect(insertCall.slice(0, 3)).toEqual(["s", "t", { a: 1 }]);
    expect(insertCall[3]!("a")).toBe("int");

    expect(await c.buildDelete("s", "t", ["id"], { id: 1 })).toEqual({ sql: "DELETE" });
    expect(spies.buildDeleteSql).toHaveBeenCalledWith("s", "t", ["id"], { id: 1 });
  });

  it("builders work without a provider connection id", async () => {
    const { provider } = fakeProvider();
    const c = client(fakeAdapter(), provider, { id: null });
    expect(await c.paginate("q", 1, 0)).toBe("q LIMIT 1 OFFSET 0");
    expect(await c.buildDelete("s", "t", ["id"], { id: 1 })).toEqual({ sql: "DELETE" });
  });

  it("createTable and alterTable call the DDL generators", async () => {
    const { provider } = fakeProvider();
    const generateCreateTableSql = vi.fn(() => "CREATE");
    const generateAlterTableSql = vi.fn(() => "ALTER");
    const c = client(fakeAdapter({ generateCreateTableSql, generateAlterTableSql }), provider);
    const to = { ...def, tableName: "u" };
    expect(await c.createTable(def)).toBe("CREATE");
    expect(generateCreateTableSql).toHaveBeenCalledWith(def);
    expect(await c.alterTable(def, to)).toBe("ALTER");
    expect(generateAlterTableSql).toHaveBeenCalledWith(def, to);
  });

  it("createTable and alterTable reject when the adapter has no generator", async () => {
    const { provider } = fakeProvider();
    const c = client(fakeAdapter(), provider);
    await expect(c.createTable(def)).rejects.toThrow("not supported");
    await expect(c.alterTable(def, def)).rejects.toThrow("not supported");
  });
});

describe("TsEngineClient default adapter", () => {
  it("uses getAdapter(type) when no adapter is given", async () => {
    const { provider } = fakeProvider();
    const c = new TsEngineClient({
      type: "sqlite",
      connectionName: "x",
      getConnectionId: () => "pc",
      getProvider: () => Promise.resolve(provider),
    });
    expect(await c.paginate("SELECT 1", 10, 20)).toBe("SELECT 1 LIMIT 10 OFFSET 20");
  });
});

describe("TsEngineClient connection id", () => {
  it("reads the id on every call, so a reconnect is picked up", async () => {
    const { provider, select } = fakeProvider();
    let id: string | undefined = "old";
    const c = new TsEngineClient({
      type: "postgres",
      connectionName: "x",
      getConnectionId: () => id,
      adapter: fakeAdapter(),
      getProvider: () => Promise.resolve(provider),
    });
    await c.schemaTables();
    id = undefined; // disconnected
    await expect(c.schemaTables()).rejects.toThrow("No connection established");
    id = "new"; // reconnected
    await c.schemaTables();
    expect(select.mock.calls.map((call) => call[0])).toEqual(["old", "new"]);
  });
});
