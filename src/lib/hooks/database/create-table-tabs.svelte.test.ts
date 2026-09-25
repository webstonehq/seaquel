import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CreateTableDefinition, CreateTableTab, SchemaTable } from "$lib/types";
import type { ProviderRegistry } from "$lib/providers";
import type { DatabaseState } from "./state.svelte.js";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";

const client = { alterTable: vi.fn(), createTable: vi.fn() };
vi.mock("$lib/engine", () => ({ getEngineClient: () => client }));
const toast = vi.hoisted(() => ({ success: vi.fn(), info: vi.fn(), warning: vi.fn() }));
vi.mock("svelte-sonner", () => ({ toast }));
const errorToast = vi.hoisted(() => vi.fn());
vi.mock("$lib/utils/toast", () => ({ errorToast }));

const { CreateTableTabManager } = await import("./create-table-tabs.svelte.js");

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

function makeManager(opts: { pending?: boolean } = {}) {
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
  const provider = { execute: vi.fn(async () => ({ rowsAffected: 0 })) };
  const providers = { getForType: vi.fn(async () => provider) } as unknown as ProviderRegistry;
  const pending = {
    isEnabled: () => opts.pending ?? false,
    add: vi.fn(),
    openSheet: vi.fn(),
  };
  const refresh = vi.fn(async () => {});
  const manager = new CreateTableTabManager(
    state,
    { add: vi.fn() } as unknown as TabOrderingManager,
    () => {},
    () => {},
    providers,
    refresh,
    () => pending as unknown as PendingChangesManager,
  );
  return { manager, state, provider, pending, refresh };
}

describe("CreateTableTabManager.executeCreate", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("warns with the notes and runs nothing when every edit is a note", async () => {
    client.alterTable.mockResolvedValue(`-- ${TYPE_NOTE}\n-- ${FK_NOTE}`);
    const { manager, provider, refresh } = makeManager();
    expect(await manager.executeCreate("tab-1")).toBe(false);
    expect(provider.execute).not.toHaveBeenCalled();
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
    expect(pending.add).not.toHaveBeenCalled();
    expect(pending.openSheet).not.toHaveBeenCalled();
    expect(toast.info).not.toHaveBeenCalled();
    expect(toast.warning.mock.calls[0][0]).toBe("No changes were applied");
    expect(toast.warning.mock.calls[0][1].description).toBe(TYPE_NOTE);
  });

  it("runs the statements, then reports success and the notes", async () => {
    client.alterTable.mockResolvedValue(`${RENAME}\n-- ${TYPE_NOTE}`);
    const { manager, provider, refresh } = makeManager();
    expect(await manager.executeCreate("tab-1")).toBe(true);
    expect(provider.execute.mock.calls).toEqual([["pc-1", RENAME]]);
    expect(refresh).toHaveBeenCalledWith("conn-1");
    expect(toast.success).toHaveBeenCalledWith('Table "customers" updated successfully');
    expect(toast.warning.mock.calls[0][0]).toBe("Some changes were not applied");
    expect(toast.warning.mock.calls[0][1].description).toBe(TYPE_NOTE);
  });

  it("queues the statements and reports the notes with pending changes on", async () => {
    client.alterTable.mockResolvedValue(`${RENAME}\n-- ${TYPE_NOTE}`);
    const { manager, pending } = makeManager({ pending: true });
    expect(await manager.executeCreate("tab-1")).toBe(true);
    expect(pending.add).toHaveBeenCalledWith("conn-1", RENAME, "other", "alter-table");
    expect(toast.info).toHaveBeenCalledWith("1 statement added to pending changes");
    expect(toast.warning.mock.calls[0][0]).toBe("Some changes were not applied");
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
