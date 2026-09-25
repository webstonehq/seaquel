import type {
  CreateTableTab,
  CreateTableDefinition,
  SchemaTable,
  ActiveViewType,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import { getEngineClient } from "$lib/engine";
import type { ProviderRegistry } from "$lib/providers";
import type { PendingChangesManager } from "./pending-changes.svelte.js";
import { toast } from "svelte-sonner";
import { errorToast } from "$lib/utils/toast";

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * Manages create table tabs: add, remove, set active.
 * Handles DDL generation and execution.
 */
export class CreateTableTabManager extends BaseTabManager<CreateTableTab> {
  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
    private providers: ProviderRegistry,
    private refreshSchemaFn: (connectionId: string) => Promise<void>,
    private getPendingChanges: () => PendingChangesManager,
  ) {
    super(state, tabOrdering, schedulePersistence, setActiveView);
  }

  protected get accessors(): TabStateAccessors<CreateTableTab> {
    return {
      getTabs: () => this.state.createTableTabsByProject,
      setTabs: (r) => (this.state.createTableTabsByProject = r),
      getActiveId: () => this.state.activeCreateTableTabIdByProject,
      setActiveId: (r) => (this.state.activeCreateTableTabIdByProject = r),
    };
  }

  /**
   * Open a new Create Table tab with an empty definition.
   */
  add(schemaName?: string): string | null {
    if (!this.state.activeProjectId || !this.state.activeConnectionId) return null;

    const schemas = [...new Set(this.state.activeSchema.map((t) => t.schema))];
    const defaultSchema = schemaName ?? schemas[0] ?? "";
    const definition: CreateTableDefinition = {
      tableName: "",
      schemaName: defaultSchema,
      columns: [
        {
          id: crypto.randomUUID(),
          name: "id",
          type: "INTEGER",
          nullable: false,
          defaultValue: "",
          isPrimaryKey: true,
          isUnique: false,
        },
      ],
      indexes: [],
      foreignKeys: [],
    };

    const tab: CreateTableTab = {
      id: `create-table-${crypto.randomUUID()}`,
      connectionId: this.state.activeConnectionId,
      name: "New Table",
      tableDefinition: definition,
      generatedSql: undefined,
    };

    return this.appendTab(tab);
  }

  /**
   * Open a Create Table tab pre-populated from an existing table's schema.
   */
  addFromTable(table: SchemaTable): string | null {
    if (!this.state.activeProjectId || !this.state.activeConnectionId) return null;

    const definition: CreateTableDefinition = {
      tableName: table.name,
      schemaName: table.schema,
      columns: table.columns.map((col) => {
        // Split type and length/precision: "varchar(255)" → type="varchar", length="255"
        const typeMatch = col.type.match(/^([^(]+)\(([^)]+)\)/);
        let type = col.type;
        let length: string | undefined;
        let precision: string | undefined;

        if (typeMatch) {
          type = typeMatch[1].trim();
          const params = typeMatch[2].trim();
          if (params.includes(",")) {
            precision = params;
          } else {
            length = params;
          }
        }

        return {
          id: crypto.randomUUID(),
          name: col.name,
          type,
          length,
          precision,
          nullable: col.nullable,
          defaultValue: col.defaultValue ?? "",
          isPrimaryKey: col.isPrimaryKey,
          isUnique: false,
        };
      }),
      indexes: table.indexes.map((idx) => ({
        id: crypto.randomUUID(),
        name: idx.name,
        columns: [...idx.columns],
        unique: idx.unique,
        type: idx.type,
      })),
      foreignKeys: table.columns
        .filter((col) => col.isForeignKey && col.foreignKeyRef)
        .map((col) => ({
          id: crypto.randomUUID(),
          column: col.name,
          referencedSchema: col.foreignKeyRef!.referencedSchema,
          referencedTable: col.foreignKeyRef!.referencedTable,
          referencedColumn: col.foreignKeyRef!.referencedColumn,
        })),
    };

    // Deep-clone the definition so edits don't mutate the original
    const originalDefinition: CreateTableDefinition = JSON.parse(JSON.stringify(definition));

    const tab: CreateTableTab = {
      id: `create-table-${crypto.randomUUID()}`,
      connectionId: this.state.activeConnectionId,
      name: table.name,
      tableDefinition: definition,
      generatedSql: undefined,
      isEditMode: true,
      originalDefinition,
    };

    return this.appendTab(tab);
  }

  /**
   * Update the table definition for a tab.
   */
  updateDefinition(
    tabId: string,
    updater: (def: CreateTableDefinition) => CreateTableDefinition,
  ): void {
    this.updateTab(tabId, (tab) => {
      const updated = updater(tab.tableDefinition);
      return {
        ...tab,
        tableDefinition: updated,
        name: updated.tableName || "New Table",
      };
    });
    this.schedulePersistence(this.state.activeProjectId);
  }

  /**
   * Execute the CREATE TABLE DDL and refresh the schema.
   */
  async executeCreate(tabId: string): Promise<boolean> {
    const tab = this.getProjectTabs().find((t) => t.id === tabId);
    if (!tab) return false;

    const connection = this.state.connections.find((c) => c.id === tab.connectionId);
    if (!connection?.providerConnectionId) return false;

    if (!tab.tableDefinition.schemaName) {
      errorToast("A schema must be selected before creating a table");
      return false;
    }

    let sql: string;
    try {
      const client = getEngineClient(connection, this.state);
      sql =
        tab.isEditMode && tab.originalDefinition
          ? await client.alterTable(tab.originalDefinition, tab.tableDefinition)
          : await client.createTable(tab.tableDefinition);
    } catch (error) {
      errorToast(`Failed to generate SQL: ${errorText(error)}`);
      return false;
    }
    if (sql === "-- No changes detected") {
      toast.info("No changes to apply");
      return false;
    }

    if (!sql) return false;

    // Split into individual statements
    const statements = sql
      .split(";\n")
      .map((s) => s.trim())
      .filter((s) => s && !s.startsWith("--"));

    // Queue statements when pending changes is enabled
    const pendingChanges = this.getPendingChanges();
    if (pendingChanges.isEnabled()) {
      const origin = tab.isEditMode ? ("alter-table" as const) : ("create-table" as const);
      for (const stmt of statements) {
        pendingChanges.add(connection.id, stmt.endsWith(";") ? stmt : stmt + ";", "other", origin);
      }
      toast.info(
        `${statements.length} statement${statements.length > 1 ? "s" : ""} added to pending changes`,
      );
      pendingChanges.openSheet();
      return true;
    }

    try {
      const provider = await this.providers.getForType(connection.type);
      for (const stmt of statements) {
        await provider.execute(
          connection.providerConnectionId,
          stmt.endsWith(";") ? stmt : stmt + ";",
        );
      }
      toast.success(
        tab.isEditMode
          ? `Table "${tab.tableDefinition.tableName}" updated successfully`
          : `Table "${tab.tableDefinition.tableName}" created successfully`,
      );
      await this.refreshSchemaFn(connection.id);
      return true;
    } catch (error) {
      errorToast(`Failed to ${tab.isEditMode ? "update" : "create"} table: ${errorText(error)}`);
      return false;
    }
  }
}
