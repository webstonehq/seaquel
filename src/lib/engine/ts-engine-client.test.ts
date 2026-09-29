import { describe, expect, it, vi } from "vitest";
import { getAdapter, type DatabaseAdapter } from "$lib/db";
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

/** The adapter's CRUD builders (the demo's `TsEditService` calls them, not the engine client). */
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
    getSchemasQuery: () => "SCHEMAS",
    getColumnsQuery: (t, s) => `COLUMNS ${s}.${t}`,
    getIndexesQuery: (t, s) => `INDEXES ${s}.${t}`,
    getForeignKeysQuery: (t, s) => `FKS ${s}.${t}`,
    getTableSizesQuery: () => "SIZES",
    getIndexUsageQuery: () => "USAGE",
    getDatabaseOverviewQuery: () => "OVERVIEW",
    getTableRowCountQuery: (t, s) => `COUNT ${s}.${t}`,
    parseTableSizesResult: (rows) => rows as never,
    parseIndexUsageResult: (rows) => rows as never,
    parseDatabaseOverviewResult: (rows) => ({ ...(rows[0] as object) }) as never,
    getColumnTypes: () => [],
    generateCreateTableSql: () => "CREATE",
    generateAddColumnSql: () => "ADD COLUMN",
    generateAlterTableSql: () => "ALTER",
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
  opts: { id?: string | null } = {},
) {
  return new TsEngineClient({
    type: "duckdb",
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
    expect(await client(fakeAdapter(), provider).listSchemas()).toEqual(["public", "app"]);
    expect(select).toHaveBeenCalledWith("pc-1", "SCHEMAS");
  });

  it("tableMetadata runs columns, indexes and foreign keys, and parses them", async () => {
    const { provider, select } = fakeProvider({
      "COLUMNS main.t": [{ c: 1 }],
      "INDEXES main.t": [{ i: 1 }],
      "FKS main.t": [{ fk: 1 }],
    });
    const meta = await client(fakeAdapter(), provider).tableMetadata("main", "t");
    expect(select.mock.calls.map((c) => c[1])).toEqual([
      "COLUMNS main.t",
      "INDEXES main.t",
      "FKS main.t",
    ]);
    expect(meta).toEqual({
      columns: [{ rows: [{ c: 1 }], fks: [{ fk: 1 }] }],
      indexes: [{ rows: [{ i: 1 }] }],
    });
  });

  it("columnTypes returns the adapter list", async () => {
    const { provider } = fakeProvider();
    const types = [{ name: "int", category: "Numeric" }] as never;
    expect(await client(fakeAdapter({ getColumnTypes: () => types }), provider).columnTypes()).toBe(
      types,
    );
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
  it("runs the three queries, parses each and counts rows per table", async () => {
    const { provider, select } = fakeProvider({
      SIZES: [
        { name: "a", schema: "main", rowCount: 0 },
        { name: "b", schema: "main", rowCount: 0 },
        { name: "c", schema: "main", rowCount: 7 },
      ],
      USAGE: [{ indexName: "i" }],
      OVERVIEW: [{ databaseName: "db", totalSize: "1 MB", tableCount: 3, indexCount: 1 }],
      "COUNT main.a": [{ row_count: 42 }],
      "COUNT main.b": [{ row_count: "not a number" }],
      "COUNT main.c": new Error("no such table"),
    });
    const stats = await client(fakeAdapter(), provider).statistics();
    expect(select.mock.calls.map((c) => c.slice(0, 2))).toEqual([
      ["pc-1", "SIZES"],
      ["pc-1", "USAGE"],
      ["pc-1", "OVERVIEW"],
      ["pc-1", "COUNT main.a"],
      ["pc-1", "COUNT main.b"],
      ["pc-1", "COUNT main.c"],
    ]);
    expect(stats).toEqual({
      overview: { databaseName: "db", totalSize: "1 MB", tableCount: 3, indexCount: 1 },
      tableSizes: [
        { name: "a", schema: "main", rowCount: 42 },
        { name: "b", schema: "main", rowCount: 0 },
        // A failed count keeps the original entry.
        { name: "c", schema: "main", rowCount: 7 },
      ],
      indexUsage: [{ indexName: "i" }],
    });
  });

  it("throws without a provider connection id", async () => {
    const { provider } = fakeProvider();
    await expect(client(fakeAdapter(), provider, { id: null }).statistics()).rejects.toThrow(
      "No connection established",
    );
  });
});

describe("TsEngineClient.explain", () => {
  it("selects the EXPLAIN query without the bind values and parses it", async () => {
    const { provider, select } = fakeProvider({ "EXPLAIN ANALYZE SELECT ?": [{ plan: 1 }] });
    const result = await client(fakeAdapter(), provider).explain("SELECT ?", [5], true);
    expect(select.mock.calls).toEqual([["pc-1", "EXPLAIN ANALYZE SELECT ?"]]);
    expect(result).toEqual({
      plan: { id: "0", nodeType: "rows:1", children: [] },
      planningTime: 1,
      isAnalyze: true,
    });
  });

  it("throws without a provider connection id", async () => {
    const { provider } = fakeProvider();
    await expect(
      client(fakeAdapter(), provider, { id: null }).explain("SELECT 1", undefined, false),
    ).rejects.toThrow("No connection established");
  });
});

describe("TsEngineClient DDL builders", () => {
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
});

describe("TsEngineClient default adapter", () => {
  it("uses getAdapter(type) when no adapter is given", async () => {
    const { provider } = fakeProvider();
    const c = new TsEngineClient({
      type: "duckdb",
      getConnectionId: () => "pc",
      getProvider: () => Promise.resolve(provider),
    });
    expect(await c.columnTypes()).toEqual(getAdapter("duckdb").getColumnTypes());
  });

  it("can be made for an engine without an adapter, and throws on first use", async () => {
    const c = new TsEngineClient({ type: "sqlite", getConnectionId: () => "pc" });
    await expect(c.columnTypes()).rejects.toThrow('Database type "sqlite" is not supported yet');
  });
});

describe("TsEngineClient.qualifiedTable", () => {
  it("keeps the data tab's quoting (the demo's DuckDB lists schemas bare)", () => {
    const c = (type: "duckdb" | "mysql") =>
      new TsEngineClient({ type, getConnectionId: () => undefined });
    expect(c("duckdb").qualifiedTable("main", "order items")).toBe('"main"."order items"');
    expect(c("mysql").qualifiedTable("db", "t")).toBe("`db`.`t`");
  });
});

describe("TsEngineClient connection id", () => {
  it("reads the id on every call, so a reconnect is picked up", async () => {
    const { provider, select } = fakeProvider();
    let id: string | undefined = "old";
    const c = new TsEngineClient({
      type: "duckdb",
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
