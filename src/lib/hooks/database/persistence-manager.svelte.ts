import { toast } from "svelte-sonner";
import type {
  PersistedQueryTab,
  PersistedSchemaTab,
  PersistedExplainTab,
  PersistedErdTab,
  PersistedStarterTab,
  PersistedSavedQuery,
  PersistedQueryHistoryItem,
  DatabaseConnection,
  PersistedProject,
  PersistedProjectState,
  ConnectionLabel,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistedConnection } from "./types.js";
import { loadStore } from "$lib/storage";
import { getKeyringService } from "$lib/services/keyring";

/**
 * Manages persistence of projects, connections, and their state to Tauri store.
 * Handles serialization, debounced saving, and state loading.
 *
 * Storage structure:
 * - projects.json: All projects
 * - app_state.json: Global app state (last active project, etc.)
 * - database_connections.json: All connections (with projectId)
 * - project_state_{projectId}.json: Tabs and UI state per project
 * - connection_data_{connectionId}.json: Query history and saved queries per connection
 */
export class PersistenceManager {
  private persistenceTimer: ReturnType<typeof setTimeout> | null = null;
  readonly PERSISTENCE_DEBOUNCE_MS = 500;
  readonly MAX_HISTORY_ITEMS = 500;

  constructor(private state: DatabaseState) {}

  /**
   * Schedule persistence with debouncing to avoid excessive I/O.
   * Now persists project state instead of connection state.
   */
  scheduleProject(projectId: string | null): void {
    if (!projectId) return;

    if (this.persistenceTimer) {
      clearTimeout(this.persistenceTimer);
    }
    this.persistenceTimer = setTimeout(() => {
      this.persistProjectState(projectId);
      this.persistenceTimer = null;
    }, this.PERSISTENCE_DEBOUNCE_MS);
  }

