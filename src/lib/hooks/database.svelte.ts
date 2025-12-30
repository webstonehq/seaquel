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
  ExplainTab,
  ExplainResult,
  ExplainPlanNode,
  ErdTab,
  PersistedConnectionState,
  PersistedQueryTab,
  PersistedSchemaTab,
  PersistedExplainTab,
  PersistedErdTab,
  PersistedSavedQuery,
  PersistedQueryHistoryItem,
} from "$lib/types";
import Database from "@tauri-apps/plugin-sql";
import { load } from "@tauri-apps/plugin-store";
import { toast } from "svelte-sonner";
import { getAdapter, type DatabaseAdapter, type ExplainNode } from "$lib/db";
import { detectQueryType, isWriteQuery, extractTableFromSelect } from "$lib/db/query-utils";
import { createSshTunnel, closeSshTunnel } from "$lib/services/ssh-tunnel";
import type { PersistedConnection } from "./database/types.js";
import { setMapValue, deleteMapKey, updateMapArrayItem } from "./database/map-utils.js";
import { DatabaseState } from "./database/state.svelte.js";
import { UIStateManager } from "./database/ui-state.svelte.js";
import { QueryHistoryManager } from "./database/query-history.svelte.js";
import { QueryTabManager } from "./database/query-tabs.svelte.js";
import { SavedQueryManager } from "./database/saved-queries.svelte.js";
import { SchemaTabManager } from "./database/schema-tabs.svelte.js";
import { ExplainTabManager } from "./database/explain-tabs.svelte.js";

class UseDatabase extends DatabaseState {
  // Managers
  private _uiState: UIStateManager;
  private _queryHistory: QueryHistoryManager;
  private _queryTabs: QueryTabManager;
  private _savedQueries: SavedQueryManager;
  private _schemaTabs: SchemaTabManager;
  private _explainTabs: ExplainTabManager;

  // Map connection IDs to their SSH tunnel IDs for cleanup
  private tunnelIds = new Map<string, string>();

  // Persistence infrastructure
  private persistenceTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly PERSISTENCE_DEBOUNCE_MS = 500;
  private readonly MAX_HISTORY_ITEMS = 500;

  private schedulePersistence(connectionId: string | null) {
    if (!connectionId) return;

    if (this.persistenceTimer) {
      clearTimeout(this.persistenceTimer);
    }
    this.persistenceTimer = setTimeout(() => {
      this.persistConnectionState(connectionId);
      this.persistenceTimer = null;
    }, this.PERSISTENCE_DEBOUNCE_MS);
  }

  flushPersistence() {
    if (this.persistenceTimer) {
      clearTimeout(this.persistenceTimer);
      this.persistenceTimer = null;
    }
    // Persist all connections that have data
    for (const connectionId of this.queryTabsByConnection.keys()) {
      this.persistConnectionState(connectionId);
    }
  }

  /**
   * Cleans up resources and flushes pending state before unmounting.
   * Should be called when the component is destroyed.
   */
  cleanup() {
    this.flushPersistence();
  }

  /**
   * Formats an unknown error into a user-friendly string message.
   * Preserves the error message if it's an Error instance.
   */
  private formatError(error: unknown): string {
    if (error instanceof Error) {
      return error.message;
    }
    return String(error);
  }

  private serializeQueryTabs(connectionId: string): PersistedQueryTab[] {
    const tabs = this.queryTabsByConnection.get(connectionId) || [];
    return tabs.map(tab => ({
      id: tab.id,
      name: tab.name,
      query: tab.query,
      savedQueryId: tab.savedQueryId,
    }));
  }

  private serializeSchemaTabs(connectionId: string): PersistedSchemaTab[] {
    const tabs = this.schemaTabsByConnection.get(connectionId) || [];
    return tabs.map(tab => ({
      id: tab.id,
      tableName: tab.table.name,
      schemaName: tab.table.schema,
    }));
  }

  private serializeExplainTabs(connectionId: string): PersistedExplainTab[] {
    const tabs = this.explainTabsByConnection.get(connectionId) || [];
    return tabs.map(tab => ({
      id: tab.id,
      name: tab.name,
      sourceQuery: tab.sourceQuery,
    }));
  }

  private serializeErdTabs(connectionId: string): PersistedErdTab[] {
    const tabs = this.erdTabsByConnection.get(connectionId) || [];
    return tabs.map(tab => ({
      id: tab.id,
      name: tab.name,
    }));
  }

