import { setContext, getContext } from "svelte";
import type {
  DatabaseConnection,
  SchemaTable,
  QueryTab,
  QueryHistoryItem,
  AIMessage,
  SchemaTab,
  QueryResult,
  SavedQuery,
} from "$lib/types";
import Database from "@tauri-apps/plugin-sql";
import { load } from "@tauri-apps/plugin-store";
import { toast } from "svelte-sonner";

// Type for persisted connection data (without password and database instance)
interface PersistedConnection {
  id: string;
  name: string;
  type: DatabaseConnection["type"];
  host: string;
  port: number;
  databaseName: string;
  username: string;
  sslMode?: string;
  connectionString?: string;
  lastConnected?: Date;
}

class UseDatabase {
  connections = $state<DatabaseConnection[]>([]);
  activeConnectionId = $state<string | null>(null);
  schemas = $state<Map<string, SchemaTable[]>>(new Map());
  queryTabsByConnection = $state<Map<string, QueryTab[]>>(new Map());
  activeQueryTabIdByConnection = $state<Map<string, string | null>>(new Map());
  schemaTabsByConnection = $state<Map<string, SchemaTab[]>>(new Map());
  activeSchemaTabIdByConnection = $state<Map<string, string | null>>(new Map());
  queryHistoryByConnection = $state<Map<string, QueryHistoryItem[]>>(new Map());
  savedQueriesByConnection = $state<Map<string, SavedQuery[]>>(new Map());
  aiMessages = $state<AIMessage[]>([]);
  isAIOpen = $state(false);
  activeView = $state<"query" | "schema">("query");

  activeConnection = $derived(
    this.connections.find((c) => c.id === this.activeConnectionId) || null,
  );

  queryTabs = $derived(
    this.activeConnectionId
      ? this.queryTabsByConnection.get(this.activeConnectionId) || []
      : [],
  );

  activeQueryTabId = $derived(
    this.activeConnectionId
      ? this.activeQueryTabIdByConnection.get(this.activeConnectionId) || null
      : null,
  );

  activeQueryTab = $derived(
    this.queryTabs.find((t) => t.id === this.activeQueryTabId) || null,
  );

  schemaTabs = $derived(
    this.activeConnectionId
      ? this.schemaTabsByConnection.get(this.activeConnectionId) || []
      : [],
  );

  activeSchemaTabId = $derived(
    this.activeConnectionId
      ? this.activeSchemaTabIdByConnection.get(this.activeConnectionId) || null
      : null,
  );

  activeSchemaTab = $derived(
    this.schemaTabs.find((t) => t.id === this.activeSchemaTabId) || null,
  );

  activeSchema = $derived(
    this.activeConnectionId
      ? this.schemas.get(this.activeConnectionId) || []
      : [],
  );

  activeConnectionQueryHistory = $derived(
    this.activeConnectionId
      ? this.queryHistoryByConnection.get(this.activeConnectionId) || []
      : [],
  );

  activeConnectionSavedQueries = $derived(
    this.activeConnectionId
      ? this.savedQueriesByConnection.get(this.activeConnectionId) || []
      : [],
  );

  constructor() {
    this.initializePersistedConnections();
  }

  private stripPasswordFromConnectionString(
    connectionString?: string,
  ): string | undefined {
    if (!connectionString) return undefined;

    try {
      // Handle SQLite
      if (
        connectionString.startsWith("sqlite://") ||
        connectionString.startsWith("sqlite:")
      ) {
        return connectionString;
      }

      // Parse URL-based connection strings
      connectionString = connectionString.replace(
        "postgresql://",
        "postgres://",
      );
      const url = new URL(connectionString);

      // Remove password from URL
      if (url.password) {
        url.password = "";
      }

      return url.toString().replace("postgres://", "postgresql://");
    } catch {
      // If parsing fails, return original string (it might not be a URL)
      return connectionString;
    }
  }

