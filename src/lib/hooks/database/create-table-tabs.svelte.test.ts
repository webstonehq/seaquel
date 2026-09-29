import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CreateTableDefinition, CreateTableTab, SchemaTable } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import type { ApplyChangesParams } from "./edit-service/types";

const client = { alterTable: vi.fn(), createTable: vi.fn() };
vi.mock("$lib/engine", () => ({ getEngineClient: () => client }));
const toast = vi.hoisted(() => ({ success: vi.fn(), info: vi.fn(), warning: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast }));
const errorToast = vi.hoisted(() => vi.fn());
vi.mock("$lib/utils/toast", () => ({ errorToast }));

const { CreateTableTabManager } = await import("./create-table-tabs.svelte.js");
const { QueryCrudManager } = await import("./query-crud.svelte.js");
const { CoreEditService } = await import("./edit-service/core-service.js");
const { scriptedCore } = await import("./edit-service/scripted-core.js");

const definition: CreateTableDefinition = {
  tableName: "customers",
  schemaName: "main",
  columns: [],
  indexes: [],
  foreignKeys: [],
};

const TYPE_NOTE = 'SQLite can\'t alter column "email" (type); recreate the table to change it';
const FK_NOTE =
  'SQLite can\'t add a foreign key to an existing table: ("tier") REFERENCES "main"."tiers" ("id"); recreate the table to add it';
const RENAME = 'ALTER TABLE "main"."customers" RENAME COLUMN "name" TO "display_name";';

function makeManager(
  opts: { pending?: boolean; script?: Parameters<typeof scriptedCore>[0] } = {},
) {
  const tab: CreateTableTab = {
    id: "tab-1",
    connectionId: "conn-1",
    name: "customers",
    tableDefinition: definition,
    originalDefinition: definition,
    isEditMode: true,
  } as CreateTableTab;
  const state = {
    activeProjectId: "p1",
    activeConnectionId: "conn-1",
    activeCreateTableTabIdByProject: {},
    connections: [{ id: "conn-1", type: "sqlite", name: "db", providerConnectionId: "pc-1" }],
    createTableTabsByProject: { p1: [tab] },
  } as unknown as DatabaseState;
  const core = scriptedCore(opts.script);
  const service = new CoreEditService(() => core.client);
  const pending = {
    isEnabled: () => opts.pending ?? false,
    addSql: vi.fn(),
    openSheet: vi.fn(),
  };
  const crud = new QueryCrudManager(
    state,
    {} as ProviderRegistry,
    pending as unknown as PendingChangesManager,
    async () => service,
  );
  const refresh = vi.fn(async () => {});
  const manager = new CreateTableTabManager(
    state,
    { add: vi.fn() } as unknown as TabOrderingManager,
    () => {},
    () => {},
    refresh,
    () => pending as unknown as PendingChangesManager,
    () => crud,
  );
  const applies = () => core.of("applyChanges") as ApplyChangesParams[];
  return { manager, state, pending, refresh, applies };
}

