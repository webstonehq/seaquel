import { load } from "@tauri-apps/plugin-store";
import { toast } from "svelte-sonner";
import type {
  PersistedConnectionState,
  PersistedQueryTab,
  PersistedSchemaTab,
  PersistedExplainTab,
  PersistedErdTab,
  PersistedSavedQuery,
  PersistedQueryHistoryItem,
  DatabaseConnection,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistedConnection } from "./types.js";

/**
 * Get filename with optional test worker suffix for parallel test isolation.
 * In test mode, each worker uses unique filenames to avoid conflicts.
 */
function getStoreFilename(basename: string): string {
  if (typeof window !== "undefined" && (window as unknown as Record<string, unknown>).__SEAQUEL_TEST_WORKER_ID__) {
    const workerId = (window as unknown as Record<string, unknown>).__SEAQUEL_TEST_WORKER_ID__ as string;
    const lastDot = basename.lastIndexOf(".");
    if (lastDot > 0) {
      const name = basename.substring(0, lastDot);
      const ext = basename.substring(lastDot);
      return `${name}_${workerId}${ext}`;
    }
    return `${basename}_${workerId}`;
  }
  return basename;
}

/**
 * Manages persistence of connection state to Tauri store.
 * Handles serialization, debounced saving, and state loading.
 */
export class PersistenceManager {
  private persistenceTimer: ReturnType<typeof setTimeout> | null = null;
  readonly PERSISTENCE_DEBOUNCE_MS = 500;
  readonly MAX_HISTORY_ITEMS = 500;

  constructor(private state: DatabaseState) {}

  /**
   * Schedule persistence with debouncing to avoid excessive I/O.
   */
  schedule(connectionId: string | null): void {
    if (!connectionId) return;

    if (this.persistenceTimer) {
      clearTimeout(this.persistenceTimer);
    }
    this.persistenceTimer = setTimeout(() => {
      this.persistConnectionState(connectionId);
      this.persistenceTimer = null;
    }, this.PERSISTENCE_DEBOUNCE_MS);
  }

  /**
   * Immediately flush any pending persistence operations.
   */
  flush(): void {
    if (this.persistenceTimer) {
      clearTimeout(this.persistenceTimer);
      this.persistenceTimer = null;
    }
    // Persist all connections that have data
    for (const connectionId of this.state.queryTabsByConnection.keys()) {
      this.persistConnectionState(connectionId);
    }
  }

  /**
   * Clean up resources. Should be called when component unmounts.
   */
  cleanup(): void {
    this.flush();
  }

  // Serialization methods