  private async initializePersistedConnections() {
    try {
      const store = await load("database_connections.json", {
        autoSave: true,
        defaults: { connections: [] },
      });
      const persistedConnections = (await store.get("connections")) as
        | PersistedConnection[]
        | null;

      if (persistedConnections && Array.isArray(persistedConnections)) {
        for (const persisted of persistedConnections) {
          // Load the connection without connecting (password is not persisted)
          const connection: DatabaseConnection = {
            id: persisted.id,
            name: persisted.name,
            type: persisted.type,
            host: persisted.host,
            port: persisted.port,
            databaseName: persisted.databaseName,
            username: persisted.username,
            password: "", // Password not persisted - user must enter it
            sslMode: persisted.sslMode,
            connectionString: persisted.connectionString,
            lastConnected: persisted.lastConnected
              ? new Date(persisted.lastConnected)
              : undefined,
            // database is undefined - user needs to provide password to connect
          };
          this.connections.push(connection);

          // Initialize empty maps for this connection
          const newQueryTabs = new Map(this.queryTabsByConnection);
          newQueryTabs.set(connection.id, []);
          this.queryTabsByConnection = newQueryTabs;

          const newSchemaTabs = new Map(this.schemaTabsByConnection);
          newSchemaTabs.set(connection.id, []);
          this.schemaTabsByConnection = newSchemaTabs;

          const newActiveQueryIds = new Map(this.activeQueryTabIdByConnection);
          newActiveQueryIds.set(connection.id, null);
          this.activeQueryTabIdByConnection = newActiveQueryIds;

          const newActiveSchemaIds = new Map(
            this.activeSchemaTabIdByConnection,
          );
          newActiveSchemaIds.set(connection.id, null);
          this.activeSchemaTabIdByConnection = newActiveSchemaIds;

          const newQueryHistory = new Map(this.queryHistoryByConnection);
          newQueryHistory.set(connection.id, []);
          this.queryHistoryByConnection = newQueryHistory;

          const newSavedQueries = new Map(this.savedQueriesByConnection);
          newSavedQueries.set(connection.id, []);
          this.savedQueriesByConnection = newSavedQueries;

          const newSchemas = new Map(this.schemas);
          newSchemas.set(connection.id, []);
          this.schemas = newSchemas;
        }
      }
    } catch (error) {
      console.error("Failed to load persisted connections:", error);
      // Silently fail - app will continue with no persisted connections
    }
  }

  private async persistConnection(connection: DatabaseConnection) {
    try {
      const store = await load("database_connections.json", {
        autoSave: true,
        defaults: { connections: [] },
      });

      const existingConnections = (await store.get("connections")) as
        | PersistedConnection[]
        | null;
      const connections = existingConnections || [];

      // Remove if already exists (update case)
      const filtered = connections.filter((c) => c.id !== connection.id);

      // Create persisted version without password and database instance
      const persistedConnection: PersistedConnection = {
        id: connection.id,
        name: connection.name,
        type: connection.type,
        host: connection.host,
        port: connection.port,
        databaseName: connection.databaseName,
        username: connection.username,
        sslMode: connection.sslMode,
        connectionString: this.stripPasswordFromConnectionString(
          connection.connectionString,
        ),
        lastConnected: connection.lastConnected,
      };

      filtered.push(persistedConnection);
      await store.set("connections", filtered);
      await store.save();
    } catch (error) {
      console.error("Failed to persist connection:", error);
      toast.error("Failed to save connection to storage");
    }
  }

  private async removePersistedConnection(connectionId: string) {
    try {
      const store = await load("database_connections.json", {
        autoSave: true,
        defaults: { connections: [] },
      });

      const existingConnections = (await store.get("connections")) as
        | PersistedConnection[]
        | null;
      const connections = existingConnections || [];

      const filtered = connections.filter((c) => c.id !== connectionId);
      await store.set("connections", filtered);
      await store.save();
    } catch (error) {
      console.error("Failed to remove persisted connection:", error);
      toast.error("Failed to remove connection from storage");
    }
  }