describe("CreateTableTabManager.executeCreate", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("warns with the notes and runs nothing when every edit is a note", async () => {
    client.alterTable.mockResolvedValue(`-- ${TYPE_NOTE}\n-- ${FK_NOTE}`);
    const { manager, applies, refresh } = makeManager();
    expect(await manager.executeCreate("tab-1")).toBe(false);
    expect(applies()).toEqual([]);
    expect(refresh).not.toHaveBeenCalled();
    expect(toast.success).not.toHaveBeenCalled();
    expect(toast.warning).toHaveBeenCalledOnce();
    expect(toast.warning.mock.calls[0][0]).toBe("No changes were applied");
    expect(toast.warning.mock.calls[0][1].description).toBe(`${TYPE_NOTE}\n${FK_NOTE}`);
  });

  it("queues nothing with pending changes on when every edit is a note", async () => {
    client.alterTable.mockResolvedValue(`-- ${TYPE_NOTE}`);
    const { manager, pending } = makeManager({ pending: true });
    expect(await manager.executeCreate("tab-1")).toBe(false);
    expect(pending.addSql).not.toHaveBeenCalled();
    expect(pending.openSheet).not.toHaveBeenCalled();
    expect(toast.info).not.toHaveBeenCalled();
    expect(toast.warning.mock.calls[0][0]).toBe("No changes were applied");
    expect(toast.warning.mock.calls[0][1].description).toBe(TYPE_NOTE);
  });

  it("applies the statements through Core, then reports success and the notes", async () => {
    client.alterTable.mockResolvedValue(`${RENAME}\n-- ${TYPE_NOTE}`);
    const { manager, applies, refresh } = makeManager();
    expect(await manager.executeCreate("tab-1")).toBe(true);
    expect(applies()).toEqual([
      {
        connectionId: "pc-1",
        changes: [{ type: "sql", id: expect.any(String), sql: RENAME, params: [] }],
        confirmed: true,
      },
    ]);
    expect(refresh).toHaveBeenCalledWith("conn-1");
    expect(toast.success).toHaveBeenCalledWith('Table "customers" updated successfully');
    expect(toast.warning.mock.calls[0][0]).toBe("Some changes were not applied");
    expect(toast.warning.mock.calls[0][1].description).toBe(TYPE_NOTE);
  });

  it("queues the statements and reports the notes with pending changes on", async () => {
    client.alterTable.mockResolvedValue(`${RENAME}\n-- ${TYPE_NOTE}`);
    const { manager, pending } = makeManager({ pending: true });
    expect(await manager.executeCreate("tab-1")).toBe(true);
    expect(pending.addSql).toHaveBeenCalledWith("conn-1", RENAME, [], "other", "alter-table");
    expect(toast.info).toHaveBeenCalledWith("1 statement added to pending changes");
    expect(toast.warning.mock.calls[0][0]).toBe("Some changes were not applied");
  });

  it("the table editor applies its statements in order and stops at the first failure", async () => {
    const ADD = 'ALTER TABLE "main"."customers" ADD COLUMN "c" int;';
    client.alterTable.mockResolvedValue(`${RENAME}\n${ADD}`);
    const { manager, applies, refresh } = makeManager({
      script: {
        applyChanges: (p) => ({
          outcome: "applied",
          mode: "inOrder",
          applied: 1,
          results: [{ id: p.changes[0].id, rowsAffected: 0 }],
          failed: {
            id: p.changes[1].id,
            index: 1,
            code: "QUERY_ERROR",
            message: "duplicate column",
          },
          ddl: true,
          history: [],
        }),
      },
    });
    expect(await manager.executeCreate("tab-1")).toBe(false);
    // One call, both statements in order: Core runs them in order (DDL) and stops.
    expect(applies()).toHaveLength(1);
    expect(applies()[0].changes.map((c) => c.type === "sql" && c.sql)).toEqual([RENAME, ADD]);
    expect(errorToast).toHaveBeenCalledWith("Failed to update table: duplicate column");
    expect(toast.success).not.toHaveBeenCalled();
    // The first statement stayed applied: the schema reloads.
    expect(refresh).toHaveBeenCalledWith("conn-1");
  });

  it("shows no warning without notes", async () => {
    client.alterTable.mockResolvedValue(RENAME);
    const { manager } = makeManager();
    expect(await manager.executeCreate("tab-1")).toBe(true);
    expect(toast.success).toHaveBeenCalledOnce();
    expect(toast.warning).not.toHaveBeenCalled();
  });
});

describe("CreateTableTabManager.addFromTable", () => {
  it("copies a column's collation into the definition, and only when it has one", () => {
    const { manager, state } = makeManager();
    const table: SchemaTable = {
      name: "fx_types",
      schema: "dbo",
      type: "table",
      columns: [
        {
          name: "c_bin",
          type: "varchar(20)",
          nullable: true,
          isPrimaryKey: false,
          isForeignKey: false,
          collation: "Latin1_General_BIN",
        },
        {
          name: "c_dbdefault",
          type: "nvarchar(20)",
          nullable: true,
          isPrimaryKey: false,
          isForeignKey: false,
        },
      ],
      indexes: [],
    };

    const id = manager.addFromTable(table);
    const tab = state.createTableTabsByProject.p1.find((t) => t.id === id)!;
    for (const def of [tab.tableDefinition, tab.originalDefinition!]) {
      expect(def.columns[0]).toMatchObject({
        name: "c_bin",
        type: "varchar",
        length: "20",
        collation: "Latin1_General_BIN",
      });
      expect(def.columns[1]).not.toHaveProperty("collation");
    }
  });

  it("copies isUnique and inUniqueConstraint, so DuckDB's ALTER TABLE notes see UNIQUE columns", () => {
    const { manager, state } = makeManager();
    const col = (name: string, extra: Partial<SchemaTable["columns"][number]> = {}) => ({
      name,
      type: "VARCHAR",
      nullable: true,
      isPrimaryKey: false,
      isForeignKey: false,
      ...extra,
    });
    const table: SchemaTable = {
      name: "customers",
      schema: "fx_aux.main",
      type: "table",
      columns: [
        col("id", { type: "INTEGER", nullable: false, isPrimaryKey: true }),
        col("name"),
        col("email", { isUnique: true, inUniqueConstraint: true }),
        col("a", { inUniqueConstraint: true }),
        col("status", { isUnique: false }),
      ],
      indexes: [],
    };

    const id = manager.addFromTable(table);
    const tab = state.createTableTabsByProject.p1.find((t) => t.id === id)!;
    for (const def of [tab.tableDefinition, tab.originalDefinition!]) {
      expect(def.schemaName).toBe("fx_aux.main");
      expect(
        def.columns.map((c) => [c.name, c.isPrimaryKey, c.isUnique, c.inUniqueConstraint]),
      ).toEqual([
        ["id", true, false, undefined],
        ["name", false, false, undefined],
        ["email", false, true, true],
        ["a", false, false, true],
        ["status", false, false, undefined],
      ]);
    }
  });
});