  private serializeSavedQueries(connectionId: string): PersistedSavedQuery[] {
    const queries = this.savedQueriesByConnection.get(connectionId) || [];
    return queries.map(q => ({
      id: q.id,
      name: q.name,
      query: q.query,
      connectionId: q.connectionId,
      createdAt: q.createdAt.toISOString(),
      updatedAt: q.updatedAt.toISOString(),
    }));
  }

  private serializeQueryHistory(connectionId: string): PersistedQueryHistoryItem[] {
    const history = this.queryHistoryByConnection.get(connectionId) || [];
    return history.slice(0, this.MAX_HISTORY_ITEMS).map(h => ({
      id: h.id,
      query: h.query,
      timestamp: h.timestamp.toISOString(),
      executionTime: h.executionTime,
      rowCount: h.rowCount,
      connectionId: h.connectionId,
      favorite: h.favorite,
    }));
  }

  private async persistConnectionState(connectionId: string) {
    try {
      const store = await load(`connection_state_${connectionId}.json`, {
        autoSave: true,
        defaults: { state: null },
      });

      const state: PersistedConnectionState = {
        connectionId,
        queryTabs: this.serializeQueryTabs(connectionId),
        schemaTabs: this.serializeSchemaTabs(connectionId),
        explainTabs: this.serializeExplainTabs(connectionId),
        erdTabs: this.serializeErdTabs(connectionId),
        tabOrder: this.tabOrderByConnection.get(connectionId) || [],
        activeQueryTabId: this.activeQueryTabIdByConnection.get(connectionId) || null,
        activeSchemaTabId: this.activeSchemaTabIdByConnection.get(connectionId) || null,
        activeExplainTabId: this.activeExplainTabIdByConnection.get(connectionId) || null,
        activeErdTabId: this.activeErdTabIdByConnection.get(connectionId) || null,
        activeView: this.activeView,
        savedQueries: this.serializeSavedQueries(connectionId),
        queryHistory: this.serializeQueryHistory(connectionId),
      };

      await store.set("state", state);
      await store.save();
    } catch (error) {
      console.error(`Failed to persist state for connection ${connectionId}:`, error);
    }
  }

  private async loadPersistedConnectionState(connectionId: string): Promise<PersistedConnectionState | null> {
    try {
      const store = await load(`connection_state_${connectionId}.json`, {
        autoSave: false,
        defaults: { state: null },
      });
      return (await store.get("state")) as PersistedConnectionState | null;
    } catch (error) {
      console.error(`Failed to load persisted state for ${connectionId}:`, error);
      return null;
    }
  }

  private async removePersistedConnectionState(connectionId: string) {
    try {
      const store = await load(`connection_state_${connectionId}.json`, {
        autoSave: true,
        defaults: { state: null },
      });
      await store.clear();
      await store.save();
    } catch (error) {
      console.error(`Failed to remove persisted state for ${connectionId}:`, error);
    }
  }

  private restoreSavedQueries(connectionId: string, data: PersistedSavedQuery[]) {
    const savedQueries: SavedQuery[] = data.map(q => ({
      id: q.id,
      name: q.name,
      query: q.query,
      connectionId: q.connectionId,
      createdAt: new Date(q.createdAt),
      updatedAt: new Date(q.updatedAt),
    }));
    setMapValue(
      () => this.savedQueriesByConnection,
      m => this.savedQueriesByConnection = m,
      connectionId,
      savedQueries
    );
  }

  private restoreQueryHistory(connectionId: string, data: PersistedQueryHistoryItem[]) {
    const history: QueryHistoryItem[] = data.map(h => ({
      id: h.id,
      query: h.query,
      timestamp: new Date(h.timestamp),
      executionTime: h.executionTime,
      rowCount: h.rowCount,
      connectionId: h.connectionId,
      favorite: h.favorite,
    }));
    setMapValue(
      () => this.queryHistoryByConnection,
      m => this.queryHistoryByConnection = m,
      connectionId,
      history
    );
  }