  async addConnection(connection: Omit<DatabaseConnection, "id">) {
    const newConnection: DatabaseConnection = {
      ...connection,
      id: `conn-${connection.host}-${connection.port}`,
      lastConnected: new Date(),
      database: connection.connectionString
        ? await Database.load(connection.connectionString)
        : undefined,
    };

    if (!this.connections.find((c) => c.id === newConnection.id)) {
      this.connections.push(newConnection);
    }

    this.activeConnectionId = newConnection.id;

    // Initialize tabs for new connection
    const newQueryTabs = new Map(this.queryTabsByConnection);
    newQueryTabs.set(newConnection.id, []);
    this.queryTabsByConnection = newQueryTabs;

    const newSchemaTabs = new Map(this.schemaTabsByConnection);
    newSchemaTabs.set(newConnection.id, []);
    this.schemaTabsByConnection = newSchemaTabs;

    const newActiveQueryIds = new Map(this.activeQueryTabIdByConnection);
    newActiveQueryIds.set(newConnection.id, null);
    this.activeQueryTabIdByConnection = newActiveQueryIds;

    const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
    newActiveSchemaIds.set(newConnection.id, null);
    this.activeSchemaTabIdByConnection = newActiveSchemaIds;

    // Initialize query history and saved queries for new connection
    const newQueryHistory = new Map(this.queryHistoryByConnection);
    newQueryHistory.set(newConnection.id, []);
    this.queryHistoryByConnection = newQueryHistory;

    const newSavedQueries = new Map(this.savedQueriesByConnection);
    newSavedQueries.set(newConnection.id, []);
    this.savedQueriesByConnection = newSavedQueries;

    const schemasWithTablesDbResult = await newConnection.database!
      .select(`SELECT 
          table_schema AS schema_name,
          table_name
      FROM 
          information_schema.tables
      WHERE 
          table_type = 'BASE TABLE'
          AND table_schema NOT IN ('pg_catalog', 'information_schema')
      ORDER BY 
          table_schema, table_name;`);
    console.log({ schemasWithTablesDbResult });

    // {schemasWithTablesDbResult: [{schema_name: "public", table_name: "public_one"}, {schema_name: "public", table_name: "public_two"}, {schema_name: "schema_two", table_name: "two_one"}]}
    const schemasWithTables: SchemaTable[] = (
      schemasWithTablesDbResult as any[]
    ).map((row: any) => {
      return {
        name: row.table_name,
        schema: row.schema_name,
        type: "table",
        columns: [],
        indexes: [],
      };
    });

    // Generate sample schema for the new connection
    // const sampleSchema: SchemaTable[] = [
    //   {
    //     name: "sample_table",
    //     schema: "public",
    //     type: "table",
    //     rowCount: 0,
    //     columns: [
    //       {
    //         name: "id",
    //         type: "integer",
    //         nullable: false,
    //         isPrimaryKey: true,
    //         isForeignKey: false,
    //       },
    //       {
    //         name: "created_at",
    //         type: "timestamp",
    //         nullable: false,
    //         defaultValue: "now()",
    //         isPrimaryKey: false,
    //         isForeignKey: false,
    //       },
    //     ],
    //     indexes: [
    //       {
    //         name: "sample_table_pkey",
    //         columns: ["id"],
    //         unique: true,
    //         type: "btree",
    //       },
    //     ],
    //   },
    // ];

    const newSchemas = new Map(this.schemas);
    newSchemas.set(newConnection.id, schemasWithTables);
    // newSchemas.set(newConnection.id, sampleSchema);
    this.schemas = newSchemas;

    // Create initial query tab for new connection
    this.addQueryTab();

    // Persist the connection to store (without password)
    this.persistConnection(newConnection);

    return newConnection.id;
  }

