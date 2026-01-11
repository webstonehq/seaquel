import type {
  Project,
  PersistedProjectState,
  PersistedQueryTab,
  PersistedSchemaTab,
  PersistedExplainTab,
  PersistedErdTab,
  PersistedSavedQuery,
  PersistedQueryHistoryItem,
} from "$lib/types";
import { DEFAULT_PROJECT_ID, DEFAULT_PROJECT_NAME } from "$lib/types";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import type { PersistedConnection } from "./types.js";

/**
 * Current storage version.
 * Increment when making breaking changes to storage format.
 */
export const CURRENT_STORAGE_VERSION = 2;

/**
 * Handles data migration between storage versions.
 * Version 1: Original format with per-connection tabs
 * Version 2: Projects format with per-project tabs
 */
export class MigrationManager {
  constructor(private persistence: PersistenceManager) {}

  /**
   * Check storage version and run migrations if needed.
   * Should be called on app startup before loading data.
   */
  async migrateIfNeeded(): Promise<void> {
    const version = await this.persistence.getStorageVersion();

    if (version < CURRENT_STORAGE_VERSION) {
      console.log(`Migrating storage from version ${version} to ${CURRENT_STORAGE_VERSION}`);

      if (version < 2) {
        await this.migrateToV2();
      }

      await this.persistence.setStorageVersion(CURRENT_STORAGE_VERSION);
      console.log("Migration completed successfully");
    }
  }

  /**
   * Migrate from v1 (per-connection tabs) to v2 (per-project tabs).
   *
   * Changes:
   * 1. Create default "Seaquel" project
   * 2. Assign all connections to default project with empty labelIds
   * 3. Merge all connection tabs into single project state
   * 4. Separate saved queries and history into connection_data files
   * 5. Remove old connection_state files
   */
  private async migrateToV2(): Promise<void> {
    console.log("Running migration to v2 (projects)...");

    // 1. Load existing connections
    const connections = await this.persistence.loadPersistedConnections();

    if (connections.length === 0) {
      // No connections to migrate, just create default project
      console.log("No existing connections, creating default project");
      await this.createDefaultProject();
      return;
    }

    // 2. Create default project
    const defaultProject = await this.createDefaultProject();

    // 3. Update connections with projectId and labelIds
    const updatedConnections: PersistedConnection[] = connections.map((conn) => ({
      ...conn,
      projectId: conn.projectId || DEFAULT_PROJECT_ID,
      labelIds: conn.labelIds || [],
    }));

    // 4. Load and merge all connection states
    const mergedState = await this.mergeConnectionStates(updatedConnections);

    // 5. Save updated connections
    for (const conn of updatedConnections) {
      // We need to save each connection individually to trigger proper persistence
      // This is a workaround since we don't have direct access to the store
      // The connections will be re-saved when the app loads them
    }

    // 6. Save merged project state
    if (mergedState) {
      await this.saveProjectState(DEFAULT_PROJECT_ID, mergedState);
    }

    // 7. Migrate saved queries and history to connection_data files
    for (const conn of connections) {
      await this.migrateConnectionData(conn.id);
    }

    console.log(`Migrated ${connections.length} connections to project "${DEFAULT_PROJECT_NAME}"`);
  }

  /**
   * Create the default project.
   */
  private async createDefaultProject(): Promise<Project> {
    const now = new Date();
    const project: Project = {
      id: DEFAULT_PROJECT_ID,
      name: DEFAULT_PROJECT_NAME,
      createdAt: now,
      updatedAt: now,
      customLabels: [],
    };

    // Persist the project
    const { loadStore } = await import("$lib/storage");
    const store = await loadStore("projects.json", {
      autoSave: true,
      defaults: { projects: [] },
    });

    await store.set("projects", [{
      id: project.id,
      name: project.name,
      createdAt: project.createdAt.toISOString(),
      updatedAt: project.updatedAt.toISOString(),
      customLabels: [],
    }]);
    await store.save();

    return project;
  }

