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

  it("returns the builder's error instead of throwing", async () => {
    client.buildUpdate.mockRejectedValue(new Error("ENGINE_ERROR: nope"));
    const { manager } = makeManager();

    const result = await manager.updateCellDirect(source, row, "name", "b");

    expect(result.success).toBe(false);
    expect(result.error).toContain("nope");
  });
});