  /**
   * Schedule connection data persistence (history, saved queries).
   */
  scheduleConnectionData(connectionId: string | null): void {
    if (!connectionId) return;

    if (this.persistenceTimer) {
      clearTimeout(this.persistenceTimer);
    }
    this.persistenceTimer = setTimeout(() => {
      this.persistConnectionData(connectionId);
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
    // Persist all projects that have data
    for (const projectId of Object.keys(this.state.queryTabsByProject)) {
      this.persistProjectState(projectId);
    }
    // Persist all connection data
    for (const connectionId of Object.keys(this.state.queryHistoryByConnection)) {
      this.persistConnectionData(connectionId);
    }
  }

  /**
   * Clean up resources. Should be called when component unmounts.
   */
  cleanup(): void {
    this.flush();
  }

  // === SERIALIZATION METHODS ===

  serializeQueryTabs(projectId: string): PersistedQueryTab[] {
    const tabs = this.state.queryTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      query: tab.query,
      savedQueryId: tab.savedQueryId,
    }));
  }

  serializeSchemaTabs(projectId: string): PersistedSchemaTab[] {
    const tabs = this.state.schemaTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      tableName: tab.table.name,
      schemaName: tab.table.schema,
    }));
  }

  serializeExplainTabs(projectId: string): PersistedExplainTab[] {
    const tabs = this.state.explainTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      sourceQuery: tab.sourceQuery,
    }));
  }

  serializeErdTabs(projectId: string): PersistedErdTab[] {
    const tabs = this.state.erdTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      name: tab.name,
      connectionId: tab.connectionId,
    }));
  }

  serializeStarterTabs(projectId: string): PersistedStarterTab[] {
    const tabs = this.state.starterTabsByProject[projectId] ?? [];
    return tabs.map((tab) => ({
      id: tab.id,
      type: tab.type,
      name: tab.name,
      closable: tab.closable,
    }));
  }

  serializeSavedQueries(connectionId: string): PersistedSavedQuery[] {
    const queries = this.state.savedQueriesByConnection[connectionId] ?? [];
    return queries.map((q) => ({
      id: q.id,
      name: q.name,
      query: q.query,
      connectionId: q.connectionId,
      createdAt: q.createdAt.toISOString(),
      updatedAt: q.updatedAt.toISOString(),
      parameters: q.parameters,
    }));
  }

  serializeQueryHistory(connectionId: string): PersistedQueryHistoryItem[] {
    const history = this.state.queryHistoryByConnection[connectionId] ?? [];
    return history.slice(0, this.MAX_HISTORY_ITEMS).map((h) => ({
      id: h.id,
      query: h.query,
      timestamp: h.timestamp.toISOString(),
      executionTime: h.executionTime,
      rowCount: h.rowCount,
      connectionId: h.connectionId,
      favorite: h.favorite,
      connectionLabelsSnapshot: h.connectionLabelsSnapshot,
      connectionNameSnapshot: h.connectionNameSnapshot,
    }));
  }

  // === PROJECT PERSISTENCE ===

  async persistProjects(): Promise<void> {
    try {
      const store = await loadStore("projects.json", {
        autoSave: true,
        defaults: { projects: [] },
      });

      const projects: PersistedProject[] = this.state.projects.map((p) => ({
        id: p.id,
        name: p.name,
        description: p.description,
        createdAt: p.createdAt.toISOString(),
        updatedAt: p.updatedAt.toISOString(),
        customLabels: p.customLabels,
      }));

      await store.set("projects", projects);
      await store.save();
    } catch (error) {
      console.error("Failed to persist projects:", error);
    }
  }

  async loadProjects(): Promise<PersistedProject[]> {
    try {
      const store = await loadStore("projects.json", {
        autoSave: false,
        defaults: { projects: [] },
      });
      return (await store.get("projects")) as PersistedProject[] || [];
    } catch (error) {
      console.error("Failed to load projects:", error);
      return [];
    }
  }

  // === APP STATE PERSISTENCE ===

  async persistAppState(): Promise<void> {
    try {
      const store = await loadStore("app_state.json", {
        autoSave: true,
        defaults: {},
      });

      await store.set("lastActiveProjectId", this.state.activeProjectId);
      await store.save();
    } catch (error) {
      console.error("Failed to persist app state:", error);
    }
  }

  async getLastActiveProjectId(): Promise<string | null> {
    try {
      const store = await loadStore("app_state.json", {
        autoSave: false,
        defaults: {},
      });
      return (await store.get("lastActiveProjectId")) as string | null;
    } catch (error) {
      console.error("Failed to load last active project:", error);
      return null;
    }
  }

  // === PROJECT STATE PERSISTENCE (tabs, active IDs) ===

  async persistProjectState(projectId: string): Promise<void> {
    try {
      const store = await loadStore(`project_state_${projectId}.json`, {
        autoSave: true,
        defaults: { state: null },
      });

      const state: PersistedProjectState = {
        projectId,
        queryTabs: this.serializeQueryTabs(projectId),
        schemaTabs: this.serializeSchemaTabs(projectId),
        explainTabs: this.serializeExplainTabs(projectId),
        erdTabs: this.serializeErdTabs(projectId),
        tabOrder: this.state.tabOrderByProject[projectId] ?? [],
        activeQueryTabId: this.state.activeQueryTabIdByProject[projectId] ?? null,
        activeSchemaTabId: this.state.activeSchemaTabIdByProject[projectId] ?? null,
        activeExplainTabId: this.state.activeExplainTabIdByProject[projectId] ?? null,
        activeErdTabId: this.state.activeErdTabIdByProject[projectId] ?? null,
        activeView: this.state.activeView,
        activeConnectionId: this.state.activeConnectionIdByProject[projectId] ?? null,
        starterTabs: this.serializeStarterTabs(projectId),
        activeStarterTabId: this.state.activeStarterTabIdByProject[projectId] ?? null,
      };

      await store.set("state", state);
      await store.save();
    } catch (error) {
      console.error(`Failed to persist state for project ${projectId}:`, error);
    }
  }

  async loadProjectState(projectId: string): Promise<PersistedProjectState | null> {
    try {
      const store = await loadStore(`project_state_${projectId}.json`, {
        autoSave: false,
        defaults: { state: null },
      });
      return (await store.get("state")) as PersistedProjectState | null;
    } catch (error) {
      console.error(`Failed to load persisted state for project ${projectId}:`, error);
      return null;
    }
  }

  async removeProjectState(projectId: string): Promise<void> {
    try {
      const store = await loadStore(`project_state_${projectId}.json`, {
        autoSave: false,
        defaults: { state: null },
      });
      await store.delete();
    } catch (error) {
      console.error(`Failed to remove persisted state for project ${projectId}:`, error);
    }
  }

  // === CONNECTION DATA PERSISTENCE (history, saved queries) ===

  async persistConnectionData(connectionId: string): Promise<void> {
    try {
      const store = await loadStore(`connection_data_${connectionId}.json`, {
        autoSave: true,
        defaults: {},
      });

      await store.set("savedQueries", this.serializeSavedQueries(connectionId));
      await store.set("queryHistory", this.serializeQueryHistory(connectionId));
      await store.save();
    } catch (error) {
      console.error(`Failed to persist data for connection ${connectionId}:`, error);
    }
  }

  async loadConnectionData(connectionId: string): Promise<{
    savedQueries: PersistedSavedQuery[];
    queryHistory: PersistedQueryHistoryItem[];
  }> {
    try {
      const store = await loadStore(`connection_data_${connectionId}.json`, {
        autoSave: false,
        defaults: {},
      });
      return {
        savedQueries: (await store.get("savedQueries")) as PersistedSavedQuery[] || [],
        queryHistory: (await store.get("queryHistory")) as PersistedQueryHistoryItem[] || [],
      };
    } catch (error) {
      console.error(`Failed to load data for connection ${connectionId}:`, error);
      return { savedQueries: [], queryHistory: [] };
    }
  }

  async removeConnectionData(connectionId: string): Promise<void> {
    try {
      const store = await loadStore(`connection_data_${connectionId}.json`, {
        autoSave: false,
        defaults: {},
      });
      await store.delete();
    } catch (error) {
      console.error(`Failed to remove data for connection ${connectionId}:`, error);
    }
  }

  // === LEGACY CONNECTION STATE (for migration) ===

  async loadLegacyConnectionState(connectionId: string): Promise<{
    queryTabs: PersistedQueryTab[];
    schemaTabs: PersistedSchemaTab[];
    explainTabs: PersistedExplainTab[];
    erdTabs: PersistedErdTab[];
    tabOrder: string[];
    activeQueryTabId: string | null;
    activeSchemaTabId: string | null;
    activeExplainTabId: string | null;
    activeErdTabId: string | null;
    activeView: 'query' | 'schema' | 'explain' | 'erd';
    savedQueries: PersistedSavedQuery[];
    queryHistory: PersistedQueryHistoryItem[];
  } | null> {
    try {
      const store = await loadStore(`connection_state_${connectionId}.json`, {
        autoSave: false,
        defaults: { state: null },
      });
      const state = await store.get("state");
      if (!state) return null;
      return state as {
        queryTabs: PersistedQueryTab[];
        schemaTabs: PersistedSchemaTab[];
        explainTabs: PersistedExplainTab[];
        erdTabs: PersistedErdTab[];
        tabOrder: string[];
        activeQueryTabId: string | null;
        activeSchemaTabId: string | null;
        activeExplainTabId: string | null;
        activeErdTabId: string | null;
        activeView: 'query' | 'schema' | 'explain' | 'erd';
        savedQueries: PersistedSavedQuery[];
        queryHistory: PersistedQueryHistoryItem[];
      };
    } catch {
      return null;
    }
  }

  async removeLegacyConnectionState(connectionId: string): Promise<void> {
    try {
      const store = await loadStore(`connection_state_${connectionId}.json`, {
        autoSave: false,
        defaults: { state: null },
      });
      await store.delete();
    } catch {
      // Ignore errors when removing legacy state
    }
  }

  // === CONNECTION PERSISTENCE ===

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

  async persistConnection(
    connection: DatabaseConnection,
    options?: {
      savePassword?: boolean;
      saveSshPassword?: boolean;
      saveSshKeyPassphrase?: boolean;
      sshPassword?: string;
      sshKeyPassphrase?: string;
    }
  ): Promise<void> {
    try {
      const store = await loadStore("database_connections.json", {
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
        savePassword: options?.savePassword,
        saveSshPassword: options?.saveSshPassword,
        saveSshKeyPassphrase: options?.saveSshKeyPassphrase,
        projectId: connection.projectId,
        labelIds: connection.labelIds,
      };

      filtered.push(persistedConnection);
      await store.set("connections", filtered);
      await store.save();

      // Save passwords to keyring if enabled
      const keyring = getKeyringService();
      if (keyring.isAvailable()) {
        try {
          if (options?.savePassword && connection.password) {
            await keyring.setDbPassword(connection.id, connection.password);
          } else if (!options?.savePassword) {
            // Delete existing password if save is disabled
            await keyring.deleteDbPassword(connection.id);
          }

          if (options?.saveSshPassword && options.sshPassword) {
            await keyring.setSshPassword(connection.id, options.sshPassword);
          } else if (!options?.saveSshPassword) {
            await keyring.deleteSshPassword(connection.id);
          }

          if (options?.saveSshKeyPassphrase && options.sshKeyPassphrase) {
            await keyring.setSshKeyPassphrase(connection.id, options.sshKeyPassphrase);
          } else if (!options?.saveSshKeyPassphrase) {
            await keyring.deleteSshKeyPassphrase(connection.id);
          }
        } catch (error) {
          console.warn("Failed to save credentials to keyring:", error);
          toast.error("Could not save password to system keychain");
        }
      }
    } catch (error) {
      console.error("Failed to persist connection:", error);
      toast.error("Failed to save connection to storage");
    }
  }

  async removePersistedConnection(connectionId: string): Promise<void> {
    try {
      const store = await loadStore("database_connections.json", {
        autoSave: true,
        defaults: { connections: [] },
      });

      const existingConnections = (await store.get("connections")) as PersistedConnection[] | null;
      const connections = existingConnections || [];

      const filtered = connections.filter((c) => c.id !== connectionId);
      await store.set("connections", filtered);
      await store.save();

      // Delete passwords from keyring
      const keyring = getKeyringService();
      if (keyring.isAvailable()) {
        try {
          await keyring.deleteAllForConnection(connectionId);
        } catch (error) {
          console.warn("Failed to delete credentials from keyring:", error);
        }
      }

      // Remove connection data
      await this.removeConnectionData(connectionId);
    } catch (error) {
      console.error("Failed to delete persisted connection:", error);
      toast.error("Failed to delete connection from storage");
    }
  }

  async loadPersistedConnections(): Promise<PersistedConnection[]> {
    try {
      const store = await loadStore("database_connections.json", {
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

  // === STORAGE VERSION ===

  async getStorageVersion(): Promise<number> {
    try {
      const store = await loadStore("app_state.json", {
        autoSave: false,
        defaults: {},
      });
      return (await store.get("storageVersion")) as number || 1;
    } catch {
      return 1;
    }
  }

  async setStorageVersion(version: number): Promise<void> {
    try {
      const store = await loadStore("app_state.json", {
        autoSave: true,
        defaults: {},
      });
      await store.set("storageVersion", version);
      await store.save();
    } catch (error) {
      console.error("Failed to set storage version:", error);
    }
  }
}