  private async restoreConnectionTabs(connectionId: string) {
    const state = await this.loadPersistedConnectionState(connectionId);
    if (!state) return false;

    // Restore query tabs (without results - they'll need to be re-executed)
    const queryTabs: QueryTab[] = state.queryTabs.map(pt => ({
      id: pt.id,
      name: pt.name,
      query: pt.query,
      savedQueryId: pt.savedQueryId,
      isExecuting: false,
      results: undefined,
    }));
    setMapValue(
      () => this.queryTabsByConnection,
      m => this.queryTabsByConnection = m,
      connectionId,
      queryTabs
    );

    // Restore explain tabs (without results - they'll need to be re-executed)
    const explainTabs: ExplainTab[] = state.explainTabs.map(pt => ({
      id: pt.id,
      name: pt.name,
      sourceQuery: pt.sourceQuery,
      isExecuting: false,
      result: undefined,
    }));
    setMapValue(
      () => this.explainTabsByConnection,
      m => this.explainTabsByConnection = m,
      connectionId,
      explainTabs
    );

    // Restore schema tabs - need to match against current schema
    await this.restoreSchemaTabs(connectionId, state.schemaTabs);

    // Restore active tab IDs (only if tabs exist)
    if (state.activeQueryTabId && queryTabs.some(t => t.id === state.activeQueryTabId)) {
      setMapValue(
        () => this.activeQueryTabIdByConnection,
        m => this.activeQueryTabIdByConnection = m,
        connectionId,
        state.activeQueryTabId
      );
    } else if (queryTabs.length > 0) {
      setMapValue(
        () => this.activeQueryTabIdByConnection,
        m => this.activeQueryTabIdByConnection = m,
        connectionId,
        queryTabs[0].id
      );
    }

    const schemaTabs = this.schemaTabsByConnection.get(connectionId) || [];
    if (state.activeSchemaTabId && schemaTabs.some(t => t.id === state.activeSchemaTabId)) {
      setMapValue(
        () => this.activeSchemaTabIdByConnection,
        m => this.activeSchemaTabIdByConnection = m,
        connectionId,
        state.activeSchemaTabId
      );
    } else if (schemaTabs.length > 0) {
      setMapValue(
        () => this.activeSchemaTabIdByConnection,
        m => this.activeSchemaTabIdByConnection = m,
        connectionId,
        schemaTabs[0].id
      );
    }

    if (state.activeExplainTabId && explainTabs.some(t => t.id === state.activeExplainTabId)) {
      setMapValue(
        () => this.activeExplainTabIdByConnection,
        m => this.activeExplainTabIdByConnection = m,
        connectionId,
        state.activeExplainTabId
      );
    } else if (explainTabs.length > 0) {
      setMapValue(
        () => this.activeExplainTabIdByConnection,
        m => this.activeExplainTabIdByConnection = m,
        connectionId,
        explainTabs[0].id
      );
    }

    // Restore tab order (if available, otherwise will use timestamp ordering)
    if (state.tabOrder && state.tabOrder.length > 0) {
      setMapValue(
        () => this.tabOrderByConnection,
        m => this.tabOrderByConnection = m,
        connectionId,
        state.tabOrder
      );
    }

    // Restore active view
    this.activeView = state.activeView;

    return queryTabs.length > 0;
  }

  private async restoreSchemaTabs(connectionId: string, persistedTabs: PersistedSchemaTab[]) {
    const schemas = this.schemas.get(connectionId) || [];
    const schemaTabs: SchemaTab[] = [];

    for (const pt of persistedTabs) {
      // Find matching table in current schema
      const table = schemas.find(
        t => t.name === pt.tableName && t.schema === pt.schemaName
      );

      if (table) {
        schemaTabs.push({
          id: pt.id,
          table: table,
        });
      }
      // If table no longer exists, skip restoring this tab
    }

    setMapValue(
      () => this.schemaTabsByConnection,
      m => this.schemaTabsByConnection = m,
      connectionId,
      schemaTabs
    );
  }

  // Initialize all per-connection maps for a new or reconnected connection
  private initializeConnectionMaps(connectionId: string): void {
    setMapValue(() => this.queryTabsByConnection, m => this.queryTabsByConnection = m, connectionId, []);
    setMapValue(() => this.schemaTabsByConnection, m => this.schemaTabsByConnection = m, connectionId, []);
    setMapValue(() => this.activeQueryTabIdByConnection, m => this.activeQueryTabIdByConnection = m, connectionId, null);
    setMapValue(() => this.activeSchemaTabIdByConnection, m => this.activeSchemaTabIdByConnection = m, connectionId, null);
    setMapValue(() => this.queryHistoryByConnection, m => this.queryHistoryByConnection = m, connectionId, []);
    setMapValue(() => this.savedQueriesByConnection, m => this.savedQueriesByConnection = m, connectionId, []);
    setMapValue(() => this.explainTabsByConnection, m => this.explainTabsByConnection = m, connectionId, []);
    setMapValue(() => this.activeExplainTabIdByConnection, m => this.activeExplainTabIdByConnection = m, connectionId, null);
    setMapValue(() => this.erdTabsByConnection, m => this.erdTabsByConnection = m, connectionId, []);
    setMapValue(() => this.activeErdTabIdByConnection, m => this.activeErdTabIdByConnection = m, connectionId, null);
    setMapValue(() => this.tabOrderByConnection, m => this.tabOrderByConnection = m, connectionId, []);
    setMapValue(() => this.schemas, m => this.schemas = m, connectionId, []);
  }

