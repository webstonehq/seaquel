import type Database from "@tauri-apps/plugin-sql";
import type { SchemaTable, SchemaTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { getAdapter, type DatabaseAdapter } from "$lib/db";
import { setMapValue } from "./map-utils.js";
import { mssqlQuery } from "$lib/services/mssql";

/**
 * Manages schema tabs: add, remove, set active.
 * Handles table metadata loading.
 */
export class SchemaTabManager {
  constructor(
    private state: DatabaseState,
    private tabOrdering: TabOrderingManager,
    private schedulePersistence: (connectionId: string | null) => void
  ) {}

  /**
   * Add a schema tab for the specified table.
   * Fetches table metadata (columns, indexes, foreign keys).
   */
  async add(table: SchemaTable): Promise<string | null> {
    if (!this.state.activeConnectionId || !this.state.activeConnection) return null;

    const tabs = this.state.schemaTabsByConnection.get(this.state.activeConnectionId) || [];
    const adapter = getAdapter(this.state.activeConnection.type);
    const isMssql = this.state.activeConnection.type === "mssql" && this.state.activeConnection.mssqlConnectionId;

    // Fetch table metadata - query columns and indexes
    let columnsResult: unknown[];
    let indexesResult: unknown[];
    let foreignKeysResult: unknown[] | undefined;

    if (isMssql) {
      const columnsQueryResult = await mssqlQuery(
        this.state.activeConnection.mssqlConnectionId!,
        adapter.getColumnsQuery(table.name, table.schema)
      );
      columnsResult = columnsQueryResult.rows;

      const indexesQueryResult = await mssqlQuery(
        this.state.activeConnection.mssqlConnectionId!,
        adapter.getIndexesQuery(table.name, table.schema)
      );
      indexesResult = indexesQueryResult.rows;

      if (adapter.getForeignKeysQuery) {
        const fkQueryResult = await mssqlQuery(
          this.state.activeConnection.mssqlConnectionId!,
          adapter.getForeignKeysQuery(table.name, table.schema)
        );
        foreignKeysResult = fkQueryResult.rows;
      }
    } else {
      columnsResult = (await this.state.activeConnection.database!.select(
        adapter.getColumnsQuery(table.name, table.schema)
      )) as unknown[];

      indexesResult = (await this.state.activeConnection.database!.select(
        adapter.getIndexesQuery(table.name, table.schema)
      )) as unknown[];

      // Fetch foreign keys if adapter supports it (needed for SQLite)
      if (adapter.getForeignKeysQuery) {
        foreignKeysResult = (await this.state.activeConnection.database!.select(
          adapter.getForeignKeysQuery(table.name, table.schema)
        )) as unknown[];
      }
    }

    // Update table with fetched columns and indexes
    const updatedTable: SchemaTable = {
      ...table,
      columns: adapter.parseColumnsResult(columnsResult || [], foreignKeysResult),
      indexes: adapter.parseIndexesResult(indexesResult || []),
    };

    // Update this.state.schemas with the refreshed table metadata
    const connectionSchemas = [...(this.state.schemas.get(this.state.activeConnectionId) || [])];
    const tableIndex = connectionSchemas.findIndex(
      (t) => t.name === table.name && t.schema === table.schema
    );
    if (tableIndex >= 0) {
      connectionSchemas[tableIndex] = updatedTable;
    }
    const newSchemas = new Map(this.state.schemas);
    newSchemas.set(this.state.activeConnectionId, connectionSchemas);
    this.state.schemas = newSchemas;

    // Check if table is already open
    const existingTab = tabs.find(
      (t) => t.table.name === table.name && t.table.schema === table.schema
    );
    if (existingTab) {
      // Update existing tab with new metadata
      const updatedTabs = tabs.map((t) =>
        t.id === existingTab.id ? { ...t, table: updatedTable } : t
      );
      const newSchemaTabs = new Map(this.state.schemaTabsByConnection);
      newSchemaTabs.set(this.state.activeConnectionId, updatedTabs);
      this.state.schemaTabsByConnection = newSchemaTabs;

      const newActiveSchemaIds = new Map(this.state.activeSchemaTabIdByConnection);
      newActiveSchemaIds.set(this.state.activeConnectionId, existingTab.id);
      this.state.activeSchemaTabIdByConnection = newActiveSchemaIds;
      this.schedulePersistence(this.state.activeConnectionId);
      return existingTab.id;
    }

    const newTab: SchemaTab = {
      id: `schema-tab-${Date.now()}`,
      table: updatedTable,
    };

    // Create new Map to trigger reactivity
    const newSchemaTabs = new Map(this.state.schemaTabsByConnection);
    newSchemaTabs.set(this.state.activeConnectionId, [...tabs, newTab]);
    this.state.schemaTabsByConnection = newSchemaTabs;

    this.tabOrdering.add(newTab.id);

    const newActiveSchemaIds = new Map(this.state.activeSchemaTabIdByConnection);
    newActiveSchemaIds.set(this.state.activeConnectionId, newTab.id);
    this.state.activeSchemaTabIdByConnection = newActiveSchemaIds;

    this.schedulePersistence(this.state.activeConnectionId);
    return newTab.id;
  }

  /**
   * Remove a schema tab by ID.
   */
  remove(id: string): void {
    this.tabOrdering.removeTabGeneric(
      () => this.state.schemaTabsByConnection,
      (m) => (this.state.schemaTabsByConnection = m),
      () => this.state.activeSchemaTabIdByConnection,
      (m) => (this.state.activeSchemaTabIdByConnection = m),
      id
    );
    this.schedulePersistence(this.state.activeConnectionId);
  }

  /**
   * Set the active schema tab by ID.
   */
  setActive(id: string): void {
    if (!this.state.activeConnectionId) return;

    const newActiveSchemaIds = new Map(this.state.activeSchemaTabIdByConnection);
    newActiveSchemaIds.set(this.state.activeConnectionId, id);
    this.state.activeSchemaTabIdByConnection = newActiveSchemaIds;
    this.schedulePersistence(this.state.activeConnectionId);
  }

  /**
   * Load column and index metadata for all tables in the background.
   * Updates the schema state progressively as each table's metadata is loaded.
   */
  async loadTableMetadataInBackground(
    connectionId: string,
    tables: SchemaTable[],
    adapter: DatabaseAdapter,
    database: Database | undefined,
    mssqlConnectionId?: string
  ): Promise<void> {
    // Process tables in parallel but update state as each completes
    const promises = tables.map(async (table, index) => {
      try {
        let columnsResult: unknown[];
        let indexesResult: unknown[];
        let foreignKeysResult: unknown[] | undefined;

        if (mssqlConnectionId) {
          const columnsQueryResult = await mssqlQuery(
            mssqlConnectionId,
            adapter.getColumnsQuery(table.name, table.schema)
          );
          columnsResult = columnsQueryResult.rows;

          const indexesQueryResult = await mssqlQuery(
            mssqlConnectionId,
            adapter.getIndexesQuery(table.name, table.schema)
          );
          indexesResult = indexesQueryResult.rows;

          if (adapter.getForeignKeysQuery) {
            const fkQueryResult = await mssqlQuery(
              mssqlConnectionId,
              adapter.getForeignKeysQuery(table.name, table.schema)
            );
            foreignKeysResult = fkQueryResult.rows;
          }
        } else if (database) {
          columnsResult = (await database.select(
            adapter.getColumnsQuery(table.name, table.schema)
          )) as unknown[];

          indexesResult = (await database.select(
            adapter.getIndexesQuery(table.name, table.schema)
          )) as unknown[];

          // Fetch foreign keys if adapter supports it (needed for SQLite)
          if (adapter.getForeignKeysQuery) {
            foreignKeysResult = (await database.select(
              adapter.getForeignKeysQuery(table.name, table.schema)
            )) as unknown[];
          }
        } else {
          // No valid connection, skip
          return;
        }

        const updatedTable: SchemaTable = {
          ...table,
          columns: adapter.parseColumnsResult(columnsResult || [], foreignKeysResult),
          indexes: adapter.parseIndexesResult(indexesResult || []),
        };

        // Update the schema state with the new table metadata
        const currentSchemas = this.state.schemas.get(connectionId);
        if (currentSchemas) {
          const updatedSchemas = [...currentSchemas];
          updatedSchemas[index] = updatedTable;
          const newSchemas = new Map(this.state.schemas);
          newSchemas.set(connectionId, updatedSchemas);
          this.state.schemas = newSchemas;
        }
      } catch (error) {
        console.error(`Failed to load metadata for table ${table.schema}.${table.name}:`, error);
      }
    });

    await Promise.allSettled(promises);
  }
}
