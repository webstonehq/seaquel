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
import { splitDdlScript } from "$lib/utils/ddl-script";
import { splitColumnType } from "$lib/utils/column-type";
import { m } from "$lib/paraglide/messages.js";

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
        // "varchar(255)" → type "varchar", length "255"; types the editor
        // can't rebuild from type + length (suffixes, enums, nested types)
        // stay whole.
        const { type, length, precision } = splitColumnType(col.type);

        return {
          id: crypto.randomUUID(),
          name: col.name,
          type,
          length,
          precision,
          nullable: col.nullable,
          defaultValue: col.defaultValue ?? "",
          isPrimaryKey: col.isPrimaryKey,
          // UNIQUE constraints as the engine reports them (DuckDB): the
          // column's own (the checkbox) and membership in any, composite
          // too, which DuckDB's ALTER TABLE notes read.
          isUnique: col.isUnique === true,
          ...(col.inUniqueConstraint ? { inUniqueConstraint: true } : {}),
          // A collation that isn't the database default (MSSQL), so ALTER
          // COLUMN restates it instead of resetting the column to the default.
          ...(col.collation ? { collation: col.collation } : {}),
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

    // Edits the engine can't make come back as `-- …` notes, not statements.
    const { statements, notes } = splitDdlScript(sql);
    const showNotes = (title: string) =>
      toast.warning(title, {
        description: notes.join("\n"),
        descriptionClass: "whitespace-pre-line",
      });
    if (statements.length === 0) {
      if (notes.length > 0) showNotes(m.table_editor_nothing_applied());
      return false;
    }

    // Queue statements when pending changes is enabled
    const pendingChanges = this.getPendingChanges();
    if (pendingChanges.isEnabled()) {
      const origin = tab.isEditMode ? ("alter-table" as const) : ("create-table" as const);
      for (const stmt of statements) {
        pendingChanges.add(connection.id, stmt, "other", origin);
      }
      toast.info(
        `${statements.length} statement${statements.length > 1 ? "s" : ""} added to pending changes`,
      );
      if (notes.length > 0) showNotes(m.table_editor_changes_skipped());
      pendingChanges.openSheet();
      return true;
    }

    try {
      const provider = await this.providers.getForType(connection.type);
      for (const stmt of statements) {
        await provider.execute(connection.providerConnectionId, stmt);
      }
      toast.success(
        tab.isEditMode
          ? `Table "${tab.tableDefinition.tableName}" updated successfully`
          : `Table "${tab.tableDefinition.tableName}" created successfully`,
      );
      if (notes.length > 0) showNotes(m.table_editor_changes_skipped());
      await this.refreshSchemaFn(connection.id);
      return true;
    } catch (error) {
      errorToast(`Failed to ${tab.isEditMode ? "update" : "create"} table: ${errorText(error)}`);
      return false;
    }
  }
}