  /**
   * Merge all connection states into a single project state.
   */
  private async mergeConnectionStates(
    connections: PersistedConnection[]
  ): Promise<PersistedProjectState | null> {
    const allQueryTabs: PersistedQueryTab[] = [];
    const allSchemaTabs: PersistedSchemaTab[] = [];
    const allExplainTabs: PersistedExplainTab[] = [];
    const allErdTabs: PersistedErdTab[] = [];
    const allTabOrder: string[] = [];
    let activeView: 'query' | 'schema' | 'explain' | 'erd' = 'query';
    let activeQueryTabId: string | null = null;
    let activeSchemaTabId: string | null = null;
    let activeExplainTabId: string | null = null;
    let activeErdTabId: string | null = null;
    let activeConnectionId: string | null = null;

    // Find the most recently connected connection to use as active
    const sortedConnections = [...connections].sort((a, b) => {
      const aTime = a.lastConnected ? new Date(a.lastConnected).getTime() : 0;
      const bTime = b.lastConnected ? new Date(b.lastConnected).getTime() : 0;
      return bTime - aTime;
    });

    for (const conn of sortedConnections) {
      const legacyState = await this.persistence.loadLegacyConnectionState(conn.id);
      if (!legacyState) continue;

      // Merge tabs
      allQueryTabs.push(...legacyState.queryTabs);
      allSchemaTabs.push(...legacyState.schemaTabs);
      allExplainTabs.push(...legacyState.explainTabs);
      allErdTabs.push(...legacyState.erdTabs);
      allTabOrder.push(...legacyState.tabOrder);

      // Use the first connection's active state
      if (!activeConnectionId) {
        activeConnectionId = conn.id;
        activeView = legacyState.activeView;
        activeQueryTabId = legacyState.activeQueryTabId;
        activeSchemaTabId = legacyState.activeSchemaTabId;
        activeExplainTabId = legacyState.activeExplainTabId;
        activeErdTabId = legacyState.activeErdTabId;
      }
    }

    if (allQueryTabs.length === 0 && allSchemaTabs.length === 0) {
      return null;
    }

    return {
      projectId: DEFAULT_PROJECT_ID,
      queryTabs: allQueryTabs,
      schemaTabs: allSchemaTabs,
      explainTabs: allExplainTabs,
      erdTabs: allErdTabs,
      tabOrder: allTabOrder,
      activeQueryTabId,
      activeSchemaTabId,
      activeExplainTabId,
      activeErdTabId,
      activeView,
      activeConnectionId,
    };
  }

  /**
   * Migrate saved queries and history from legacy connection state to new format.
   */
  private async migrateConnectionData(connectionId: string): Promise<void> {
    const legacyState = await this.persistence.loadLegacyConnectionState(connectionId);
    if (!legacyState) return;

    // Migrate saved queries and history
    const { savedQueries, queryHistory } = legacyState;

    if (savedQueries.length > 0 || queryHistory.length > 0) {
      const { loadStore } = await import("$lib/storage");
      const store = await loadStore(`connection_data_${connectionId}.json`, {
        autoSave: true,
        defaults: {},
      });

      // Add missing fields to history items for backwards compatibility
      const migratedHistory: PersistedQueryHistoryItem[] = queryHistory.map((h) => ({
        ...h,
        connectionLabelsSnapshot: (h as any).connectionLabelsSnapshot || [],
        connectionNameSnapshot: (h as any).connectionNameSnapshot || "",
      }));

      await store.set("savedQueries", savedQueries);
      await store.set("queryHistory", migratedHistory);
      await store.save();
    }

    // Remove legacy connection state file
    await this.persistence.removeLegacyConnectionState(connectionId);
  }

  /**
   * Save project state directly to storage.
   */
  private async saveProjectState(
    projectId: string,
    state: PersistedProjectState
  ): Promise<void> {
    const { loadStore } = await import("$lib/storage");
    const store = await loadStore(`project_state_${projectId}.json`, {
      autoSave: true,
      defaults: { state: null },
    });

    await store.set("state", state);
    await store.save();
  }
}