  async reconnectConnection(connectionId: string, connection: Omit<DatabaseConnection, "id">) {
    const existingConnection = this.connections.find((c) => c.id === connectionId);
    if (!existingConnection) {
      throw new Error(`Connection with id ${connectionId} not found`);
    }

    // Update the existing connection with the new database connection
    const database = connection.connectionString
      ? await Database.load(connection.connectionString)
      : undefined;

    existingConnection.database = database;
    existingConnection.lastConnected = new Date();
    existingConnection.password = connection.password;

    // Ensure maps are initialized for this connection
    if (!this.queryTabsByConnection.has(connectionId)) {
      const newQueryTabs = new Map(this.queryTabsByConnection);
      newQueryTabs.set(connectionId, []);
      this.queryTabsByConnection = newQueryTabs;
    }

    if (!this.schemaTabsByConnection.has(connectionId)) {
      const newSchemaTabs = new Map(this.schemaTabsByConnection);
      newSchemaTabs.set(connectionId, []);
      this.schemaTabsByConnection = newSchemaTabs;
    }

    if (!this.activeQueryTabIdByConnection.has(connectionId)) {
      const newActiveQueryIds = new Map(this.activeQueryTabIdByConnection);
      newActiveQueryIds.set(connectionId, null);
      this.activeQueryTabIdByConnection = newActiveQueryIds;
    }

    if (!this.activeSchemaTabIdByConnection.has(connectionId)) {
      const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
      newActiveSchemaIds.set(connectionId, null);
      this.activeSchemaTabIdByConnection = newActiveSchemaIds;
    }

    if (!this.queryHistoryByConnection.has(connectionId)) {
      const newQueryHistory = new Map(this.queryHistoryByConnection);
      newQueryHistory.set(connectionId, []);
      this.queryHistoryByConnection = newQueryHistory;
    }

    if (!this.savedQueriesByConnection.has(connectionId)) {
      const newSavedQueries = new Map(this.savedQueriesByConnection);
      newSavedQueries.set(connectionId, []);
      this.savedQueriesByConnection = newSavedQueries;
    }

    // Fetch schemas
    const schemasWithTablesDbResult = await database!
      .select(`SELECT 
          table_schema AS schema_name,
          table_name
      FROM 
          information_schema.tables
      WHERE 
          table_type = 'BASE TABLE'
          AND table_schema NOT IN ('pg_catalog', 'information_schema')
      ORDER BY 
          table_schema, table_name;`);
    console.log({ schemasWithTablesDbResult });

    const schemasWithTables: SchemaTable[] = (
      schemasWithTablesDbResult as any[]
    ).map((row: any) => {
      return {
        name: row.table_name,
        schema: row.schema_name,
        type: "table",
        columns: [],
        indexes: [],
      };
    });

    const newSchemas = new Map(this.schemas);
    newSchemas.set(connectionId, schemasWithTables);
    this.schemas = newSchemas;

    // Set this as the active connection
    this.activeConnectionId = connectionId;

    // Create initial query tab if there aren't any
    const tabs = this.queryTabsByConnection.get(connectionId) || [];
    if (tabs.length === 0) {
      this.addQueryTab();
    }

    // Persist the connection to store (without password for security)
    this.persistConnection(existingConnection);

    return connectionId;
  }

  removeConnection(id: string) {
    // Remove from persistence
    this.removePersistedConnection(id);
    this.connections = this.connections.filter((c) => c.id !== id);

    // Clean up tabs for removed connection
    const newQueryTabs = new Map(this.queryTabsByConnection);
    newQueryTabs.delete(id);
    this.queryTabsByConnection = newQueryTabs;

    const newSchemaTabs = new Map(this.schemaTabsByConnection);
    newSchemaTabs.delete(id);
    this.schemaTabsByConnection = newSchemaTabs;

    const newActiveQueryIds = new Map(this.activeQueryTabIdByConnection);
    newActiveQueryIds.delete(id);
    this.activeQueryTabIdByConnection = newActiveQueryIds;

    const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
    newActiveSchemaIds.delete(id);
    this.activeSchemaTabIdByConnection = newActiveSchemaIds;

    const newSchemas = new Map(this.schemas);
    newSchemas.delete(id);
    this.schemas = newSchemas;

    // Clean up query history and saved queries for removed connection
    const newQueryHistory = new Map(this.queryHistoryByConnection);
    newQueryHistory.delete(id);
    this.queryHistoryByConnection = newQueryHistory;

    const newSavedQueries = new Map(this.savedQueriesByConnection);
    newSavedQueries.delete(id);
    this.savedQueriesByConnection = newSavedQueries;

    if (this.activeConnectionId === id) {
      const nextConnection = this.connections.find((c) => c.database);
      this.activeConnectionId = nextConnection?.id || null;
    }
  }