  // Clean up all per-connection maps when removing a connection
  private cleanupConnectionMaps(connectionId: string): void {
    deleteMapKey(() => this.queryTabsByConnection, m => this.queryTabsByConnection = m, connectionId);
    deleteMapKey(() => this.schemaTabsByConnection, m => this.schemaTabsByConnection = m, connectionId);
    deleteMapKey(() => this.activeQueryTabIdByConnection, m => this.activeQueryTabIdByConnection = m, connectionId);
    deleteMapKey(() => this.activeSchemaTabIdByConnection, m => this.activeSchemaTabIdByConnection = m, connectionId);
    deleteMapKey(() => this.queryHistoryByConnection, m => this.queryHistoryByConnection = m, connectionId);
    deleteMapKey(() => this.savedQueriesByConnection, m => this.savedQueriesByConnection = m, connectionId);
    deleteMapKey(() => this.explainTabsByConnection, m => this.explainTabsByConnection = m, connectionId);
    deleteMapKey(() => this.activeExplainTabIdByConnection, m => this.activeExplainTabIdByConnection = m, connectionId);
    deleteMapKey(() => this.erdTabsByConnection, m => this.erdTabsByConnection = m, connectionId);
    deleteMapKey(() => this.activeErdTabIdByConnection, m => this.activeErdTabIdByConnection = m, connectionId);
    deleteMapKey(() => this.tabOrderByConnection, m => this.tabOrderByConnection = m, connectionId);
    deleteMapKey(() => this.schemas, m => this.schemas = m, connectionId);
  }

