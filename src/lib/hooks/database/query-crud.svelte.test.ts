import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SchemaColumn, SchemaTable } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";

const client = {
  buildUpdate: vi.fn(),
  buildSetDefault: vi.fn(),
  buildInsert: vi.fn(),
  buildDelete: vi.fn(),
  tableMetadata: vi.fn(),
};
const getEngineClient = vi.fn((..._args: unknown[]) => client);
const usesRustEngine = vi.fn((c: { type: string }) => c.type === "postgres");
vi.mock("$lib/engine", () => ({
  getEngineClient: (...args: unknown[]) => getEngineClient(...args),
  usesRustEngine: (c: { type: string }) => usesRustEngine(c),
}));
const debug = vi.fn();
vi.mock("$lib/utils/logger", () => ({
  log: { debug: (...args: unknown[]) => debug(...args), error: vi.fn(), info: vi.fn() },
}));

const { QueryCrudManager, castMapForColumns } = await import("./query-crud.svelte.js");

function column(name: string, type: string, castType?: string): SchemaColumn {
  return {
    name,
    type,
    ...(castType === undefined ? {} : { castType }),
    nullable: true,
    isPrimaryKey: false,
    isForeignKey: false,
  } as SchemaColumn;
}

const users: SchemaTable = {
  name: "users",
  schema: "public",
  type: "table",
  columns: [
    column("id", "bigint"),
    column("name", "TEXT"),
    column("email", "character varying"),
    column("code", "character"),
    column("mood", "USER-DEFINED"),
    column("tags", "ARRAY"),
    column("balance", "numeric"),
    column("meta", "jsonb"),
  ],
  indexes: [],
} as unknown as SchemaTable;

const connection = {
  id: "conn-1",
  type: "postgres",
  name: "Local",
  providerConnectionId: "pc-1",
};

function makeManager(
  opts: {
    pending?: boolean;
    activeConnectionId?: string | null;
    tables?: SchemaTable[];
    type?: string;
  } = {},
) {
  const active = { ...connection, type: opts.type ?? connection.type };
  const state = {
    activeConnectionId: opts.activeConnectionId === undefined ? "conn-1" : opts.activeConnectionId,
    activeConnection: active,
    connections: [active],
    schemas: { "conn-1": opts.tables ?? [users] },
  } as unknown as DatabaseState;
  const provider = { execute: vi.fn(async () => ({ rowsAffected: 1, lastInsertId: 7 })) };
  const providers = { getForType: vi.fn(async () => provider) } as unknown as ProviderRegistry;
  const addPending = vi.fn();
  const pending = {
    isEnabled: () => opts.pending ?? false,
    add: addPending,
    findForCell: vi.fn(() => undefined),
    update: vi.fn(),
  } as unknown as PendingChangesManager;
  return { manager: new QueryCrudManager(state, providers, pending), state, provider, addPending };
}

describe("castMapForColumns", () => {
  it("casts every column except text, varchar, user-defined and array ones", () => {
    expect(castMapForColumns(users.columns)).toEqual({
      id: "bigint",
      code: "bpchar",
      balance: "numeric",
      meta: "jsonb",
    });
  });

  it("casts length-typed bit and character columns to their unbounded types", () => {
    // information_schema reports `bit`/`character` without the length, and
    // CAST(… AS bit) would be bit(1): the WHERE clause would match nothing.
    expect(
      castMapForColumns([
        column("flags", "bit"),
        column("code", "character"),
        column("mask", "bit varying"),
      ]),
    ).toEqual({ flags: "bit varying", code: "bpchar", mask: "bit varying" });
  });

  it("keeps the column's own type spelling", () => {
    expect(castMapForColumns([column("n", "NUMERIC(10,2)")])).toEqual({ n: "NUMERIC(10,2)" });
  });

  it("casts to castType verbatim when the column has one, for every column", () => {
    // Postgres reports castType (bug fix 7): enums and arrays get their real
    // type, and text columns a harmless text cast.
    expect(
      castMapForColumns([
        column("mood", "USER-DEFINED", 'app."Weird Mood"'),
        column("tags", "ARRAY", "integer[]"),
        column("name", "text", "text"),
        column("email", "character varying", "character varying"),
        column("flags", "bit", '"bit"'),
        column("code", "character", "bpchar"),
        column("n", "numeric", "numeric(10,2)"),
        // No castType (other engines, or an older cache entry): today's rules.
        column("legacy", "USER-DEFINED"),
        column("mask", "bit"),
        column("id", "bigint"),
      ]),
    ).toEqual({
      mood: 'app."Weird Mood"',
      tags: "integer[]",
      name: "text",
      email: "character varying",
      flags: '"bit"',
      code: "bpchar",
      n: "numeric(10,2)",
      mask: "bit varying",
      id: "bigint",
    });
  });
});

