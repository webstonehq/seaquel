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
  SSHTunnelConfig,
  ExplainTab,
  ExplainResult,
  ExplainPlanNode,
} from "$lib/types";
import Database from "@tauri-apps/plugin-sql";
import { load } from "@tauri-apps/plugin-store";
import { toast } from "svelte-sonner";
import { getAdapter, type DatabaseAdapter, type ExplainNode } from "$lib/db";
import { detectQueryType, isWriteQuery, extractTableFromSelect } from "$lib/db/query-utils";
import { createSshTunnel, closeSshTunnel } from "$lib/services/ssh-tunnel";

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
  sshTunnel?: SSHTunnelConfig;
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
  explainTabsByConnection = $state<Map<string, ExplainTab[]>>(new Map());
  activeExplainTabIdByConnection = $state<Map<string, string | null>>(new Map());
  aiMessages = $state<AIMessage[]>([]);
  isAIOpen = $state(false);
  activeView = $state<"query" | "schema" | "explain">("query");

  // Map connection IDs to their SSH tunnel IDs for cleanup
  private tunnelIds = new Map<string, string>();

  // Helper method to update Map state with reactivity
  private setMapValue<K, V>(
    getter: () => Map<K, V>,
    setter: (m: Map<K, V>) => void,
    key: K,
    value: V
  ): void {
    const newMap = new Map(getter());
    newMap.set(key, value);
    setter(newMap);
  }

  // Helper method to delete from Map state with reactivity
  private deleteMapKey<K, V>(
    getter: () => Map<K, V>,
    setter: (m: Map<K, V>) => void,
    key: K
  ): void {
    const newMap = new Map(getter());
    newMap.delete(key);
    setter(newMap);
  }

  // Initialize all per-connection maps for a new or reconnected connection
  private initializeConnectionMaps(connectionId: string): void {
    this.setMapValue(() => this.queryTabsByConnection, m => this.queryTabsByConnection = m, connectionId, []);
    this.setMapValue(() => this.schemaTabsByConnection, m => this.schemaTabsByConnection = m, connectionId, []);
    this.setMapValue(() => this.activeQueryTabIdByConnection, m => this.activeQueryTabIdByConnection = m, connectionId, null);
    this.setMapValue(() => this.activeSchemaTabIdByConnection, m => this.activeSchemaTabIdByConnection = m, connectionId, null);
    this.setMapValue(() => this.queryHistoryByConnection, m => this.queryHistoryByConnection = m, connectionId, []);
    this.setMapValue(() => this.savedQueriesByConnection, m => this.savedQueriesByConnection = m, connectionId, []);
    this.setMapValue(() => this.explainTabsByConnection, m => this.explainTabsByConnection = m, connectionId, []);
    this.setMapValue(() => this.activeExplainTabIdByConnection, m => this.activeExplainTabIdByConnection = m, connectionId, null);
    this.setMapValue(() => this.schemas, m => this.schemas = m, connectionId, []);
  }

  // Clean up all per-connection maps when removing a connection
  private cleanupConnectionMaps(connectionId: string): void {
    this.deleteMapKey(() => this.queryTabsByConnection, m => this.queryTabsByConnection = m, connectionId);
    this.deleteMapKey(() => this.schemaTabsByConnection, m => this.schemaTabsByConnection = m, connectionId);
    this.deleteMapKey(() => this.activeQueryTabIdByConnection, m => this.activeQueryTabIdByConnection = m, connectionId);
    this.deleteMapKey(() => this.activeSchemaTabIdByConnection, m => this.activeSchemaTabIdByConnection = m, connectionId);
    this.deleteMapKey(() => this.queryHistoryByConnection, m => this.queryHistoryByConnection = m, connectionId);
    this.deleteMapKey(() => this.savedQueriesByConnection, m => this.savedQueriesByConnection = m, connectionId);
    this.deleteMapKey(() => this.explainTabsByConnection, m => this.explainTabsByConnection = m, connectionId);
    this.deleteMapKey(() => this.activeExplainTabIdByConnection, m => this.activeExplainTabIdByConnection = m, connectionId);
    this.deleteMapKey(() => this.schemas, m => this.schemas = m, connectionId);
  }

  // Ensure maps exist for a connection (used during reconnect to preserve existing state)
  private ensureConnectionMapsExist(connectionId: string): void {
    if (!this.queryTabsByConnection.has(connectionId)) {
      this.setMapValue(() => this.queryTabsByConnection, m => this.queryTabsByConnection = m, connectionId, []);
    }
    if (!this.schemaTabsByConnection.has(connectionId)) {
      this.setMapValue(() => this.schemaTabsByConnection, m => this.schemaTabsByConnection = m, connectionId, []);
    }
    if (!this.activeQueryTabIdByConnection.has(connectionId)) {
      this.setMapValue(() => this.activeQueryTabIdByConnection, m => this.activeQueryTabIdByConnection = m, connectionId, null);
    }
    if (!this.activeSchemaTabIdByConnection.has(connectionId)) {
      this.setMapValue(() => this.activeSchemaTabIdByConnection, m => this.activeSchemaTabIdByConnection = m, connectionId, null);
    }
    if (!this.queryHistoryByConnection.has(connectionId)) {
      this.setMapValue(() => this.queryHistoryByConnection, m => this.queryHistoryByConnection = m, connectionId, []);
    }
    if (!this.savedQueriesByConnection.has(connectionId)) {
      this.setMapValue(() => this.savedQueriesByConnection, m => this.savedQueriesByConnection = m, connectionId, []);
    }
    if (!this.explainTabsByConnection.has(connectionId)) {
      this.setMapValue(() => this.explainTabsByConnection, m => this.explainTabsByConnection = m, connectionId, []);
    }
    if (!this.activeExplainTabIdByConnection.has(connectionId)) {
      this.setMapValue(() => this.activeExplainTabIdByConnection, m => this.activeExplainTabIdByConnection = m, connectionId, null);
    }
  }

  // Generic tab removal helper
  private removeTabGeneric<T extends { id: string }>(
    tabsGetter: () => Map<string, T[]>,
    tabsSetter: (m: Map<string, T[]>) => void,
    activeIdGetter: () => Map<string, string | null>,
    activeIdSetter: (m: Map<string, string | null>) => void,
    tabId: string
  ): void {
    if (!this.activeConnectionId) return;

    const tabs = tabsGetter().get(this.activeConnectionId) || [];
    const index = tabs.findIndex((t) => t.id === tabId);
    const newTabs = tabs.filter((t) => t.id !== tabId);

    this.setMapValue(tabsGetter, tabsSetter, this.activeConnectionId, newTabs);

    const currentActiveId = activeIdGetter().get(this.activeConnectionId);
    if (currentActiveId === tabId) {
      let newActiveId: string | null = null;
      if (newTabs.length > 0) {
        const newIndex = Math.min(index, newTabs.length - 1);
        newActiveId = newTabs[newIndex]?.id || null;
      }
      this.setMapValue(activeIdGetter, activeIdSetter, this.activeConnectionId, newActiveId);
    }
  }

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

  explainTabs = $derived(
    this.activeConnectionId
      ? this.explainTabsByConnection.get(this.activeConnectionId) || []
      : [],
  );

  activeExplainTabId = $derived(
    this.activeConnectionId
      ? this.activeExplainTabIdByConnection.get(this.activeConnectionId) || null
      : null,
  );

  activeExplainTab = $derived(
    this.explainTabs.find((t) => t.id === this.activeExplainTabId) || null,
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
            sshTunnel: persisted.sshTunnel,
            // database is undefined - user needs to provide password to connect
          };
          this.connections.push(connection);
          this.initializeConnectionMaps(connection.id);
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
        sshTunnel: connection.sshTunnel,
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

  async addConnection(connection: Omit<DatabaseConnection, "id"> & {
    sshPassword?: string;
    sshKeyPath?: string;
    sshKeyPassphrase?: string;
  }) {
    let effectiveConnectionString = connection.connectionString;
    let tunnelLocalPort: number | undefined;

    // Establish SSH tunnel if enabled
    if (connection.sshTunnel?.enabled) {
      try {
        const tunnelResult = await createSshTunnel({
          sshHost: connection.sshTunnel.host,
          sshPort: connection.sshTunnel.port,
          sshUsername: connection.sshTunnel.username,
          authMethod: connection.sshTunnel.authMethod,
          password: connection.sshPassword,
          keyPath: connection.sshKeyPath,
          keyPassphrase: connection.sshKeyPassphrase,
          remoteHost: connection.host,
          remotePort: connection.port,
        });

        tunnelLocalPort = tunnelResult.localPort;

        // Build new connection string using tunnel
        if (effectiveConnectionString) {
          const url = new URL(effectiveConnectionString.replace("postgresql://", "postgres://"));
          url.hostname = "127.0.0.1";
          url.port = String(tunnelResult.localPort);
          effectiveConnectionString = url.toString();
        }

        toast.success(`SSH tunnel established on port ${tunnelResult.localPort}`);

        // Store tunnel ID for later cleanup
        const connectionId = `conn-${connection.host}-${connection.port}`;
        this.tunnelIds.set(connectionId, tunnelResult.tunnelId);
      } catch (error) {
        toast.error(`SSH tunnel failed: ${error}`);
        throw error;
      }
    }

    const newConnection: DatabaseConnection = {
      ...connection,
      id: `conn-${connection.host}-${connection.port}`,
      lastConnected: new Date(),
      tunnelLocalPort,
      database: effectiveConnectionString
        ? await Database.load(effectiveConnectionString)
        : undefined,
    };

    if (!this.connections.find((c) => c.id === newConnection.id)) {
      this.connections.push(newConnection);
    }

    this.activeConnectionId = newConnection.id;
    this.initializeConnectionMaps(newConnection.id);

    const adapter = getAdapter(newConnection.type);
    const schemasWithTablesDbResult = await newConnection.database!
      .select(adapter.getSchemaQuery());

    const schemasWithTables: SchemaTable[] = adapter.parseSchemaResult(
      schemasWithTablesDbResult as unknown[]
    );

    // Store tables immediately (without column metadata) so UI is responsive
    const newSchemas = new Map(this.schemas);
    newSchemas.set(newConnection.id, schemasWithTables);
    this.schemas = newSchemas;

    // Load column metadata asynchronously in the background
    this.loadTableMetadataInBackground(newConnection.id, schemasWithTables, adapter, newConnection.database!);

    // Create initial query tab for new connection
    this.addQueryTab();

    // Persist the connection to store (without password)
    this.persistConnection(newConnection);

    return newConnection.id;
  }

  async reconnectConnection(connectionId: string, connection: Omit<DatabaseConnection, "id"> & {
    sshPassword?: string;
    sshKeyPath?: string;
    sshKeyPassphrase?: string;
  }) {
    const existingConnection = this.connections.find((c) => c.id === connectionId);
    if (!existingConnection) {
      throw new Error(`Connection with id ${connectionId} not found`);
    }

    // Close existing tunnel if any
    const existingTunnelId = this.tunnelIds.get(connectionId);
    if (existingTunnelId) {
      try {
        await closeSshTunnel(existingTunnelId);
      } catch {
        // Ignore cleanup errors
      }
      this.tunnelIds.delete(connectionId);
    }

    let effectiveConnectionString = connection.connectionString;
    let tunnelLocalPort: number | undefined;

    // Establish SSH tunnel if enabled
    if (connection.sshTunnel?.enabled) {
      try {
        const tunnelResult = await createSshTunnel({
          sshHost: connection.sshTunnel.host,
          sshPort: connection.sshTunnel.port,
          sshUsername: connection.sshTunnel.username,
          authMethod: connection.sshTunnel.authMethod,
          password: connection.sshPassword,
          keyPath: connection.sshKeyPath,
          keyPassphrase: connection.sshKeyPassphrase,
          remoteHost: connection.host,
          remotePort: connection.port,
        });

        tunnelLocalPort = tunnelResult.localPort;

        // Build new connection string using tunnel
        if (effectiveConnectionString) {
          const url = new URL(effectiveConnectionString.replace("postgresql://", "postgres://"));
          url.hostname = "127.0.0.1";
          url.port = String(tunnelResult.localPort);
          effectiveConnectionString = url.toString();
        }

        toast.success(`SSH tunnel established on port ${tunnelResult.localPort}`);
        this.tunnelIds.set(connectionId, tunnelResult.tunnelId);
      } catch (error) {
        toast.error(`SSH tunnel failed: ${error}`);
        throw error;
      }
    }

    // Update the existing connection with the new database connection
    const database = effectiveConnectionString
      ? await Database.load(effectiveConnectionString)
      : undefined;

    existingConnection.database = database;
    existingConnection.lastConnected = new Date();
    existingConnection.password = connection.password;
    existingConnection.tunnelLocalPort = tunnelLocalPort;
    existingConnection.sshTunnel = connection.sshTunnel;
    this.ensureConnectionMapsExist(connectionId);

    // Fetch schemas
    const adapter = getAdapter(existingConnection.type);
    const schemasWithTablesDbResult = await database!
      .select(adapter.getSchemaQuery());

    const schemasWithTables: SchemaTable[] = adapter.parseSchemaResult(
      schemasWithTablesDbResult as unknown[]
    );

    // Store tables immediately (without column metadata) so UI is responsive
    const newSchemas = new Map(this.schemas);
    newSchemas.set(connectionId, schemasWithTables);
    this.schemas = newSchemas;

    // Load column metadata asynchronously in the background
    this.loadTableMetadataInBackground(connectionId, schemasWithTables, adapter, database!);

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
    // Close SSH tunnel if exists
    const tunnelId = this.tunnelIds.get(id);
    if (tunnelId) {
      closeSshTunnel(tunnelId).catch(console.error);
      this.tunnelIds.delete(id);
    }

    // Remove from persistence
    this.removePersistedConnection(id);
    this.connections = this.connections.filter((c) => c.id !== id);
    this.cleanupConnectionMaps(id);

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
    this.removeTabGeneric(
      () => this.queryTabsByConnection,
      m => this.queryTabsByConnection = m,
      () => this.activeQueryTabIdByConnection,
      m => this.activeQueryTabIdByConnection = m,
      id
    );
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

  /**
   * Find a query tab by its query content and focus it, or create a new one if not found.
   * Returns the tab ID.
   */
  focusOrCreateQueryTab(query: string, name?: string): string | null {
    if (!this.activeConnectionId) return null;

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const existingTab = tabs.find((t) => t.query.trim() === query.trim());

    if (existingTab) {
      this.setActiveQueryTab(existingTab.id);
      this.setActiveView("query");
      return existingTab.id;
    }

    // Create new tab if not found
    return this.addQueryTab(name, query);
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

  /**
   * Load column and index metadata for all tables in the background.
   * Updates the schema state progressively as each table's metadata is loaded.
   */
  private loadTableMetadataInBackground(
    connectionId: string,
    tables: SchemaTable[],
    adapter: DatabaseAdapter,
    database: Database
  ): void {
    // Process tables in parallel but update state as each completes
    tables.forEach(async (table, index) => {
      try {
        const columnsResult = (await database.select(
          adapter.getColumnsQuery(table.name, table.schema)
        )) as unknown[];

        const indexesResult = (await database.select(
          adapter.getIndexesQuery(table.name, table.schema)
        )) as unknown[];

        // Fetch foreign keys if adapter supports it (needed for SQLite)
        let foreignKeysResult: unknown[] | undefined;
        if (adapter.getForeignKeysQuery) {
          foreignKeysResult = (await database.select(
            adapter.getForeignKeysQuery(table.name, table.schema)
          )) as unknown[];
        }

        const updatedTable: SchemaTable = {
          ...table,
          columns: adapter.parseColumnsResult(columnsResult || [], foreignKeysResult),
          indexes: adapter.parseIndexesResult(indexesResult || []),
        };

        // Update the schema state with the new table metadata
        const currentSchemas = this.schemas.get(connectionId);
        if (currentSchemas) {
          const updatedSchemas = [...currentSchemas];
          updatedSchemas[index] = updatedTable;
          const newSchemas = new Map(this.schemas);
          newSchemas.set(connectionId, updatedSchemas);
          this.schemas = newSchemas;
        }
      } catch (error) {
        console.error(`Failed to load metadata for table ${table.schema}.${table.name}:`, error);
      }
    });
  }

  async addSchemaTab(table: SchemaTable) {
    if (!this.activeConnectionId || !this.activeConnection) return null;

    const tabs = this.schemaTabsByConnection.get(this.activeConnectionId) || [];
    const adapter = getAdapter(this.activeConnection.type);

    // Fetch table metadata - query columns and indexes
    const columnsResult = (await this.activeConnection.database!.select(
      adapter.getColumnsQuery(table.name, table.schema)
    )) as unknown[];

    const indexesResult = (await this.activeConnection.database!.select(
      adapter.getIndexesQuery(table.name, table.schema)
    )) as unknown[];

    // Fetch foreign keys if adapter supports it (needed for SQLite)
    let foreignKeysResult: unknown[] | undefined;
    if (adapter.getForeignKeysQuery) {
      foreignKeysResult = (await this.activeConnection.database!.select(
        adapter.getForeignKeysQuery(table.name, table.schema)
      )) as unknown[];
    }

    // Update table with fetched columns and indexes
    const updatedTable: SchemaTable = {
      ...table,
      columns: adapter.parseColumnsResult(columnsResult || [], foreignKeysResult),
      indexes: adapter.parseIndexesResult(indexesResult || []),
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
    this.removeTabGeneric(
      () => this.schemaTabsByConnection,
      m => this.schemaTabsByConnection = m,
      () => this.activeSchemaTabIdByConnection,
      m => this.activeSchemaTabIdByConnection = m,
      id
    );
  }

  setActiveSchemaTab(id: string) {
    if (!this.activeConnectionId) return;

    const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
    newActiveSchemaIds.set(this.activeConnectionId, id);
    this.activeSchemaTabIdByConnection = newActiveSchemaIds;
  }

  private readonly DEFAULT_PAGE_SIZE = 100;

  async executeQuery(tabId: string, page: number = 1, pageSize?: number) {
    if (!this.activeConnectionId) return;

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    tab.isExecuting = true;
    const effectivePageSize = pageSize ?? tab.results?.pageSize ?? this.DEFAULT_PAGE_SIZE;

    try {
      const start = performance.now();
      const baseQuery = tab.query.replace(/;$/, '');
      const queryType = detectQueryType(baseQuery);

      // Handle write queries (INSERT/UPDATE/DELETE)
      if (isWriteQuery(baseQuery)) {
        const executeResult = await this.activeConnection?.database!.execute(baseQuery);
        const totalMs = performance.now() - start;

        const results: QueryResult = {
          columns: ['Result'],
          rows: [{ Result: `${executeResult?.rowsAffected ?? 0} row(s) affected` }],
          rowCount: 1,
          totalRows: 1,
          executionTime: Math.round(totalMs * 100) / 100,
          affectedRows: executeResult?.rowsAffected ?? 0,
          lastInsertId: executeResult?.lastInsertId,
          queryType,
          page: 1,
          pageSize: 1,
          totalPages: 1,
        };

        tab.results = results;
        tab.isExecuting = false;

        // Add to history
        this.addToHistory(tab.query, results);
        return;
      }

      // Handle SELECT queries
      // Check if query already has LIMIT clause - if so, skip pagination
      const hasLimit = /\bLIMIT\b/i.test(baseQuery);

      let totalRows = 0;
      let paginatedQuery = baseQuery;

      if (!hasLimit) {
        // Get total count first by wrapping in a subquery
        const countQuery = `SELECT COUNT(*) as total FROM (${baseQuery}) AS count_query`;
        try {
          const countResult = (await this.activeConnection?.database!.select(countQuery)) as any[];
          totalRows = parseInt(countResult[0]?.total ?? '0', 10);
        } catch {
          // If count fails, just run the query without pagination
          totalRows = -1;
        }

        // Add LIMIT/OFFSET if we successfully got a count (it's a SELECT)
        if (totalRows >= 0) {
          const offset = (page - 1) * effectivePageSize;
          paginatedQuery = `${baseQuery} LIMIT ${effectivePageSize} OFFSET ${offset}`;
        }
      } else {
        // Query has its own LIMIT, don't paginate
        totalRows = -1;
      }

      const dbResult = (await this.activeConnection?.database!.select(paginatedQuery)) as any[];
      const totalMs = performance.now() - start;

      // If count failed or query had LIMIT, use result length as total
      if (totalRows < 0) {
        totalRows = dbResult?.length ?? 0;
      }

      const totalPages = hasLimit ? 1 : Math.max(1, Math.ceil(totalRows / effectivePageSize));

      // Try to extract source table info for CRUD operations
      const tableInfo = extractTableFromSelect(baseQuery);
      let sourceTable: QueryResult['sourceTable'] | undefined;

      if (tableInfo) {
        const schema = tableInfo.schema || 'public';
        const primaryKeys = this.getPrimaryKeysForTable(schema, tableInfo.table);
        if (primaryKeys.length > 0) {
          sourceTable = {
            schema,
            name: tableInfo.table,
            primaryKeys,
          };
        }
      }

      // Generate results
      const results: QueryResult = {
        columns: (dbResult?.length ?? 0) > 0 ? Object.keys(dbResult[0]) : [],
        rows: dbResult || [],
        rowCount: dbResult?.length ?? 0,
        totalRows,
        executionTime: Math.round(totalMs * 100) / 100,
        queryType,
        sourceTable,
        page,
        pageSize: effectivePageSize,
        totalPages,
      };

      tab.results = results;
      tab.isExecuting = false;

      // Add to history (only on first page to avoid duplicates)
      if (page === 1) {
        this.addToHistory(tab.query, results);
      }
    } catch (error) {
      tab.isExecuting = false;
      toast.error(`Query failed: ${error}`);
    }
  }

  private addToHistory(query: string, results: QueryResult) {
    if (!this.activeConnectionId) return;

    const queryHistory = this.queryHistoryByConnection.get(this.activeConnectionId) || [];
    const newQueryHistory = new Map(this.queryHistoryByConnection);
    newQueryHistory.set(this.activeConnectionId, [
      {
        id: `hist-${Date.now()}`,
        query,
        timestamp: new Date(),
        executionTime: results.executionTime,
        rowCount: results.affectedRows ?? results.totalRows,
        connectionId: this.activeConnectionId,
        favorite: false,
      },
      ...queryHistory,
    ]);
    this.queryHistoryByConnection = newQueryHistory;
  }

  getPrimaryKeysForTable(schema: string, tableName: string): string[] {
    if (!this.activeConnectionId) return [];
    const tables = this.schemas.get(this.activeConnectionId) || [];
    const table = tables.find(t => t.name === tableName && t.schema === schema);
    if (!table) return [];
    return table.columns.filter(c => c.isPrimaryKey).map(c => c.name);
  }

  async updateCell(
    tabId: string,
    rowIndex: number,
    column: string,
    newValue: unknown,
    sourceTable: { schema: string; name: string; primaryKeys: string[] }
  ): Promise<{ success: boolean; error?: string }> {
    const tabs = this.queryTabsByConnection.get(this.activeConnectionId!) || [];
    const tab = tabs.find(t => t.id === tabId);
    if (!tab?.results) return { success: false, error: 'No results' };

    const row = tab.results.rows[rowIndex];
    if (!row) return { success: false, error: 'Row not found' };

    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: 'No primary key found' };
    }

    // Build parameterized query
    const whereConditions = sourceTable.primaryKeys.map((pk, i) => `"${pk}" = $${i + 2}`);
    const query = `UPDATE "${sourceTable.schema}"."${sourceTable.name}" SET "${column}" = $1 WHERE ${whereConditions.join(' AND ')}`;
    const bindValues = [newValue, ...sourceTable.primaryKeys.map(pk => row[pk])];

    try {
      await this.activeConnection?.database!.execute(query, bindValues);
      // Update the local row data
      row[column] = newValue;
      return { success: true };
    } catch (error) {
      return { success: false, error: String(error) };
    }
  }

  async insertRow(
    sourceTable: { schema: string; name: string },
    values: Record<string, unknown>
  ): Promise<{ success: boolean; error?: string; lastInsertId?: number }> {
    const columns = Object.keys(values);
    if (columns.length === 0) {
      return { success: false, error: 'No values provided' };
    }

    const columnNames = columns.map(c => `"${c}"`).join(', ');
    const placeholders = columns.map((_, i) => `$${i + 1}`).join(', ');
    const query = `INSERT INTO "${sourceTable.schema}"."${sourceTable.name}" (${columnNames}) VALUES (${placeholders})`;

    try {
      const result = await this.activeConnection?.database!.execute(query, Object.values(values));
      return { success: true, lastInsertId: result?.lastInsertId };
    } catch (error) {
      return { success: false, error: String(error) };
    }
  }

  async deleteRow(
    sourceTable: { schema: string; name: string; primaryKeys: string[] },
    row: Record<string, unknown>
  ): Promise<{ success: boolean; error?: string }> {
    if (sourceTable.primaryKeys.length === 0) {
      return { success: false, error: 'No primary key found' };
    }

    const whereConditions = sourceTable.primaryKeys.map((pk, i) => `"${pk}" = $${i + 1}`);
    const query = `DELETE FROM "${sourceTable.schema}"."${sourceTable.name}" WHERE ${whereConditions.join(' AND ')}`;
    const bindValues = sourceTable.primaryKeys.map(pk => row[pk]);

    try {
      await this.activeConnection?.database!.execute(query, bindValues);
      return { success: true };
    } catch (error) {
      return { success: false, error: String(error) };
    }
  }

  async goToPage(tabId: string, page: number) {
    const tabs = this.queryTabsByConnection.get(this.activeConnectionId!) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab?.results) return;

    const targetPage = Math.max(1, Math.min(page, tab.results.totalPages));
    await this.executeQuery(tabId, targetPage, tab.results.pageSize);
  }

  async setPageSize(tabId: string, pageSize: number) {
    await this.executeQuery(tabId, 1, pageSize);
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

  setActiveView(view: "query" | "schema" | "explain") {
    this.activeView = view;
  }

  // EXPLAIN/ANALYZE methods
  async executeExplain(tabId: string, analyze: boolean = false) {
    if (!this.activeConnectionId) return;

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab || !tab.query.trim()) return;

    // Create a new explain tab
    const explainTabs = this.explainTabsByConnection.get(this.activeConnectionId) || [];
    const explainTabId = `explain-${Date.now()}`;
    const queryPreview = tab.query.substring(0, 30).replace(/\s+/g, ' ').trim();
    const newExplainTab: ExplainTab = $state({
      id: explainTabId,
      name: analyze ? `Analyze: ${queryPreview}...` : `Explain: ${queryPreview}...`,
      sourceQuery: tab.query,
      result: undefined,
      isExecuting: true,
    });

    const newExplainTabs = new Map(this.explainTabsByConnection);
    newExplainTabs.set(this.activeConnectionId, [...explainTabs, newExplainTab]);
    this.explainTabsByConnection = newExplainTabs;

    // Set as active and switch view
    const newActiveExplainIds = new Map(this.activeExplainTabIdByConnection);
    newActiveExplainIds.set(this.activeConnectionId, explainTabId);
    this.activeExplainTabIdByConnection = newActiveExplainIds;
    this.setActiveView("explain");

    try {
      const adapter = getAdapter(this.activeConnection!.type);
      const explainQuery = adapter.getExplainQuery(tab.query, analyze);
      const result = await this.activeConnection?.database!.select(explainQuery) as unknown[];

      // Use adapter to parse the results into common format
      const parsedNode = adapter.parseExplainResult(result, analyze);

      // Convert to ExplainResult format for rendering
      const explainResult: ExplainResult = this.convertExplainNodeToResult(parsedNode, analyze);

      // Update the explain tab with results
      newExplainTab.result = explainResult;
      newExplainTab.isExecuting = false;

      // Trigger reactivity
      const updatedExplainTabs = new Map(this.explainTabsByConnection);
      updatedExplainTabs.set(this.activeConnectionId!, [...this.explainTabsByConnection.get(this.activeConnectionId!) || []]);
      this.explainTabsByConnection = updatedExplainTabs;

    } catch (error) {
      // Remove failed explain tab
      const updatedExplainTabs = new Map(this.explainTabsByConnection);
      const currentTabs = updatedExplainTabs.get(this.activeConnectionId!) || [];
      updatedExplainTabs.set(this.activeConnectionId!, currentTabs.filter(t => t.id !== explainTabId));
      this.explainTabsByConnection = updatedExplainTabs;

      // Switch back to query view
      this.setActiveView("query");
      toast.error(`Explain failed: ${error}`);
    }
  }

  private convertExplainNodeToResult(node: ExplainNode, isAnalyze: boolean): ExplainResult {
    let nodeCounter = 0;

    const convertNode = (n: ExplainNode): ExplainPlanNode => {
      const id = `node-${nodeCounter++}`;

      return {
        id,
        nodeType: n.type,
        relationName: undefined,
        alias: undefined,
        startupCost: 0,
        totalCost: n.cost || 0,
        planRows: n.rows || 0,
        planWidth: 0,
        actualStartupTime: undefined,
        actualTotalTime: n.actualTime,
        actualRows: n.actualRows,
        actualLoops: undefined,
        filter: n.label !== n.type ? n.label : undefined,
        indexName: undefined,
        indexCond: undefined,
        joinType: undefined,
        hashCond: undefined,
        sortKey: undefined,
        children: (n.children || []).map((child) => convertNode(child)),
      };
    };

    return {
      plan: convertNode(node),
      planningTime: 0,
      executionTime: undefined,
      isAnalyze,
    };
  }

  removeExplainTab(id: string) {
    this.removeTabGeneric(
      () => this.explainTabsByConnection,
      m => this.explainTabsByConnection = m,
      () => this.activeExplainTabIdByConnection,
      m => this.activeExplainTabIdByConnection = m,
      id
    );
    // Switch to query view if no explain tabs left
    if (this.activeConnectionId && this.explainTabs.length === 0) {
      this.setActiveView("query");
    }
  }

  setActiveExplainTab(id: string) {
    if (!this.activeConnectionId) return;

    const newActiveExplainIds = new Map(this.activeExplainTabIdByConnection);
    newActiveExplainIds.set(this.activeConnectionId, id);
    this.activeExplainTabIdByConnection = newActiveExplainIds;
  }
}

export const setDatabase = () => setContext("database", new UseDatabase());
export const useDatabase = () =>
  getContext<ReturnType<typeof setDatabase>>("database");