  // Ensure maps exist for a connection (used during reconnect to preserve existing state)
  private ensureConnectionMapsExist(connectionId: string): void {
    if (!this.queryTabsByConnection.has(connectionId)) {
      setMapValue(() => this.queryTabsByConnection, m => this.queryTabsByConnection = m, connectionId, []);
    }
    if (!this.schemaTabsByConnection.has(connectionId)) {
      setMapValue(() => this.schemaTabsByConnection, m => this.schemaTabsByConnection = m, connectionId, []);
    }
    if (!this.activeQueryTabIdByConnection.has(connectionId)) {
      setMapValue(() => this.activeQueryTabIdByConnection, m => this.activeQueryTabIdByConnection = m, connectionId, null);
    }
    if (!this.activeSchemaTabIdByConnection.has(connectionId)) {
      setMapValue(() => this.activeSchemaTabIdByConnection, m => this.activeSchemaTabIdByConnection = m, connectionId, null);
    }
    if (!this.queryHistoryByConnection.has(connectionId)) {
      setMapValue(() => this.queryHistoryByConnection, m => this.queryHistoryByConnection = m, connectionId, []);
    }
    if (!this.savedQueriesByConnection.has(connectionId)) {
      setMapValue(() => this.savedQueriesByConnection, m => this.savedQueriesByConnection = m, connectionId, []);
    }
    if (!this.explainTabsByConnection.has(connectionId)) {
      setMapValue(() => this.explainTabsByConnection, m => this.explainTabsByConnection = m, connectionId, []);
    }
    if (!this.activeExplainTabIdByConnection.has(connectionId)) {
      setMapValue(() => this.activeExplainTabIdByConnection, m => this.activeExplainTabIdByConnection = m, connectionId, null);
    }
    if (!this.erdTabsByConnection.has(connectionId)) {
      setMapValue(() => this.erdTabsByConnection, m => this.erdTabsByConnection = m, connectionId, []);
    }
    if (!this.activeErdTabIdByConnection.has(connectionId)) {
      setMapValue(() => this.activeErdTabIdByConnection, m => this.activeErdTabIdByConnection = m, connectionId, null);
    }
    if (!this.tabOrderByConnection.has(connectionId)) {
      setMapValue(() => this.tabOrderByConnection, m => this.tabOrderByConnection = m, connectionId, []);
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

    setMapValue(tabsGetter, tabsSetter, this.activeConnectionId, newTabs);

    // Remove from tab order
    this.removeFromTabOrder(tabId);

    const currentActiveId = activeIdGetter().get(this.activeConnectionId);
    if (currentActiveId === tabId) {
      let newActiveId: string | null = null;
      if (newTabs.length > 0) {
        const newIndex = Math.min(index, newTabs.length - 1);
        newActiveId = newTabs[newIndex]?.id || null;
      }
      setMapValue(activeIdGetter, activeIdSetter, this.activeConnectionId, newActiveId);
    }
  }

  // Tab ordering methods
  private addToTabOrder(tabId: string): void {
    if (!this.activeConnectionId) return;
    const order = this.tabOrderByConnection.get(this.activeConnectionId) || [];
    if (!order.includes(tabId)) {
      setMapValue(
        () => this.tabOrderByConnection,
        m => this.tabOrderByConnection = m,
        this.activeConnectionId,
        [...order, tabId]
      );
    }
  }

  private removeFromTabOrder(tabId: string): void {
    if (!this.activeConnectionId) return;
    const order = this.tabOrderByConnection.get(this.activeConnectionId) || [];
    setMapValue(
      () => this.tabOrderByConnection,
      m => this.tabOrderByConnection = m,
      this.activeConnectionId,
      order.filter(id => id !== tabId)
    );
  }

  reorderTabs(newOrder: string[]): void {
    if (!this.activeConnectionId) return;
    setMapValue(
      () => this.tabOrderByConnection,
      m => this.tabOrderByConnection = m,
      this.activeConnectionId,
      newOrder
    );
    this.schedulePersistence(this.activeConnectionId);
  }

  // Helper to extract timestamp from tab ID for default ordering
  private getTabTimestamp(id: string): number {
    const match = id.match(/\d+$/);
    return match ? parseInt(match[0], 10) : 0;
  }

  get orderedTabs(): Array<{id: string, type: 'query' | 'schema' | 'explain' | 'erd', tab: QueryTab | SchemaTab | ExplainTab | ErdTab}> {
    if (!this.activeConnectionId) return [];

    const allTabsUnordered = [
      ...this.queryTabs.map(t => ({ id: t.id, type: 'query' as const, tab: t })),
      ...this.schemaTabs.map(t => ({ id: t.id, type: 'schema' as const, tab: t })),
      ...this.explainTabs.map(t => ({ id: t.id, type: 'explain' as const, tab: t })),
      ...this.erdTabs.map(t => ({ id: t.id, type: 'erd' as const, tab: t }))
    ];

    const order = this.tabOrderByConnection.get(this.activeConnectionId) || [];

    // Sort by order array, falling back to timestamp for new tabs
    return allTabsUnordered.sort((a, b) => {
      const aIndex = order.indexOf(a.id);
      const bIndex = order.indexOf(b.id);

      // Both in order array: use order
      if (aIndex !== -1 && bIndex !== -1) return aIndex - bIndex;

      // Only one in order: ordered comes first
      if (aIndex !== -1) return -1;
      if (bIndex !== -1) return 1;

      // Neither in order: fall back to timestamp
      return this.getTabTimestamp(a.id) - this.getTabTimestamp(b.id);
    });
  }

  constructor() {
    super();
    this._uiState = new UIStateManager(this, (id) => this.schedulePersistence(id));
    this._queryHistory = new QueryHistoryManager(this, (id) => this.schedulePersistence(id));
    this._queryTabs = new QueryTabManager(
      this,
      (id) => this.schedulePersistence(id),
      this.removeTabGeneric.bind(this),
      this.addToTabOrder.bind(this)
    );
    this._savedQueries = new SavedQueryManager(this, (id) => this.schedulePersistence(id));
    this._schemaTabs = new SchemaTabManager(
      this,
      (id) => this.schedulePersistence(id),
      this.removeTabGeneric.bind(this)
    );
    this._explainTabs = new ExplainTabManager(
      this,
      (id) => this.schedulePersistence(id),
      this.removeTabGeneric.bind(this),
      (view) => this.setActiveView(view)
    );
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

          // Pre-load saved queries and history (doesn't require active DB connection)
          const state = await this.loadPersistedConnectionState(connection.id);
          if (state) {
            this.restoreSavedQueries(connection.id, state.savedQueries);
            this.restoreQueryHistory(connection.id, state.queryHistory);
          }
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

  /**
   * Establishes an SSH tunnel if configured, returning the modified connection string
   * and local port information.
   */
  private async setupSshTunnel(
    connection: {
      sshTunnel?: DatabaseConnection['sshTunnel'];
      host: string;
      port: number;
      connectionString?: string;
    },
    credentials: {
      sshPassword?: string;
      sshKeyPath?: string;
      sshKeyPassphrase?: string;
    },
    connectionId: string
  ): Promise<{ effectiveConnectionString: string | undefined; tunnelLocalPort?: number }> {
    if (!connection.sshTunnel?.enabled) {
      return { effectiveConnectionString: connection.connectionString };
    }

    try {
      const tunnelResult = await createSshTunnel({
        sshHost: connection.sshTunnel.host,
        sshPort: connection.sshTunnel.port,
        sshUsername: connection.sshTunnel.username,
        authMethod: connection.sshTunnel.authMethod,
        password: credentials.sshPassword,
        keyPath: credentials.sshKeyPath,
        keyPassphrase: credentials.sshKeyPassphrase,
        remoteHost: connection.host,
        remotePort: connection.port,
      });

      let effectiveConnectionString = connection.connectionString;
      if (effectiveConnectionString) {
        const url = new URL(effectiveConnectionString.replace("postgresql://", "postgres://"));
        url.hostname = "127.0.0.1";
        url.port = String(tunnelResult.localPort);
        effectiveConnectionString = url.toString();
      }

      toast.success(`SSH tunnel established on port ${tunnelResult.localPort}`);
      this.tunnelIds.set(connectionId, tunnelResult.tunnelId);

      return {
        effectiveConnectionString,
        tunnelLocalPort: tunnelResult.localPort,
      };
    } catch (error) {
      toast.error(`SSH tunnel failed: ${error}`);
      throw error;
    }
  }

  async addConnection(connection: Omit<DatabaseConnection, "id"> & {
    sshPassword?: string;
    sshKeyPath?: string;
    sshKeyPassphrase?: string;
  }) {
    const connectionId =
      connection.type === 'sqlite'
        ? `conn-sqlite-${connection.databaseName}`
        : `conn-${connection.host}-${connection.port}`;

    const { effectiveConnectionString, tunnelLocalPort } = await this.setupSshTunnel(
      connection,
      {
        sshPassword: connection.sshPassword,
        sshKeyPath: connection.sshKeyPath,
        sshKeyPassphrase: connection.sshKeyPassphrase,
      },
      connectionId
    );

    const newConnection: DatabaseConnection = {
      ...connection,
      id: connectionId,
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

    const { effectiveConnectionString, tunnelLocalPort } = await this.setupSshTunnel(
      connection,
      {
        sshPassword: connection.sshPassword,
        sshKeyPath: connection.sshKeyPath,
        sshKeyPassphrase: connection.sshKeyPassphrase,
      },
      connectionId
    );

    // Update the existing connection with the new database connection
    const database = effectiveConnectionString
      ? await Database.load(effectiveConnectionString)
      : undefined;

    // Create updated connection object to ensure Svelte reactivity sees the change
    const updatedConnection: DatabaseConnection = {
      ...existingConnection,
      database,
      lastConnected: new Date(),
      password: connection.password,
      tunnelLocalPort,
      sshTunnel: connection.sshTunnel,
    };

    // Replace the old connection with the updated one in the connections array
    this.connections = this.connections.map(c =>
      c.id === connectionId ? updatedConnection : c
    );

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

    // Try to restore tabs from persisted state
    const hasRestoredTabs = await this.restoreConnectionTabs(connectionId);

    // Create initial query tab if no tabs were restored
    if (!hasRestoredTabs) {
      const tabs = this.queryTabsByConnection.get(connectionId) || [];
      if (tabs.length === 0) {
        this.addQueryTab();
      }
    }

    // Persist the connection to store (without password for security)
    this.persistConnection(updatedConnection);

    return connectionId;
  }

  async testConnection(connection: Omit<DatabaseConnection, "id"> & {
    sshPassword?: string;
    sshKeyPath?: string;
    sshKeyPassphrase?: string;
  }) {
    let effectiveConnectionString = connection.connectionString;
    let tunnelId: string | undefined;

    // Establish SSH tunnel if enabled
    if (connection.sshTunnel?.enabled) {
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

      tunnelId = tunnelResult.tunnelId;

      // Build new connection string using tunnel
      if (effectiveConnectionString) {
        const url = new URL(effectiveConnectionString.replace("postgresql://", "postgres://"));
        url.hostname = "127.0.0.1";
        url.port = String(tunnelResult.localPort);
        effectiveConnectionString = url.toString();
      }
    }

    try {
      // Try to connect to the database
      const database = await Database.load(effectiveConnectionString!);
      // Close the test connection immediately
      await database.close();
    } finally {
      // Clean up SSH tunnel if we created one
      if (tunnelId) {
        try {
          await closeSshTunnel(tunnelId);
        } catch {
          // Ignore cleanup errors
        }
      }
    }
  }

  removeConnection(id: string) {
    // Close SSH tunnel if exists
    const tunnelId = this.tunnelIds.get(id);
    if (tunnelId) {
      closeSshTunnel(tunnelId).catch(console.error);
      this.tunnelIds.delete(id);
    }

    // Remove from persistence (both connection and its state)
    this.removePersistedConnection(id);
    this.removePersistedConnectionState(id);
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
      const wasConnected = !!connection.database;
      connection.database = undefined;
      if (wasConnected) {
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

  // Query tab methods - delegated to QueryTabManager
  addQueryTab(name?: string, query?: string, savedQueryId?: string) {
    return this._queryTabs.addQueryTab(name, query, savedQueryId);
  }

  removeQueryTab(id: string) {
    this._queryTabs.removeQueryTab(id);
  }

  renameQueryTab(id: string, newName: string) {
    this._queryTabs.renameQueryTab(id, newName);
  }

  setActiveQueryTab(id: string) {
    this._queryTabs.setActiveQueryTab(id);
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

  hasUnsavedChanges(tabId: string): boolean {
    return this._queryTabs.hasUnsavedChanges(tabId);
  }

  updateQueryTabContent(id: string, query: string) {
    this._queryTabs.updateQueryTabContent(id, query);
  }

  // Saved queries - delegated to SavedQueryManager
  saveQuery(name: string, query: string, tabId?: string) {
    return this._savedQueries.saveQuery(name, query, tabId);
  }

  deleteSavedQuery(id: string) {
    this._savedQueries.deleteSavedQuery(id);
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
  private async loadTableMetadataInBackground(
    connectionId: string,
    tables: SchemaTable[],
    adapter: DatabaseAdapter,
    database: Database
  ): Promise<void> {
    // Process tables in parallel but update state as each completes
    const promises = tables.map(async (table, index) => {
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

    await Promise.allSettled(promises);
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
      this.schedulePersistence(this.activeConnectionId);
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

    this.addToTabOrder(newTab.id);

    const newActiveSchemaIds = new Map(this.activeSchemaTabIdByConnection);
    newActiveSchemaIds.set(this.activeConnectionId, newTab.id);
    this.activeSchemaTabIdByConnection = newActiveSchemaIds;

    this.schedulePersistence(this.activeConnectionId);
    return newTab.id;
  }

  // Schema tab methods - delegated to SchemaTabManager
  removeSchemaTab(id: string) {
    this._schemaTabs.removeSchemaTab(id);
  }

  setActiveSchemaTab(id: string) {
    this._schemaTabs.setActiveSchemaTab(id);
  }

  private readonly DEFAULT_PAGE_SIZE = 100;

  /**
   * Update a query tab's state with proper Svelte 5 reactivity.
   * Creates new objects to ensure derived values update.
   */
  private updateQueryTabState(tabId: string, updates: Partial<QueryTab>) {
    if (!this.activeConnectionId) return;
    updateMapArrayItem(
      () => this.queryTabsByConnection,
      m => this.queryTabsByConnection = m,
      this.activeConnectionId,
      tabId,
      updates
    );
  }

  async executeQuery(tabId: string, page: number = 1, pageSize?: number) {
    if (!this.activeConnectionId) return;

    const database = this.activeConnection?.database;
    if (!database) {
      toast.error("Not connected to database. Please reconnect.");
      return;
    }

    const tabs = this.queryTabsByConnection.get(this.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === tabId);
    if (!tab) return;

    // Mark as executing with proper reactivity
    this.updateQueryTabState(tabId, { isExecuting: true });
    const effectivePageSize = pageSize ?? tab.results?.pageSize ?? this.DEFAULT_PAGE_SIZE;

    try {
      const start = performance.now();
      const baseQuery = tab.query.replace(/;$/, '');
      const queryType = detectQueryType(baseQuery);

      // Handle write queries (INSERT/UPDATE/DELETE)
      if (isWriteQuery(baseQuery)) {
        const executeResult = await database.execute(baseQuery);
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

        // Update tab with results using proper reactivity
        this.updateQueryTabState(tabId, { results, isExecuting: false });

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
          const countResult = (await database.select(countQuery)) as any[];
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

      const dbResult = (await database.select(paginatedQuery)) as any[];
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

      // Update tab with results using proper reactivity
      this.updateQueryTabState(tabId, { results, isExecuting: false });

      // Add to history (only on first page to avoid duplicates)
      if (page === 1) {
        this.addToHistory(tab.query, results);
      }
    } catch (error) {
      this.updateQueryTabState(tabId, { isExecuting: false });
      toast.error(`Query failed: ${error}`);
    }
  }

  // Query history - delegated to QueryHistoryManager
  private addToHistory(query: string, results: QueryResult) {
    this._queryHistory.addToHistory(query, results);
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
      return { success: false, error: this.formatError(error) };
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
      return { success: false, error: this.formatError(error) };
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
      return { success: false, error: this.formatError(error) };
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
    this._queryHistory.toggleQueryFavorite(id);
  }

  // UI state methods - delegated to UIStateManager
  toggleAI() {
    this._uiState.toggleAI();
  }

  sendAIMessage(content: string) {
    this._uiState.sendAIMessage(content);
  }

  setActiveView(view: "query" | "schema" | "explain" | "erd") {
    this._uiState.setActiveView(view);
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

    this.addToTabOrder(explainTabId);

    // Set as active and switch view
    const newActiveExplainIds = new Map(this.activeExplainTabIdByConnection);
    newActiveExplainIds.set(this.activeConnectionId, explainTabId);
    this.activeExplainTabIdByConnection = newActiveExplainIds;
    this.setActiveView("explain");
    this.schedulePersistence(this.activeConnectionId);

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

  // Explain tab methods - delegated to ExplainTabManager
  removeExplainTab(id: string) {
    this._explainTabs.removeExplainTab(id);
  }

  setActiveExplainTab(id: string) {
    this._explainTabs.setActiveExplainTab(id);
  }

  // ERD tab methods
  addErdTab(): string | null {
    if (!this.activeConnectionId || !this.activeConnection) return null;

    const tabs = this.erdTabsByConnection.get(this.activeConnectionId) || [];

    // Check if an ERD tab already exists for this connection
    const existingTab = tabs.find(t => t.name === `ERD: ${this.activeConnection!.name}`);
    if (existingTab) {
      // Just switch to the existing tab
      setMapValue(
        () => this.activeErdTabIdByConnection,
        m => this.activeErdTabIdByConnection = m,
        this.activeConnectionId,
        existingTab.id
      );
      this.setActiveView("erd");
      return existingTab.id;
    }

    const erdTabId = `erd-${Date.now()}`;
    const newErdTab: ErdTab = {
      id: erdTabId,
      name: `ERD: ${this.activeConnection.name}`,
    };

    const newErdTabs = new Map(this.erdTabsByConnection);
    newErdTabs.set(this.activeConnectionId, [...tabs, newErdTab]);
    this.erdTabsByConnection = newErdTabs;

    this.addToTabOrder(erdTabId);

    setMapValue(
      () => this.activeErdTabIdByConnection,
      m => this.activeErdTabIdByConnection = m,
      this.activeConnectionId,
      erdTabId
    );

    this.setActiveView("erd");
    this.schedulePersistence(this.activeConnectionId);

    return erdTabId;
  }

  removeErdTab(id: string) {
    this.removeTabGeneric(
      () => this.erdTabsByConnection,
      m => this.erdTabsByConnection = m,
      () => this.activeErdTabIdByConnection,
      m => this.activeErdTabIdByConnection = m,
      id
    );
    this.schedulePersistence(this.activeConnectionId);

    // If no more ERD tabs, switch back to query view
    const remainingTabs = this.erdTabsByConnection.get(this.activeConnectionId!) || [];
    if (remainingTabs.length === 0) {
      this.setActiveView("query");
    }
  }

  setActiveErdTab(id: string) {
    if (!this.activeConnectionId) return;
    setMapValue(
      () => this.activeErdTabIdByConnection,
      m => this.activeErdTabIdByConnection = m,
      this.activeConnectionId,
      id
    );
    this.schedulePersistence(this.activeConnectionId);
  }
}

export const setDatabase = () => setContext("database", new UseDatabase());
export const useDatabase = () =>
  getContext<ReturnType<typeof setDatabase>>("database");