describe("QueryCrudManager.buildCastMap", () => {
  const fetched = {
    columns: [column("id", "uuid", "uuid"), column("mood", "USER-DEFINED", "app.mood")],
    indexes: [{ name: "orders_pkey", columns: ["id"], unique: true, type: "btree" }],
  };

  beforeEach(() => {
    getEngineClient.mockClear();
    client.tableMetadata.mockReset();
    debug.mockClear();
  });

  it("returns the active connection's table cast map", async () => {
    const { manager } = makeManager();
    expect(await manager.buildCastMap("public", "users")).toEqual({
      id: "bigint",
      code: "bpchar",
      balance: "numeric",
      meta: "jsonb",
    });
    expect(client.tableMetadata).not.toHaveBeenCalled();
  });

  it("is undefined without an active connection", async () => {
    const { manager } = makeManager({ activeConnectionId: null });
    expect(await manager.buildCastMap("public", "users")).toBe(undefined);
    expect(client.tableMetadata).not.toHaveBeenCalled();
  });

  it("loads the columns of a table missing from the schema cache", async () => {
    client.tableMetadata.mockResolvedValue(fetched);
    const { manager, state } = makeManager();

    expect(await manager.buildCastMap("app", "orders")).toEqual({ id: "uuid", mood: "app.mood" });
    expect(getEngineClient).toHaveBeenCalledWith(state.activeConnection, state);
    expect(client.tableMetadata).toHaveBeenCalledWith("app", "orders");
    // Not in the cache's table list, so there is nothing to update.
    expect(state.schemas["conn-1"]).toEqual([users]);
  });

  it("loads and caches the columns of a cached table without them", async () => {
    client.tableMetadata.mockResolvedValue(fetched);
    const orders = { ...users, name: "orders", schema: "app", columns: [] } as SchemaTable;
    const { manager, state } = makeManager({ tables: [users, orders] });

    expect(await manager.buildCastMap("app", "orders")).toEqual({ id: "uuid", mood: "app.mood" });
    expect(state.schemas["conn-1"]).toEqual([users, { ...orders, ...fetched }]);

    // Cached now: no second fetch.
    expect(await manager.buildCastMap("app", "orders")).toEqual({ id: "uuid", mood: "app.mood" });
    expect(client.tableMetadata).toHaveBeenCalledTimes(1);
  });

  it("falls back to no cast map when loading fails, and logs it at debug level", async () => {
    client.tableMetadata.mockRejectedValue(new Error("connection lost"));
    const { manager, state } = makeManager();

    expect(await manager.buildCastMap("app", "orders")).toBe(undefined);
    expect(debug).toHaveBeenCalledWith(expect.stringContaining("connection lost"));
    expect(state.schemas["conn-1"]).toEqual([users]);
  });

  it("loads a table's columns once for concurrent and later edits", async () => {
    let resolve!: (m: typeof fetched) => void;
    client.tableMetadata.mockReturnValue(new Promise((r) => (resolve = r)));
    const { manager } = makeManager();

    const first = manager.buildCastMap("app", "orders");
    const second = manager.buildCastMap("app", "orders");
    resolve(fetched);
    const casts = { id: "uuid", mood: "app.mood" };
    expect(await first).toEqual(casts);
    expect(await second).toEqual(casts);
    // Not in the schema cache's table list, but remembered all the same.
    expect(await manager.buildCastMap("app", "orders")).toEqual(casts);
    expect(client.tableMetadata).toHaveBeenCalledTimes(1);
  });

  it("loads again after the schema reloads, a reconnect or a failed load", async () => {
    client.tableMetadata.mockResolvedValue(fetched);
    const { manager, state } = makeManager();

    await manager.buildCastMap("app", "orders");
    manager.forgetLoadedColumns("conn-1");
    await manager.buildCastMap("app", "orders");
    expect(client.tableMetadata).toHaveBeenCalledTimes(2);

    // reconnect() replaces the connection with a new provider connection id.
    const reconnected = { ...state.activeConnection!, providerConnectionId: "pc-2" };
    Object.assign(state, { activeConnection: reconnected, connections: [reconnected] });
    await manager.buildCastMap("app", "orders");
    expect(client.tableMetadata).toHaveBeenCalledTimes(3);

    client.tableMetadata.mockRejectedValueOnce(new Error("timeout"));
    manager.forgetLoadedColumns("conn-1");
    expect(await manager.buildCastMap("app", "orders")).toBe(undefined);
    expect(await manager.buildCastMap("app", "orders")).toEqual({ id: "uuid", mood: "app.mood" });
    expect(client.tableMetadata).toHaveBeenCalledTimes(5);
  });

  it("doesn't overwrite columns a schema refresh stored while it was loading", async () => {
    let resolve!: (m: typeof fetched) => void;
    client.tableMetadata.mockReturnValue(new Promise((r) => (resolve = r)));
    const orders = { ...users, name: "orders", schema: "app", columns: [] } as SchemaTable;
    const { manager, state } = makeManager({ tables: [users, orders] });

    const pending = manager.buildCastMap("app", "orders");
    const refreshed = {
      ...orders,
      columns: [column("id", "uuid", "uuid"), column("total", "numeric", "numeric(10,2)")],
    };
    state.schemas = { "conn-1": [users, refreshed] };
    resolve(fetched);
    await pending;

    expect(state.schemas["conn-1"]).toEqual([users, refreshed]);
  });

  // Task 1 (phase 2): SQLite's CAST(? AS DATETIME/JSON/BOOLEAN/…) applies
  // numeric affinity and corrupts the value, and the other engines ignore
  // casts, so only Postgres gets a cast map.
  it.each(["sqlite", "mysql", "mariadb", "mssql", "duckdb"])(
    "has no cast map for %s, even with the table's columns cached",
    async (type) => {
      const { manager } = makeManager({ type });
      expect(await manager.buildCastMap("public", "users")).toBe(undefined);
      expect(await manager.buildCastMap("app", "orders")).toBe(undefined);
      expect(client.tableMetadata).not.toHaveBeenCalled();
    },
  );

  it.each(["sqlite", "mysql", "mariadb", "mssql", "duckdb"])(
    "doesn't load columns for %s once it runs on the Rust engine",
    async (type) => {
      usesRustEngine.mockReturnValue(true);
      try {
        const { manager } = makeManager({ type });
        expect(await manager.buildCastMap("app", "orders")).toBe(undefined);
        expect(client.tableMetadata).not.toHaveBeenCalled();
      } finally {
        usesRustEngine.mockReset();
        usesRustEngine.mockImplementation((c) => c.type === "postgres");
      }
    },
  );

  it("doesn't load columns for TypeScript engines", async () => {
    const { manager } = makeManager({ type: "sqlite" });
    expect(await manager.buildCastMap("main", "nope")).toBe(undefined);
    expect(client.tableMetadata).not.toHaveBeenCalled();
  });
});