  serializeQueryTabs(connectionId: string): PersistedQueryTab[] {
    const tabs = this.state.queryTabsByConnection.get(connectionId) || [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      query: tab.query,
      savedQueryId: tab.savedQueryId,
    }));
  }

  serializeSchemaTabs(connectionId: string): PersistedSchemaTab[] {
    const tabs = this.state.schemaTabsByConnection.get(connectionId) || [];
    return tabs.map((tab) => ({
      id: tab.id,
      tableName: tab.table.name,
      schemaName: tab.table.schema,
    }));
  }

  serializeExplainTabs(connectionId: string): PersistedExplainTab[] {
    const tabs = this.state.explainTabsByConnection.get(connectionId) || [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      sourceQuery: tab.sourceQuery,
    }));
  }

  serializeErdTabs(connectionId: string): PersistedErdTab[] {
    const tabs = this.state.erdTabsByConnection.get(connectionId) || [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
    }));
  }

  serializeSavedQueries(connectionId: string): PersistedSavedQuery[] {
    const queries = this.state.savedQueriesByConnection.get(connectionId) || [];
    return queries.map((q) => ({
      id: q.id,
      name: q.name,
      query: q.query,
      connectionId: q.connectionId,
      createdAt: q.createdAt.toISOString(),
      updatedAt: q.updatedAt.toISOString(),
    }));
  }

  serializeQueryHistory(connectionId: string): PersistedQueryHistoryItem[] {
    const history = this.state.queryHistoryByConnection.get(connectionId) || [];
    return history.slice(0, this.MAX_HISTORY_ITEMS).map((h) => ({
      id: h.id,
      query: h.query,
      timestamp: h.timestamp.toISOString(),
      executionTime: h.executionTime,
      rowCount: h.rowCount,
      connectionId: h.connectionId,
      favorite: h.favorite,
    }));
  }

  // Persistence methods

  async persistConnectionState(connectionId: string): Promise<void> {
    try {
      const store = await load(getStoreFilename(`connection_state_${connectionId}.json`), {
        autoSave: true,
        defaults: { state: null },
      });

      const state: PersistedConnectionState = {
        connectionId,
        queryTabs: this.serializeQueryTabs(connectionId),
        schemaTabs: this.serializeSchemaTabs(connectionId),
        explainTabs: this.serializeExplainTabs(connectionId),
        erdTabs: this.serializeErdTabs(connectionId),
        tabOrder: this.state.tabOrderByConnection.get(connectionId) || [],
        activeQueryTabId: this.state.activeQueryTabIdByConnection.get(connectionId) || null,
        activeSchemaTabId: this.state.activeSchemaTabIdByConnection.get(connectionId) || null,
        activeExplainTabId: this.state.activeExplainTabIdByConnection.get(connectionId) || null,
        activeErdTabId: this.state.activeErdTabIdByConnection.get(connectionId) || null,
        activeView: this.state.activeView,
        savedQueries: this.serializeSavedQueries(connectionId),
        queryHistory: this.serializeQueryHistory(connectionId),
      };

      await store.set("state", state);
      await store.save();
    } catch (error) {
      console.error(`Failed to persist state for connection ${connectionId}:`, error);
    }
  }

  async loadPersistedConnectionState(connectionId: string): Promise<PersistedConnectionState | null> {
    try {
      const store = await load(getStoreFilename(`connection_state_${connectionId}.json`), {
        autoSave: false,
        defaults: { state: null },
      });
      return (await store.get("state")) as PersistedConnectionState | null;
    } catch (error) {
      console.error(`Failed to load persisted state for ${connectionId}:`, error);
      return null;
    }
  }

  async removePersistedConnectionState(connectionId: string): Promise<void> {
    try {
      const store = await load(getStoreFilename(`connection_state_${connectionId}.json`), {
        autoSave: true,
        defaults: { state: null },
      });
      await store.clear();
      await store.save();
    } catch (error) {
      console.error(`Failed to remove persisted state for ${connectionId}:`, error);
    }
  }

  // Connection persistence (separate from connection state)

  stripPasswordFromConnectionString(connectionString?: string): string | undefined {
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
      let normalized = connectionString.replace("postgresql://", "postgres://");
      const url = new URL(normalized);

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

  async persistConnection(connection: DatabaseConnection): Promise<void> {
    try {
      const store = await load(getStoreFilename("database_connections.json"), {
        autoSave: true,
        defaults: { connections: [] },
      });

      const existingConnections = (await store.get("connections")) as PersistedConnection[] | null;
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
        connectionString: this.stripPasswordFromConnectionString(connection.connectionString),
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

  async removePersistedConnection(connectionId: string): Promise<void> {
    try {
      const store = await load(getStoreFilename("database_connections.json"), {
        autoSave: true,
        defaults: { connections: [] },
      });

      const existingConnections = (await store.get("connections")) as PersistedConnection[] | null;
      const connections = existingConnections || [];

      const filtered = connections.filter((c) => c.id !== connectionId);
      await store.set("connections", filtered);
      await store.save();
    } catch (error) {
      console.error("Failed to remove persisted connection:", error);
      toast.error("Failed to remove connection from storage");
    }
  }

  async loadPersistedConnections(): Promise<PersistedConnection[]> {
    try {
      const store = await load(getStoreFilename("database_connections.json"), {
        autoSave: true,
        defaults: { connections: [] },
      });
      const persistedConnections = (await store.get("connections")) as PersistedConnection[] | null;
      return persistedConnections || [];
    } catch (error) {
      console.error("Failed to load persisted connections:", error);
      return [];
    }
  }
}