  setActiveConnection(id: string) {
    this.activeConnectionId = id;
  }

  toggleConnection(id: string) {
    const connection = this.connections.find((c) => c.id === id);
    if (connection) {
      connection.database = undefined;
      if (connection.database) {
        connection.lastConnected = new Date();
      } else {
        // If disconnecting the active connection, switch to another connected one
        if (this.activeConnectionId === id) {
          const nextConnection = this.connections.find(
            (c) => c.database && c.id !== id,
          );
          this.activeConnectionId = nextConnection?.id || null;
        }
      }
    }
  }

  addQueryTab(name?: string, query?: string, savedQueryId?: string) {
    if (!this.activeConnectionId) return null;

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const newTab: QueryTab = $state({
      id: `tab-${Date.now()}`,
      name: name || `Query ${tabs.length + 1}`,
      query: query || "",
      isExecuting: false,
      savedQueryId,
    });

    // Create new Map to trigger reactivity
    const newQueryTabs = new Map(this.queryTabsByConnection);
    newQueryTabs.set(this.activeConnectionId, [...tabs, newTab]);
    this.queryTabsByConnection = newQueryTabs;

    const newActiveQueryIds = new Map(this.activeQueryTabIdByConnection);
    newActiveQueryIds.set(this.activeConnectionId, newTab.id);
    this.activeQueryTabIdByConnection = newActiveQueryIds;

    return newTab.id;
  }

  removeQueryTab(id: string) {
    if (!this.activeConnectionId) return;

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const index = tabs.findIndex((t) => t.id === id);
    const newTabs = tabs.filter((t) => t.id !== id);

    const newQueryTabs = new Map(this.queryTabsByConnection);
    newQueryTabs.set(this.activeConnectionId, newTabs);
    this.queryTabsByConnection = newQueryTabs;

    const currentActiveId = this.activeQueryTabIdByConnection.get(
      this.activeConnectionId,
    );
    if (currentActiveId === id) {
      const newActiveQueryIds = new Map(this.activeQueryTabIdByConnection);
      if (newTabs.length > 0) {
        const newIndex = Math.min(index, newTabs.length - 1);
        newActiveQueryIds.set(
          this.activeConnectionId,
          newTabs[newIndex]?.id || null,
        );
      } else {
        newActiveQueryIds.set(this.activeConnectionId, null);
      }
      this.activeQueryTabIdByConnection = newActiveQueryIds;
    }
  }