describe("QueryCrudManager via EngineClient", () => {
  const source = { schema: "public", name: "users", primaryKeys: ["id"] };
  const row = { id: 9007199254740993n, name: "a" };

  beforeEach(() => {
    getEngineClient.mockClear();
    for (const fn of Object.values(client)) fn.mockReset();
  });

  it("updateCellDirect builds with the cast map and executes the bindings", async () => {
    client.buildUpdate.mockResolvedValue({ sql: "UPDATE", bindValues: ["b", row.id] });
    const { manager, state, provider } = makeManager();

    const result = await manager.updateCellDirect(source, row, "name", "b");

    expect(result).toEqual({ success: true });
    expect(getEngineClient).toHaveBeenCalledWith(connection, state);
    expect(client.buildUpdate).toHaveBeenCalledWith("public", "users", "name", "b", ["id"], row, {
      id: "bigint",
      code: "bpchar",
      balance: "numeric",
      meta: "jsonb",
    });
    expect(provider.execute).toHaveBeenCalledWith("pc-1", "UPDATE", ["b", row.id]);
  });

  it("insertRow passes the cast map and returns lastInsertId", async () => {
    client.buildInsert.mockResolvedValue({ sql: "INSERT", bindValues: [1] });
    const { manager } = makeManager();

    const result = await manager.insertRow(source, { id: 1 });

    expect(result).toEqual({ success: true, lastInsertId: 7 });
    expect(client.buildInsert.mock.calls[0]).toEqual([
      "public",
      "users",
      { id: 1 },
      { id: "bigint", code: "bpchar", balance: "numeric", meta: "jsonb" },
    ]);
  });

  it("deleteRow and setCellDefaultDirect queue the built SQL when pending changes are on", async () => {
    client.buildDelete.mockResolvedValue({ sql: "DELETE", bindValues: [row.id] });
    client.buildSetDefault.mockResolvedValue({ sql: "SET DEFAULT", bindValues: [row.id] });
    const { manager, addPending, provider } = makeManager({ pending: true });

    expect(await manager.deleteRow(source, row)).toEqual({ success: true, queued: true });
    expect(await manager.setCellDefaultDirect(source, row, "name")).toEqual({
      success: true,
      queued: true,
    });

    // The cast map covers the primary-key placeholders too (bug fix 6).
    const casts = { id: "bigint", code: "bpchar", balance: "numeric", meta: "jsonb" };
    expect(client.buildDelete).toHaveBeenCalledWith("public", "users", ["id"], row, casts);
    expect(client.buildSetDefault).toHaveBeenCalledWith(
      "public",
      "users",
      "name",
      ["id"],
      row,
      casts,
    );
    expect(addPending.mock.calls.map((c) => [c[1], c[5]])).toEqual([
      ["DELETE", [row.id]],
      ["SET DEFAULT", [row.id]],
    ]);
    expect(provider.execute).not.toHaveBeenCalled();
  });

  it("builds with the loaded cast map when the table isn't cached", async () => {
    client.tableMetadata.mockResolvedValue({
      columns: [column("id", "uuid", "uuid"), column("meta", "jsonb", "jsonb")],
      indexes: [],
    });
    client.buildUpdate.mockResolvedValue({ sql: "UPDATE", bindValues: [null, "k"] });
    client.buildSetDefault.mockResolvedValue({ sql: "SET DEFAULT", bindValues: ["k"] });
    client.buildInsert.mockResolvedValue({ sql: "INSERT", bindValues: [null] });
    client.buildDelete.mockResolvedValue({ sql: "DELETE", bindValues: ["k"] });
    const { manager } = makeManager();
    const orders = { schema: "app", name: "orders", primaryKeys: ["id"] };
    const key = { id: "k" };
    const casts = { id: "uuid", meta: "jsonb" };

    expect(await manager.updateCellDirect(orders, key, "meta", null)).toEqual({ success: true });
    expect(await manager.setCellDefaultDirect(orders, key, "meta")).toEqual({ success: true });
    expect(await manager.insertRow(orders, { meta: null })).toMatchObject({ success: true });
    expect(await manager.deleteRow(orders, key)).toEqual({ success: true });

    expect(client.buildUpdate.mock.calls[0]?.[6]).toEqual(casts);
    expect(client.buildSetDefault.mock.calls[0]?.[5]).toEqual(casts);
    expect(client.buildInsert.mock.calls[0]?.[3]).toEqual(casts);
    expect(client.buildDelete.mock.calls[0]?.[4]).toEqual(casts);
  });

  it("builds SQLite edits without casts", async () => {
    client.buildUpdate.mockResolvedValue({ sql: "UPDATE", bindValues: ["2024-01-01 10:00", 1] });
    client.buildInsert.mockResolvedValue({ sql: "INSERT", bindValues: ['{"a":1}'] });
    const { manager } = makeManager({ type: "sqlite" });

    await manager.updateCellDirect(source, row, "balance", "2024-01-01 10:00");
    await manager.insertRow(source, { meta: '{"a":1}' });

    expect(client.buildUpdate.mock.calls[0]?.[6]).toBe(undefined);
    expect(client.buildInsert.mock.calls[0]?.[3]).toBe(undefined);
  });

  it("sends SQLite's Set to default the column's default expression from fresh metadata", async () => {
    usesRustEngine.mockImplementation((c) => c.type === "postgres" || c.type === "sqlite");
    try {
      client.tableMetadata.mockResolvedValue({
        columns: [
          { ...column("status", "TEXT"), defaultValue: "'active'" },
          { ...column("at", "DATETIME"), defaultValue: "CURRENT_TIMESTAMP" },
          column("note", "TEXT"),
        ],
        indexes: [],
      });
      client.buildSetDefault.mockResolvedValue({ sql: "UPDATE", bindValues: [row.id] });
      const { manager, provider } = makeManager({ type: "sqlite" });

      for (const c of ["status", "at", "note"]) {
        expect(await manager.setCellDefaultDirect(source, row, c)).toEqual({ success: true });
      }
      // No casts for SQLite; the default expression is the 7th argument, NULL without one.
      expect(client.buildSetDefault.mock.calls.map((c) => [c[2], c[5], c[6]])).toEqual([
        ["status", undefined, "'active'"],
        ["at", undefined, "CURRENT_TIMESTAMP"],
        ["note", undefined, "NULL"],
      ]);
      expect(client.tableMetadata).toHaveBeenCalledTimes(3);
      expect(client.tableMetadata).toHaveBeenCalledWith("public", "users");
      expect(provider.execute).toHaveBeenCalledTimes(3);

      // A column the metadata doesn't list fails instead of setting NULL.
      const missing = await manager.setCellDefaultDirect(source, row, "gone");
      expect(missing.success).toBe(false);
      expect(missing.error).toContain('Column "gone" not found');
      expect(client.buildSetDefault).toHaveBeenCalledTimes(3);
    } finally {
      usesRustEngine.mockImplementation((c) => c.type === "postgres");
    }
  });

  it("sends no default expression for other engines", async () => {
    client.buildSetDefault.mockResolvedValue({ sql: "UPDATE", bindValues: [row.id] });
    const { manager } = makeManager();
    await manager.setCellDefaultDirect(source, row, "name");
    expect(client.buildSetDefault.mock.calls[0]).toHaveLength(6);
    expect(client.tableMetadata).not.toHaveBeenCalled();
  });

  it("fails a keyed edit that matched no row, naming the table and key", async () => {
    client.buildUpdate.mockResolvedValue({ sql: "UPDATE", bindValues: ["b", row.id] });
    client.buildSetDefault.mockResolvedValue({ sql: "SET DEFAULT", bindValues: [row.id] });
    client.buildDelete.mockResolvedValue({ sql: "DELETE", bindValues: [row.id] });
    const { manager, provider } = makeManager();
    provider.execute.mockResolvedValue({ rowsAffected: 0, lastInsertId: 7 });

    for (const result of [
      await manager.updateCellDirect(source, row, "name", "b"),
      await manager.setCellDefaultDirect(source, row, "name"),
      await manager.deleteRow(source, row),
    ]) {
      expect(result.success).toBe(false);
      expect(result.error).toContain("public.users");
      expect(result.error).toContain("id = 9007199254740993");
    }
    expect(provider.execute).toHaveBeenCalledTimes(3);
  });

  it("names every key column, quoting text keys", async () => {
    client.buildDelete.mockResolvedValue({ sql: "DELETE", bindValues: [] });
    const { manager, provider } = makeManager();
    provider.execute.mockResolvedValue({ rowsAffected: 0, lastInsertId: 7 });

    const result = await manager.deleteRow(
      { schema: "public", name: "users", primaryKeys: ["org", "name"] },
      { org: 3, name: "O'Brien" },
    );

    expect(result.error).toContain("org = 3, name = 'O''Brien'");
  });

  it("succeeds when a keyed edit affected its row", async () => {
    client.buildDelete.mockResolvedValue({ sql: "DELETE", bindValues: [row.id] });
    const { manager } = makeManager();
    expect(await manager.deleteRow(source, row)).toEqual({ success: true });
  });

  it("returns the builder's error instead of throwing", async () => {
    client.buildUpdate.mockRejectedValue(new Error("ENGINE_ERROR: nope"));
    const { manager } = makeManager();

    const result = await manager.updateCellDirect(source, row, "name", "b");

    expect(result.success).toBe(false);
    expect(result.error).toContain("nope");
  });
});

