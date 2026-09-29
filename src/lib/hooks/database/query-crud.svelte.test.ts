/**
 * The grid's edits over the edits service (phase 5c): intents out, outcomes
 * in, on the saved connection each edit is given, never the active one.
 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import type { ApplyChangesParams, PlanEditsParams } from "./edit-service/types";

const debug = vi.fn();
const logError = vi.fn();
vi.mock("$lib/utils/logger", () => ({
  log: {
    debug: (...args: unknown[]) => debug(...args),
    error: (...args: unknown[]) => logError(...args),
    info: vi.fn(),
    warn: vi.fn(),
  },
}));

const { QueryCrudManager } = await import("./query-crud.svelte.js");
const { CoreEditService } = await import("./edit-service/core-service.js");
const { scriptedCore, refusal } = await import("./edit-service/scripted-core.js");

describe("QueryCrudManager edits", () => {
  // Connection A holds the data; B is active in the sidebar.
  const a = { id: "conn-a", type: "postgres", name: "A", providerConnectionId: "pc-a" };
  const b = { id: "conn-b", type: "postgres", name: "B", providerConnectionId: "pc-b" };
  const source = { schema: "public", name: "users", primaryKeys: ["id"] };
  const row = { id: 5, name: "a" };

  function setup(opts: { pending?: boolean; script?: Parameters<typeof scriptedCore>[0] } = {}) {
    const state = {
      activeConnectionId: "conn-b",
      activeConnection: b,
      connections: [a, b],
      schemas: {},
    } as unknown as DatabaseState;
    const core = scriptedCore(opts.script);
    const service = new CoreEditService(() => core.client);
    const serviceFor = vi.fn(async (..._args: unknown[]) => service);
    const addPlanned = vi.fn();
    const pending = {
      isEnabled: () => opts.pending ?? false,
      addPlanned,
    } as unknown as PendingChangesManager;
    const manager = new QueryCrudManager(
      state,
      {} as ProviderRegistry,
      pending,
      serviceFor as never,
    );
    const applies = () => core.of("applyChanges") as ApplyChangesParams[];
    const plans = () => core.of("planEdits") as PlanEditsParams[];
    return { manager, state, core, serviceFor, addPlanned, applies, plans };
  }

  beforeEach(() => {
    logError.mockClear();
  });

  it("an immediate edit sends one applyChanges with one change, unconfirmed and without history", async () => {
    const { manager, applies, plans } = setup();
    expect(await manager.updateCellDirect("conn-a", source, row, "name", "b")).toEqual({
      success: true,
      error: undefined,
      queued: undefined,
    });
    expect(plans()).toEqual([]);
    expect(applies()).toHaveLength(1);
    const [apply] = applies();
    expect(apply.connectionId).toBe("pc-a");
    expect(apply.confirmed).toBeUndefined();
    expect(apply.history).toBeUndefined();
    expect(apply.changes).toEqual([
      {
        type: "edit",
        id: expect.any(String),
        edit: {
          type: "updateCell",
          target: { schema: "public", table: "users" },
          key: [["id", 5]],
          column: "name",
          value: "b",
        },
      },
    ]);
  });

  it("runs every edit on the given connection, not the active one", async () => {
    const { manager, applies, serviceFor, state } = setup();
    await manager.updateCellDirect("conn-a", source, row, "name", "b");
    await manager.setCellDefaultDirect("conn-a", source, row, "name");
    await manager.insertRow("conn-a", source, { id: 1 });
    await manager.deleteRow("conn-a", source, row);

    expect(applies().map((p) => p.connectionId)).toEqual(["pc-a", "pc-a", "pc-a", "pc-a"]);
    expect(applies().map((p) => p.changes[0].type === "edit" && p.changes[0].edit.type)).toEqual([
      "updateCell",
      "setDefault",
      "insertRow",
      "deleteRow",
    ]);
    for (const call of serviceFor.mock.calls) expect(call[0]).toBe(state.connections[0]);
  });

  it("insertRow returns the insert id", async () => {
    const { manager } = setup({
      script: {
        applyChanges: (p) => ({
          outcome: "applied",
          mode: "single",
          applied: 1,
          results: [{ id: p.changes[0].id, rowsAffected: 1, lastInsertId: 42 }],
          ddl: false,
          history: [],
        }),
      },
    });
    expect(await manager.insertRow("conn-a", source, { name: "x" })).toEqual({
      success: true,
      error: undefined,
      queued: undefined,
      lastInsertId: 42,
    });
  });

  it("a queued edit stores the intent and its planned display fields", async () => {
    const { manager, addPlanned, applies, plans } = setup({ pending: true });
    expect(await manager.updateCellDirect("conn-a", source, row, "name", "b")).toMatchObject({
      success: true,
      queued: true,
    });
    expect(applies()).toEqual([]);
    expect(plans()).toEqual([
      {
        connectionId: "pc-a",
        edits: [
          {
            type: "updateCell",
            target: { schema: "public", table: "users" },
            key: [["id", 5]],
            column: "name",
            value: "b",
          },
        ],
      },
    ]);
    const [connectionId, edit, planned, origin, target] = addPlanned.mock.calls[0] ?? [];
    expect(connectionId).toBe("conn-a");
    expect(edit).toEqual(plans()[0].edits[0]);
    expect(planned).toMatchObject({ queryType: "update", dml: true });
    expect(origin).toBe("inline-edit");
    expect(target).toEqual({
      schema: "public",
      table: "users",
      column: "name",
      primaryKeyValues: { id: 5 },
      newValue: "b",
    });
  });

  it("two quick queued edits of one cell keep the later, however their plans land", async () => {
    let releaseFirst!: () => void;
    const firstHeld = new Promise<void>((resolve) => (releaseFirst = resolve));
    let n = 0;
    const { manager, addPlanned } = setup({
      pending: true,
      script: {
        planEdits: async (p) => {
          n += 1;
          if (n === 1) await firstHeld;
          return p.edits.map(() => ({ sql: "X", params: [], queryType: "update", dml: true }));
        },
      },
    });
    const first = manager.updateCellDirect("conn-a", source, row, "name", "first");
    await new Promise((resolve) => setTimeout(resolve, 0));
    const second = await manager.updateCellDirect("conn-a", source, row, "name", "second");
    releaseFirst();
    expect(await first).toMatchObject({ success: true, queued: true });
    expect(second).toMatchObject({ success: true, queued: true });
    // The later edit queued; the earlier plan, landing after it, was dropped.
    expect(addPlanned).toHaveBeenCalledOnce();
    expect(addPlanned.mock.calls[0][1]).toMatchObject({ value: "second" });

    // Another cell, or the same cell after, isn't affected.
    await manager.updateCellDirect("conn-a", source, { id: 6 }, "name", "other");
    await manager.updateCellDirect("conn-a", source, row, "name", "third");
    expect(addPlanned).toHaveBeenCalledTimes(3);
  });

  it("queues under the given connection with each edit's origin", async () => {
    const { manager, addPlanned } = setup({ pending: true });
    await manager.updateCellDirect("conn-a", source, row, "name", "b");
    await manager.setCellDefaultDirect("conn-a", source, row, "name");
    await manager.insertRow("conn-a", source, { id: 1 });
    await manager.deleteRow("conn-a", source, row);
    expect(addPlanned.mock.calls.map((c) => [c[0], c[3]])).toEqual([
      ["conn-a", "inline-edit"],
      ["conn-a", "set-default"],
      ["conn-a", "insert-row"],
      ["conn-a", "delete-row"],
    ]);
  });

  it("NO_ROWS_AFFECTED shows the i18n message with the change's table and key", async () => {
    const { manager } = setup({
      script: {
        applyChanges: (p) => ({
          outcome: "applied",
          mode: "single",
          applied: 0,
          results: [],
          failed: {
            id: p.changes[0].id,
            index: 0,
            code: "NO_ROWS_AFFECTED",
            message: "Change 1 matched no row.",
          },
          ddl: false,
          history: [],
        }),
      },
    });
    const composite = { schema: "inv", name: "stock", primaryKeys: ["region", "sku"] };
    const result = await manager.deleteRow("conn-a", composite, { region: "eu", sku: "A'1" });
    expect(result.success).toBe(false);
    expect(result.error).toContain("inv.stock");
    expect(result.error).toContain("region = 'eu', sku = 'A''1'");
  });

  it("a database error shows as the grid shows run errors; a refusal shows its code", async () => {
    const failing = setup({
      script: {
        applyChanges: (p) => ({
          outcome: "applied",
          mode: "single",
          applied: 0,
          results: [],
          failed: { id: p.changes[0].id, index: 0, code: "QUERY_ERROR", message: "boom" },
          ddl: false,
          history: [],
        }),
      },
    });
    expect((await failing.manager.updateCellDirect("conn-a", source, row, "name", "b")).error).toBe(
      "boom",
    );

    const refused = setup({
      script: {
        applyChanges: () => {
          throw refusal("NOT_EDITABLE", "users has no primary key");
        },
      },
    });
    expect((await refused.manager.updateCellDirect("conn-a", source, row, "name", "b")).error).toBe(
      "NOT_EDITABLE: users has no primary key",
    );
  });

  it("an edit Core asks to confirm shows a translated message and applies nothing", async () => {
    const { manager } = setup({
      script: {
        applyChanges: () => ({
          outcome: "confirmRequired",
          destructive: [{ index: 0, sql: "DELETE FROM users", reason: "delete_no_where" }],
          destructiveTotal: 1,
        }),
      },
    });
    const { success, error } = await manager.deleteRow("conn-a", source, row);
    expect(success).toBe(false);
    expect(error).toMatch(/wasn't confirmed, so nothing was applied/);
  });

  it("TRANSACTION_OPEN asks to commit or roll back first", async () => {
    const { manager } = setup({
      script: {
        applyChanges: () => {
          throw refusal("TRANSACTION_OPEN", "a transaction is open");
        },
      },
    });
    const { error } = await manager.updateCellDirect("conn-a", source, row, "name", "b");
    expect(error).toMatch(/Commit or roll it back first/);
  });

  it("an edit after a reconnect uses the connection's new Core id", async () => {
    const { manager, state, applies, serviceFor } = setup();
    state.connections = [{ ...a, providerConnectionId: "pc-a2" } as never, b as never];
    await manager.updateCellDirect("conn-a", source, row, "name", "b");
    expect(applies()[0].connectionId).toBe("pc-a2");

    // A reconnect that lands while the service is looked up.
    const service = await serviceFor.mock.results[0]?.value;
    serviceFor.mockImplementationOnce(async () => {
      state.connections = [{ ...a, providerConnectionId: "pc-a3" } as never, b as never];
      return service;
    });
    await manager.deleteRow("conn-a", source, row);
    expect(applies()[1].connectionId).toBe("pc-a3");
  });

  it("an edit on a disconnected or removed connection is refused, and nothing is sent", async () => {
    const { manager, state, core, addPlanned } = setup();
    state.connections = [{ ...a, providerConnectionId: undefined } as never, b as never];
    const refused = { success: false, error: "No connection established" };
    expect(await manager.updateCellDirect("conn-a", source, row, "name", "b")).toEqual(refused);
    expect(await manager.setCellDefaultDirect("conn-a", source, row, "name")).toEqual(refused);
    expect(await manager.insertRow("conn-a", source, { id: 1 })).toMatchObject(refused);
    expect(await manager.deleteRow("conn-a", source, row)).toEqual(refused);
    state.connections = [b as never];
    expect(await manager.updateCellDirect("conn-a", source, row, "name", "b")).toEqual(refused);
    expect(core.calls).toEqual([]);
    expect(addPlanned).not.toHaveBeenCalled();
  });

  it("a queued edit whose connection went while it was planned queues nothing", async () => {
    const holder: { state?: DatabaseState } = {};
    const { manager, state, addPlanned } = setup({
      pending: true,
      script: {
        planEdits: (p) => {
          holder.state!.connections = [{ ...a, providerConnectionId: undefined } as never];
          return p.edits.map(() => ({ sql: "X", params: [], queryType: "update", dml: true }));
        },
      },
    });
    holder.state = state;
    expect(await manager.updateCellDirect("conn-a", source, row, "name", "b")).toEqual({
      success: false,
      error: "No connection established",
    });
    expect(addPlanned).not.toHaveBeenCalled();
  });

  it("a table without a primary key is refused before anything is sent", async () => {
    const { manager, core } = setup();
    const noKey = { ...source, primaryKeys: [] };
    expect(await manager.updateCellDirect("conn-a", noKey, row, "name", "b")).toEqual({
      success: false,
      error: "No primary key found",
    });
    expect(core.calls).toEqual([]);
  });

  it("the stale-key log line holds no key value", async () => {
    const canary = "CANARY-51c9";
    const { manager } = setup({
      script: {
        applyChanges: (p) => ({
          outcome: "applied",
          mode: "single",
          applied: 0,
          results: [],
          failed: { id: p.changes[0].id, index: 0, code: "NO_ROWS_AFFECTED", message: "x" },
          ddl: false,
          history: [],
        }),
      },
    });
    const result = await manager.updateCellDirect("conn-a", source, { id: canary }, "name", "b");
    // The user still sees the key; the log doesn't.
    expect(result.error).toContain(canary);
    expect(logError).toHaveBeenCalledTimes(1);
    const line = String(logError.mock.calls[0]?.[0]);
    expect(line).toContain("public.users");
    expect(line).not.toContain(canary);
  });

  it("sidebar drop and truncate send intents with confirmed", async () => {
    const { manager, applies } = setup();
    await manager.dropObject("conn-a", { schema: "public", name: "v" }, "view");
    await manager.truncateTable("conn-a", { schema: "public", name: "t" });
    expect(applies().map((p) => [p.connectionId, p.confirmed, p.changes[0]])).toEqual([
      [
        "pc-a",
        true,
        {
          type: "edit",
          id: expect.any(String),
          edit: { type: "dropObject", target: { schema: "public", table: "v" }, kind: "view" },
        },
      ],
      [
        "pc-a",
        true,
        {
          type: "edit",
          id: expect.any(String),
          edit: { type: "truncateTable", target: { schema: "public", table: "t" } },
        },
      ],
    ]);
  });

  it("sidebar drop and truncate queue with their origins when pending changes are on", async () => {
    const { manager, addPlanned, applies } = setup({ pending: true });
    expect(await manager.dropObject("conn-a", { schema: "public", name: "t" }, "table")).toEqual({
      queued: true,
    });
    await manager.dropObject("conn-a", { schema: "public", name: "mv" }, "materializedView");
    await manager.truncateTable("conn-a", { schema: "public", name: "t" });
    expect(addPlanned.mock.calls.map((c) => c[3])).toEqual([
      "drop-table",
      "drop-view",
      "truncate-table",
    ]);
    expect(applies()).toEqual([]);
  });

  it("a failed sidebar drop throws the error", async () => {
    const { manager } = setup({
      script: {
        applyChanges: (p) => ({
          outcome: "applied",
          mode: "single",
          applied: 0,
          results: [],
          failed: { id: p.changes[0].id, index: 0, code: "EXECUTE_ERROR", message: "in use" },
          ddl: false,
          history: [],
        }),
      },
    });
    await expect(
      manager.dropObject("conn-a", { schema: "public", name: "t" }, "table"),
    ).rejects.toThrow("EXECUTE_ERROR: in use");
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
      selectReadOnly: vi.fn(async (..._args: unknown[]) => ({
        rows: [{ n: 1 }],
        truncated: false,
      })),
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

    expect(await manager.executeReadOnly("conn-1", "SELECT 1 AS n", signal)).toEqual({
      rows: [{ n: 1 }],
      truncated: false,
    });
    expect(getForType).toHaveBeenCalledWith("postgres");
    expect(provider.selectReadOnly).toHaveBeenCalledWith(
      "pc-1",
      "SELECT 1 AS n",
      signal,
      undefined,
    );
    expect(provider.select).not.toHaveBeenCalled();
  });

  it("reads the provider connection id at call time, so it survives a reconnect", async () => {
    const { manager, provider, state } = readOnlySetup([local, other]);
    // reconnect() replaces the connection object with a new provider id.
    state.connections = [{ ...local, providerConnectionId: "pc-9" } as never, other as never];

    await manager.executeReadOnly("conn-1", "SELECT 1");
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-9", "SELECT 1", undefined, undefined);
  });

  it("reads the provider connection id after getting the provider", async () => {
    const { manager, provider, state, getForType } = readOnlySetup([local, other]);
    // A reconnect lands while the provider is being fetched.
    getForType.mockImplementationOnce(async () => {
      state.connections = [{ ...local, providerConnectionId: "pc-9" } as never, other as never];
      return provider;
    });
    await manager.executeReadOnly("conn-1", "SELECT 1");
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-9", "SELECT 1", undefined, undefined);
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
    expect(provider.selectReadOnly).toHaveBeenCalledWith("pc-2", sql, undefined, undefined);
  });

  it("rejects with the provider's error", async () => {
    const { manager, provider } = readOnlySetup([local, other]);
    provider.selectReadOnly.mockRejectedValueOnce(new Error("READ_ONLY: cannot execute INSERT"));
    await expect(manager.executeReadOnly("conn-1", "SELECT f()")).rejects.toThrow(
      "cannot execute INSERT",
    );
  });
});