  renameQueryTab(id: string, newName: string) {
    if (!this.activeConnectionId) return;

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === id);
    if (tab) {
      tab.name = newName;
      // Trigger reactivity by creating new Map
      const newQueryTabs = new Map(this.queryTabsByConnection);
      newQueryTabs.set(this.activeConnectionId, [...tabs]);
      this.queryTabsByConnection = newQueryTabs;
    }
  }

  setActiveQueryTab(id: string) {
    if (!this.activeConnectionId) return;

    const newActiveQueryIds = new Map(this.activeQueryTabIdByConnection);
    newActiveQueryIds.set(this.activeConnectionId, id);
    this.activeQueryTabIdByConnection = newActiveQueryIds;
  }

  updateQueryTabContent(id: string, query: string) {
    if (!this.activeConnectionId) return;

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === id);
    if (tab) {
      tab.query = query;
    }
  }

  saveQuery(name: string, query: string, tabId?: string) {
    if (!this.activeConnectionId) return null;

    // Check if this tab is already linked to a saved query
    let savedQueryId: string | undefined;
    if (tabId) {
      const tabs =
        this.queryTabsByConnection.get(this.activeConnectionId) || [];
      const tab = tabs.find((t) => t.id === tabId);
      savedQueryId = tab?.savedQueryId;
    }

    if (savedQueryId) {
      // Update existing saved query
      const savedQueries =
        this.savedQueriesByConnection.get(this.activeConnectionId) || [];
      const savedQuery = savedQueries.find((q) => q.id === savedQueryId);
      if (savedQuery) {
        savedQuery.name = name;
        savedQuery.query = query;
        savedQuery.updatedAt = new Date();
        const newSavedQueries = new Map(this.savedQueriesByConnection);
        newSavedQueries.set(this.activeConnectionId, [...savedQueries]);
        this.savedQueriesByConnection = newSavedQueries;
        return savedQueryId;
      }
    }

    // Create new saved query
    const newSavedQuery: SavedQuery = {
      id: `saved-${Date.now()}`,
      name,
      query,
      connectionId: this.activeConnectionId,
      createdAt: new Date(),
      updatedAt: new Date(),
    };

    const savedQueries =
      this.savedQueriesByConnection.get(this.activeConnectionId) || [];
    const newSavedQueries = new Map(this.savedQueriesByConnection);
    newSavedQueries.set(this.activeConnectionId, [
      ...savedQueries,
      newSavedQuery,
    ]);
    this.savedQueriesByConnection = newSavedQueries;

    // Link tab to saved query if tabId provided
    if (tabId) {
      const tabs =
        this.queryTabsByConnection.get(this.activeConnectionId) || [];
      const tab = tabs.find((t) => t.id === tabId);
      if (tab) {
        tab.savedQueryId = newSavedQuery.id;
        tab.name = name;
        const newQueryTabs = new Map(this.queryTabsByConnection);
        newQueryTabs.set(this.activeConnectionId, [...tabs]);
        this.queryTabsByConnection = newQueryTabs;
      }
    }

    return newSavedQuery.id;
  }

  deleteSavedQuery(id: string) {
    if (!this.activeConnectionId) return;

    const savedQueries =
      this.savedQueriesByConnection.get(this.activeConnectionId) || [];
    const filtered = savedQueries.filter((q) => q.id !== id);
    const newSavedQueries = new Map(this.savedQueriesByConnection);
    newSavedQueries.set(this.activeConnectionId, filtered);
    this.savedQueriesByConnection = newSavedQueries;

    // Remove savedQueryId from any tabs using this query
    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    tabs.forEach((tab) => {
      if (tab.savedQueryId === id) {
        tab.savedQueryId = undefined;
      }
    });
    const newQueryTabs = new Map(this.queryTabsByConnection);
    newQueryTabs.set(this.activeConnectionId, [...tabs]);
    this.queryTabsByConnection = newQueryTabs;
  }

  loadSavedQuery(savedQueryId: string) {
    if (!this.activeConnectionId) return;

    const savedQueries =
      this.savedQueriesByConnection.get(this.activeConnectionId) || [];
    const savedQuery = savedQueries.find((q) => q.id === savedQueryId);
    if (!savedQuery) return;

    // Check if a tab with this saved query is already open
    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const existingTab = tabs.find((t) => t.savedQueryId === savedQueryId);

    if (existingTab) {
      // Switch to existing tab
      this.setActiveQueryTab(existingTab.id);
      this.setActiveView("query");
    } else {
      // Create new tab
      this.addQueryTab(savedQuery.name, savedQuery.query, savedQueryId);
      this.setActiveView("query");
    }
  }

  loadQueryFromHistory(historyId: string) {
    if (!this.activeConnectionId) return;

    const queryHistory =
      this.queryHistoryByConnection.get(this.activeConnectionId) || [];
    const item = queryHistory.find((h) => h.id === historyId);
    if (!item) return;

    // Check if a tab with the exact same query is already open
    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const existingTab = tabs.find((t) => t.query.trim() === item.query.trim());

    if (existingTab) {
      // Switch to existing tab
      this.setActiveQueryTab(existingTab.id);
      this.setActiveView("query");
    } else {
      // Create new tab
      this.addQueryTab(
        `History: ${item.query.substring(0, 20)}...`,
        item.query,
      );
      this.setActiveView("query");
    }
  }

  async addSchemaTab(table: SchemaTable) {
    if (!this.activeConnectionId) return null;

    const tabs = this.schemaTabsByConnection.get(this.activeConnectionId) || [];

    // Fetch table metadata - query columns and indexes
    const columnsResult = (await this.activeConnection?.database!.select(`
      SELECT 
        column_name,
        data_type,
        is_nullable,
        column_default,
        (SELECT EXISTS (
          SELECT 1 FROM information_schema.constraint_column_usage ccu
          JOIN information_schema.table_constraints tc ON ccu.constraint_name = tc.constraint_name
          WHERE ccu.column_name = c.column_name 
            AND ccu.table_schema = c.table_schema
            AND ccu.table_name = c.table_name
            AND tc.constraint_type = 'PRIMARY KEY'
        )) as is_primary_key,
        (SELECT EXISTS (
          SELECT 1 FROM information_schema.constraint_column_usage ccu
          JOIN information_schema.table_constraints tc ON ccu.constraint_name = tc.constraint_name
          WHERE ccu.column_name = c.column_name 
            AND ccu.table_schema = c.table_schema
            AND ccu.table_name = c.table_name
            AND tc.constraint_type = 'FOREIGN KEY'
        )) as is_foreign_key
      FROM information_schema.columns c
      WHERE table_name = '${table.name}' AND table_schema = '${table.schema}'
      ORDER BY ordinal_position
    `)) as any[];

    const indexesResult = (await this.activeConnection?.database!.select(`
      SELECT
        indexname,
        indexdef,
        schemaname,
        tablename
      FROM pg_indexes
      WHERE tablename = '${table.name}' AND schemaname = '${table.schema}'
    `)) as any[];

    // Parse index columns from indexdef (e.g., "CREATE UNIQUE INDEX idx ON schema.table (col1, col2)")
    const parseIndexColumns = (indexdef: string): string[] => {
      const match = indexdef.match(/\((.*?)\)/);
      if (!match) return [];
      return match[1].split(",").map((col) => col.trim());
    };

    // Update table with fetched columns and indexes
    const updatedTable: SchemaTable = {
      ...table,
      columns: (columnsResult || []).map((col: any) => ({
        name: col.column_name,
        type: col.data_type,
        nullable: col.is_nullable === "YES",
        defaultValue: col.column_default || undefined,
        isPrimaryKey: col.is_primary_key,
        isForeignKey: col.is_foreign_key,
      })),
      indexes: (indexesResult || []).map((idx: any) => ({
        name: idx.indexname,
        columns: parseIndexColumns(idx.indexdef),
        unique: idx.indexdef.includes("UNIQUE"),
        type: "btree", // PostgreSQL primarily uses btree
      })),
    };

    // Update this.schemas with the refreshed table metadata
    const connectionSchemas = [
      ...(this.schemas.get(this.activeConnectionId) || []),
    ];
    const tableIndex = connectionSchemas.findIndex(
      (t) => t.name === table.name && t.schema === table.schema,
    );
    if (tableIndex >= 0) {
      connectionSchemas[tableIndex] = updatedTable;
    }
    const newSchemas = new Map(this.schemas);
    newSchemas.set(this.activeConnectionId, connectionSchemas);
    this.schemas = newSchemas;

    // Check if table is already open
    const existingTab = tabs.find(
      (t) => t.table.name === table.name && t.table.schema === table.schema,
    );
    if (existingTab) {
      // Update existing tab with new metadata
      existingTab.table = updatedTable;
      const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
      newActiveSchemaIds.set(this.activeConnectionId, existingTab.id);
      this.activeSchemaTabIdByConnection = newActiveSchemaIds;
      return existingTab.id;
    }

    const newTab: SchemaTab = {
      id: `schema-tab-${Date.now()}`,
      table: updatedTable,
    };

    // Create new Map to trigger reactivity
    const newSchemaTabs = new Map(this.schemaTabsByConnection);
    newSchemaTabs.set(this.activeConnectionId, [...tabs, newTab]);
    this.schemaTabsByConnection = newSchemaTabs;

    const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
    newActiveSchemaIds.set(this.activeConnectionId, newTab.id);
    this.activeSchemaTabIdByConnection = newActiveSchemaIds;

    return newTab.id;
  }

  removeSchemaTab(id: string) {
    if (!this.activeConnectionId) return;

    const tabs = this.schemaTabsByConnection.get(this.activeConnectionId) || [];
    const index = tabs.findIndex((t) => t.id === id);
    const newTabs = tabs.filter((t) => t.id !== id);

    const newSchemaTabs = new Map(this.schemaTabsByConnection);
    newSchemaTabs.set(this.activeConnectionId, newTabs);
    this.schemaTabsByConnection = newSchemaTabs;

    const currentActiveId = this.activeSchemaTabIdByConnection.get(
      this.activeConnectionId,
    );
    if (currentActiveId === id) {
      const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
      if (newTabs.length > 0) {
        const newIndex = Math.min(index, newTabs.length - 1);
        newActiveSchemaIds.set(
          this.activeConnectionId,
          newTabs[newIndex]?.id || null,
        );
      } else {
        newActiveSchemaIds.set(this.activeConnectionId, null);
      }
      this.activeSchemaTabIdByConnection = newActiveSchemaIds;
    }
  }

  setActiveSchemaTab(id: string) {
    if (!this.activeConnectionId) return;

    const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
    newActiveSchemaIds.set(this.activeConnectionId, id);
    this.activeSchemaTabIdByConnection = newActiveSchemaIds;
  }

  async executeQuery(tabId: string) {
    if (!this.activeConnectionId) return;

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    tab.isExecuting = true;

    const start = performance.now();
    const dbResult = (await this.activeConnection?.database!.select(
      tab.query,
    )) as any[];
    const totalMs = performance.now() - start;

    // Generate results
    const results: QueryResult = {
      columns: (dbResult?.length ?? 0) > 0 ? Object.keys(dbResult[0]) : [],
      rows: dbResult || [],
      rowCount: dbResult?.length ?? 0,
      executionTime: Math.round(totalMs * 100) / 100,
    };

    tab.results = results;
    tab.isExecuting = false;

    // Add to history
    const queryHistory =
      this.queryHistoryByConnection.get(this.activeConnectionId) || [];
    const newQueryHistory = new Map(this.queryHistoryByConnection);
    newQueryHistory.set(this.activeConnectionId, [
      {
        id: `hist-${Date.now()}`,
        query: tab.query,
        timestamp: new Date(),
        executionTime: results.executionTime,
        rowCount: results.rowCount,
        connectionId: this.activeConnectionId,
        favorite: false,
      },
      ...queryHistory,
    ]);
    this.queryHistoryByConnection = newQueryHistory;
  }

  toggleQueryFavorite(id: string) {
    if (!this.activeConnectionId) return;

    const queryHistory =
      this.queryHistoryByConnection.get(this.activeConnectionId) || [];
    const item = queryHistory.find((h) => h.id === id);
    if (item) {
      item.favorite = !item.favorite;
      const newQueryHistory = new Map(this.queryHistoryByConnection);
      newQueryHistory.set(this.activeConnectionId, [...queryHistory]);
      this.queryHistoryByConnection = newQueryHistory;
    }
  }

  toggleAI() {
    this.isAIOpen = !this.isAIOpen;
  }

  sendAIMessage(content: string) {
    const userMessage: AIMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      content,
      timestamp: new Date(),
    };
    this.aiMessages.push(userMessage);

    // Simulate AI response
    setTimeout(() => {
      const responses = [
        "Hey, it's Mike 👋. I appreciate your enthusiasm to play with the AI integration. It's not quite ready yet (boo), but rest assured this is a key feature that is high on my roadmap.",
      ];

      const assistantMessage: AIMessage = {
        id: `msg-${Date.now()}`,
        role: "assistant",
        content: responses[Math.floor(Math.random() * responses.length)],
        timestamp: new Date(),
      };
      this.aiMessages.push(assistantMessage);
    }, 1000);
  }

  setActiveView(view: "query" | "schema") {
    this.activeView = view;
  }
}

export const setDatabase = () => setContext("database", new UseDatabase());
export const useDatabase = () =>
  getContext<ReturnType<typeof setDatabase>>("database");