describe("QueryCrudManager.executeReadOnly", () => {
  function readOnlySetup(connections: Record<string, unknown>[]) {
    const state = {
      activeConnectionId: "conn-2",
      activeConnection: connections.find((c) => c.id === "conn-2"),
      connections,
      schemas: {},
    } as unknown as DatabaseState;
    const provider = {
      select: vi.fn(async () => [{ written: true }]),
      selectReadOnly: vi.fn(async (..._args: unknown[]) => [{ n: 1 }]),
    };
    const getForType = vi.fn(async (_type: string) => provider);
    const providers = { getForType } as unknown as ProviderRegistry;
    const pending = { isEnabled: () => false } as unknown as PendingChangesManager;
    return {
      manager: new QueryCrudManager(state, providers, pending),
      state,
      provider,
      getForType,
    };
  }

  const local = { id: "conn-1", type: "postgres", name: "Local", providerConnectionId: "pc-1" };
  const other = { id: "conn-2", type: "sqlite", name: "Other", providerConnectionId: "pc-2" };

  it("runs on the named connection, not the active one, through selectReadOnly", async () => {
    const { manager, provider, getForType } = readOnlySetup([local, other]);
    const signal = new AbortController().signal;

    expect(await manager.executeReadOnly("conn-1", "SELECT 1 AS n", signal)).toEqual([{ n: 1 }]);
    expect(getForType).toHaveBeenCalledWith("postgres");
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-1", "SELECT 1 AS n", signal);
    expect(provider.select).not.toHaveBeenCalled();
  });

  it("reads the provider connection id at call time, so it survives a reconnect", async () => {
    const { manager, provider, state } = readOnlySetup([local, other]);
    // reconnect() replaces the connection object with a new provider id.
    state.connections = [{ ...local, providerConnectionId: "pc-9" } as never, other as never];

    await manager.executeReadOnly("conn-1", "SELECT 1");
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-9", "SELECT 1", undefined);
  });

  it("reads the provider connection id after getting the provider", async () => {
    const { manager, provider, state, getForType } = readOnlySetup([local, other]);
    // A reconnect lands while the provider is being fetched.
    getForType.mockImplementationOnce(async () => {
      state.connections = [{ ...local, providerConnectionId: "pc-9" } as never, other as never];
      return provider;
    });
    await manager.executeReadOnly("conn-1", "SELECT 1");
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-9", "SELECT 1", undefined);
  });

  it("refuses a connection disconnected while the provider was fetched", async () => {
    const { manager, provider, state, getForType } = readOnlySetup([local, other]);
    getForType.mockImplementationOnce(async () => {
      state.connections = [{ ...local, providerConnectionId: undefined } as never, other as never];
      return provider;
    });
    await expect(manager.executeReadOnly("conn-1", "SELECT 1")).rejects.toThrow("is disconnected");
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("refuses a removed connection, naming it", async () => {
    const { manager, provider } = readOnlySetup([other]);
    await expect(manager.executeReadOnly("conn-1", "SELECT 1", undefined, "Local")).rejects.toThrow(
      'The connection "Local" was removed',
    );
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("refuses a disconnected connection, naming it", async () => {
    const { manager, provider } = readOnlySetup([
      { ...local, providerConnectionId: undefined },
      other,
    ]);
    await expect(manager.executeReadOnly("conn-1", "SELECT 1")).rejects.toThrow(
      'The connection "Local" is disconnected; reconnect it and try again',
    );
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("runs the token check before the provider", async () => {
    const { manager, provider } = readOnlySetup([local, other]);
    await expect(manager.executeReadOnly("conn-1", "SELECT 1; DELETE FROM t")).rejects.toThrow(
      "Only read-only SELECT queries are permitted",
    );
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
  });

  it("checks against the named connection's engine, not the active one's", async () => {
    // `#` starts a comment on MySQL, so this is one SELECT there; on
    // Postgres it's an operator and the DELETE is a second statement.
    const sql = "SELECT 1 # ; DELETE FROM t";
    const mysql = { ...other, type: "mysql" };
    const { manager, provider } = readOnlySetup([local, mysql]);
    await expect(manager.executeReadOnly("conn-1", sql)).rejects.toThrow(
      "Only read-only SELECT queries are permitted",
    );
    expect(provider.selectReadOnly).not.toHaveBeenCalled();
    await manager.executeReadOnly("conn-2", sql);
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-2", sql, undefined);
  });

  it("rejects with the provider's error", async () => {
    const { manager, provider } = readOnlySetup([local, other]);
    provider.selectReadOnly.mockRejectedValueOnce(new Error("READ_ONLY: cannot execute INSERT"));
    await expect(manager.executeReadOnly("conn-1", "SELECT f()")).rejects.toThrow(
      "cannot execute INSERT",
    );
  });
});
