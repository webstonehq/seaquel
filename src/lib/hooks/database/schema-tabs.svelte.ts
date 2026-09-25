import type { SchemaTable, SchemaTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import { getEngineClient, type EngineClient } from "$lib/engine";
import { handleError, createError } from "$lib/errors";
import { log } from "$lib/utils/logger";
import { storeTableMetadata } from "./schema-cache.js";

/**
 * Manages schema tabs: add, remove, set active.
 * Handles table metadata loading.
 * Tabs are organized per-project.
 */
export class SchemaTabManager extends BaseTabManager<SchemaTab> {
  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
  ) {
    super(state, tabOrdering, schedulePersistence);
  }

  protected get accessors(): TabStateAccessors<SchemaTab> {
    return {
      getTabs: () => this.state.schemaTabsByProject,
      setTabs: (r) => (this.state.schemaTabsByProject = r),
      getActiveId: () => this.state.activeSchemaTabIdByProject,
      setActiveId: (r) => (this.state.activeSchemaTabIdByProject = r),
    };
  }

  /**
   * Fetch columns, indexes, and foreign keys for a single table.
   * Returns an updated SchemaTable with metadata populated.
   */
  private async fetchTableMetadata(client: EngineClient, table: SchemaTable): Promise<SchemaTable> {
    const { columns, indexes } = await client.tableMetadata(table.schema, table.name);
    return { ...table, columns, indexes };
  }

  /**
   * Add a schema tab for the specified table.
   * Fetches table metadata (columns, indexes, foreign keys).
   */
  async add(table: SchemaTable): Promise<string | null> {
    if (
      !this.state.activeProjectId ||
      !this.state.activeConnectionId ||
      !this.state.activeConnection
    )
      return null;

    const connectionId = this.state.activeConnectionId;
    const tabs = this.getProjectTabs();

    // Use cached metadata from the schema store (populated at connection time)
    const cached = (this.state.schemas[connectionId] ?? []).find(
      (t) => t.name === table.name && t.schema === table.schema,
    );
    const tableWithMetadata = cached ?? table;

    // Check if table is already open on the active connection
    const existingTab = tabs.find(
      (t) =>
        t.connectionId === connectionId &&
        t.table.name === table.name &&
        t.table.schema === table.schema,
    );
    if (existingTab) {
      this.updateTab(existingTab.id, (t) => ({ ...t, table: tableWithMetadata }));
      this.setActive(existingTab.id);
      return existingTab.id;
    }

    const newTab: SchemaTab = {
      id: `schema-tab-${crypto.randomUUID()}`,
      connectionId,
      table: tableWithMetadata,
    };

    const tabId = this.appendTab(newTab);

    // Refresh metadata in the background if columns aren't loaded yet
    if (tableWithMetadata.columns.length === 0) {
      void this.refreshTabMetadata(tabId, connectionId, table);
    }

    return tabId;
  }

  /**
   * Refresh a schema tab's metadata in the background.
   */
  private async refreshTabMetadata(
    tabId: string,
    connectionId: string,
    table: SchemaTable,
  ): Promise<void> {
    const connection = this.state.connections.find((c) => c.id === connectionId);
    if (!connection?.providerConnectionId) return;

    try {
      const client = getEngineClient(connection, this.state);
      const updatedTable = await this.fetchTableMetadata(client, table);

      storeTableMetadata(this.state, connectionId, updatedTable);

      // Update the open tab
      this.updateTab(tabId, (t) => ({ ...t, table: updatedTable }));
    } catch {
      // Silently fail — the tab still shows whatever we had cached
    }
  }

  /**
   * Load column and index metadata for all tables in the background.
   * Updates the schema state progressively as each table's metadata is loaded.
   */
  async loadTableMetadataInBackground(
    connectionId: string,
    tables: SchemaTable[],
    client: EngineClient,
  ): Promise<void> {
    // Nothing to load while disconnected (e.g. the connection dropped since the schema loaded).
    if (!this.state.connections.find((c) => c.id === connectionId)?.providerConnectionId) return;

    void log.debug(`Loading metadata for ${tables.length} tables on ${connectionId}`);
    // Process tables in parallel but update state as each completes
    const promises = tables.map(async (table, index) => {
      try {
        const updatedTable = await this.fetchTableMetadata(client, table);

        // Update the schema state with the new table metadata
        const currentSchemas = this.state.schemas[connectionId];
        if (currentSchemas) {
          const updatedSchemas = [...currentSchemas];
          updatedSchemas[index] = updatedTable;
          this.state.schemas = {
            ...this.state.schemas,
            [connectionId]: updatedSchemas,
          };
        }

        this.syncOpenTabsForTable(connectionId, updatedTable);
      } catch (error) {
        void log.error(`Metadata load failed for ${connectionId}`);
        handleError(
          createError(
            "SCHEMA_LOAD_FAILED",
            error instanceof Error ? error.message : String(error),
            `Failed to load metadata for ${table.schema}.${table.name}`,
            { table: table.name, schema: table.schema },
          ),
          { silent: true },
        );
      }
    });

    await Promise.allSettled(promises);
    this.dropTabsForMissingTables(connectionId, tables);
    void log.debug(`Metadata loaded for ${connectionId}`);
  }

  /**
   * Point every open tab for this table at the freshly loaded metadata.
   *
   * Tabs restored from a previous session carry only the table and schema
   * name — the connection isn't live at restore time — so without this they
   * render as an empty table until the user reopens them.
   */
  private syncOpenTabsForTable(connectionId: string, table: SchemaTable): void {
    const projectId = this.state.connections.find((c) => c.id === connectionId)?.projectId;
    if (!projectId) return;

    const tabs = this.state.schemaTabsByProject[projectId] ?? [];
    const matches = (tab: SchemaTab) =>
      tab.connectionId === connectionId &&
      tab.table.name === table.name &&
      tab.table.schema === table.schema &&
      tab.table.columns.length === 0;
    if (!tabs.some(matches)) return;

    // Written per project rather than through updateTab(), which always
    // targets the active project — the connection may belong to another one.
    this.state.schemaTabsByProject = {
      ...this.state.schemaTabsByProject,
      [projectId]: tabs.map((tab) => (matches(tab) ? { ...tab, table } : tab)),
    };
  }

  /**
   * Close restored tabs whose table no longer exists on the connection.
   * Otherwise they'd sit there permanently empty with nothing to load.
   */
  private dropTabsForMissingTables(connectionId: string, tables: SchemaTable[]): void {
    const projectId = this.state.connections.find((c) => c.id === connectionId)?.projectId;
    // remove() operates on the active project, so leave other projects' tabs
    // alone; they are re-checked the next time that project is connected.
    if (!projectId || projectId !== this.state.activeProjectId) return;

    const known = new Set(tables.map((t) => `${t.schema}.${t.name}`));
    const stale = (this.state.schemaTabsByProject[projectId] ?? []).filter(
      (tab) =>
        tab.connectionId === connectionId &&
        tab.table.columns.length === 0 &&
        !known.has(`${tab.table.schema}.${tab.table.name}`),
    );
    for (const tab of stale) {
      void log.info(`Closing schema tab for missing table ${tab.table.schema}.${tab.table.name}`);
      this.remove(tab.id);
    }
  }
}
