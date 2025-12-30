import type {
  QueryTab,
  SchemaTab,
  ExplainTab,
  SavedQuery,
  QueryHistoryItem,
  PersistedSchemaTab,
  PersistedSavedQuery,
  PersistedQueryHistoryItem,
  PersistedConnectionState,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import { setMapValue, deleteMapKey } from "./map-utils.js";

/**
 * Manages restoration of persisted state when reconnecting to a database.
 * Handles hydration of tabs, saved queries, and query history.
 */
export class StateRestorationManager {
  constructor(
    private state: DatabaseState,
    private persistence: PersistenceManager
  ) {}

  /**
   * Initialize all per-connection maps for a new connection.
   */
  initializeConnectionMaps(connectionId: string): void {
    setMapValue(() => this.state.queryTabsByConnection, (m) => (this.state.queryTabsByConnection = m), connectionId, []);
    setMapValue(() => this.state.schemaTabsByConnection, (m) => (this.state.schemaTabsByConnection = m), connectionId, []);
    setMapValue(() => this.state.activeQueryTabIdByConnection, (m) => (this.state.activeQueryTabIdByConnection = m), connectionId, null);
    setMapValue(() => this.state.activeSchemaTabIdByConnection, (m) => (this.state.activeSchemaTabIdByConnection = m), connectionId, null);
    setMapValue(() => this.state.queryHistoryByConnection, (m) => (this.state.queryHistoryByConnection = m), connectionId, []);
    setMapValue(() => this.state.savedQueriesByConnection, (m) => (this.state.savedQueriesByConnection = m), connectionId, []);
    setMapValue(() => this.state.explainTabsByConnection, (m) => (this.state.explainTabsByConnection = m), connectionId, []);
    setMapValue(() => this.state.activeExplainTabIdByConnection, (m) => (this.state.activeExplainTabIdByConnection = m), connectionId, null);
    setMapValue(() => this.state.erdTabsByConnection, (m) => (this.state.erdTabsByConnection = m), connectionId, []);
    setMapValue(() => this.state.activeErdTabIdByConnection, (m) => (this.state.activeErdTabIdByConnection = m), connectionId, null);
    setMapValue(() => this.state.tabOrderByConnection, (m) => (this.state.tabOrderByConnection = m), connectionId, []);
    setMapValue(() => this.state.schemas, (m) => (this.state.schemas = m), connectionId, []);
  }

  /**
   * Clean up all per-connection maps when removing a connection.
   */
  cleanupConnectionMaps(connectionId: string): void {
    deleteMapKey(() => this.state.queryTabsByConnection, (m) => (this.state.queryTabsByConnection = m), connectionId);
    deleteMapKey(() => this.state.schemaTabsByConnection, (m) => (this.state.schemaTabsByConnection = m), connectionId);
    deleteMapKey(() => this.state.activeQueryTabIdByConnection, (m) => (this.state.activeQueryTabIdByConnection = m), connectionId);
    deleteMapKey(() => this.state.activeSchemaTabIdByConnection, (m) => (this.state.activeSchemaTabIdByConnection = m), connectionId);
    deleteMapKey(() => this.state.queryHistoryByConnection, (m) => (this.state.queryHistoryByConnection = m), connectionId);
    deleteMapKey(() => this.state.savedQueriesByConnection, (m) => (this.state.savedQueriesByConnection = m), connectionId);
    deleteMapKey(() => this.state.explainTabsByConnection, (m) => (this.state.explainTabsByConnection = m), connectionId);
    deleteMapKey(() => this.state.activeExplainTabIdByConnection, (m) => (this.state.activeExplainTabIdByConnection = m), connectionId);
    deleteMapKey(() => this.state.erdTabsByConnection, (m) => (this.state.erdTabsByConnection = m), connectionId);
    deleteMapKey(() => this.state.activeErdTabIdByConnection, (m) => (this.state.activeErdTabIdByConnection = m), connectionId);
    deleteMapKey(() => this.state.tabOrderByConnection, (m) => (this.state.tabOrderByConnection = m), connectionId);
    deleteMapKey(() => this.state.schemas, (m) => (this.state.schemas = m), connectionId);
  }

  /**
   * Ensure maps exist for a connection (used during reconnect to preserve existing state).
   */
  ensureConnectionMapsExist(connectionId: string): void {
    if (!this.state.queryTabsByConnection.has(connectionId)) {
      setMapValue(() => this.state.queryTabsByConnection, (m) => (this.state.queryTabsByConnection = m), connectionId, []);
    }
    if (!this.state.schemaTabsByConnection.has(connectionId)) {
      setMapValue(() => this.state.schemaTabsByConnection, (m) => (this.state.schemaTabsByConnection = m), connectionId, []);
    }
    if (!this.state.activeQueryTabIdByConnection.has(connectionId)) {
      setMapValue(() => this.state.activeQueryTabIdByConnection, (m) => (this.state.activeQueryTabIdByConnection = m), connectionId, null);
    }
    if (!this.state.activeSchemaTabIdByConnection.has(connectionId)) {
      setMapValue(() => this.state.activeSchemaTabIdByConnection, (m) => (this.state.activeSchemaTabIdByConnection = m), connectionId, null);
    }
    if (!this.state.queryHistoryByConnection.has(connectionId)) {
      setMapValue(() => this.state.queryHistoryByConnection, (m) => (this.state.queryHistoryByConnection = m), connectionId, []);
    }
    if (!this.state.savedQueriesByConnection.has(connectionId)) {
      setMapValue(() => this.state.savedQueriesByConnection, (m) => (this.state.savedQueriesByConnection = m), connectionId, []);
    }
    if (!this.state.explainTabsByConnection.has(connectionId)) {
      setMapValue(() => this.state.explainTabsByConnection, (m) => (this.state.explainTabsByConnection = m), connectionId, []);
    }
    if (!this.state.activeExplainTabIdByConnection.has(connectionId)) {
      setMapValue(() => this.state.activeExplainTabIdByConnection, (m) => (this.state.activeExplainTabIdByConnection = m), connectionId, null);
    }
    if (!this.state.erdTabsByConnection.has(connectionId)) {
      setMapValue(() => this.state.erdTabsByConnection, (m) => (this.state.erdTabsByConnection = m), connectionId, []);
    }
    if (!this.state.activeErdTabIdByConnection.has(connectionId)) {
      setMapValue(() => this.state.activeErdTabIdByConnection, (m) => (this.state.activeErdTabIdByConnection = m), connectionId, null);
    }
    if (!this.state.tabOrderByConnection.has(connectionId)) {
      setMapValue(() => this.state.tabOrderByConnection, (m) => (this.state.tabOrderByConnection = m), connectionId, []);
    }
  }

  /**
   * Restore saved queries from persisted data.
   */
  restoreSavedQueries(connectionId: string, data: PersistedSavedQuery[]): void {
    const savedQueries: SavedQuery[] = data.map((q) => ({
      id: q.id,
      name: q.name,
      query: q.query,
      connectionId: q.connectionId,
      createdAt: new Date(q.createdAt),
      updatedAt: new Date(q.updatedAt),
    }));
    setMapValue(
      () => this.state.savedQueriesByConnection,
      (m) => (this.state.savedQueriesByConnection = m),
      connectionId,
      savedQueries
    );
  }

  /**
   * Restore query history from persisted data.
   */
  restoreQueryHistory(connectionId: string, data: PersistedQueryHistoryItem[]): void {
    const history: QueryHistoryItem[] = data.map((h) => ({
      id: h.id,
      query: h.query,
      timestamp: new Date(h.timestamp),
      executionTime: h.executionTime,
      rowCount: h.rowCount,
      connectionId: h.connectionId,
      favorite: h.favorite,
    }));
    setMapValue(
      () => this.state.queryHistoryByConnection,
      (m) => (this.state.queryHistoryByConnection = m),
      connectionId,
      history
    );
  }

  /**
   * Restore schema tabs matching current schema.
   */
  private restoreSchemaTabs(connectionId: string, persistedTabs: PersistedSchemaTab[]): void {
    const schemas = this.state.schemas.get(connectionId) || [];
    const schemaTabs: SchemaTab[] = [];

    for (const pt of persistedTabs) {
      // Find matching table in current schema
      const table = schemas.find((t) => t.name === pt.tableName && t.schema === pt.schemaName);

      if (table) {
        schemaTabs.push({
          id: pt.id,
          table: table,
        });
      }
      // If table no longer exists, skip restoring this tab
    }

    setMapValue(
      () => this.state.schemaTabsByConnection,
      (m) => (this.state.schemaTabsByConnection = m),
      connectionId,
      schemaTabs
    );
  }

  /**
   * Restore connection tabs from persisted state.
   * Returns true if query tabs were restored.
   */
  async restoreConnectionTabs(connectionId: string): Promise<boolean> {
    const persistedState = await this.persistence.loadPersistedConnectionState(connectionId);
    if (!persistedState) return false;

    // Restore query tabs (without results - they'll need to be re-executed)
    const queryTabs: QueryTab[] = persistedState.queryTabs.map((pt) => ({
      id: pt.id,
      name: pt.name,
      query: pt.query,
      savedQueryId: pt.savedQueryId,
      isExecuting: false,
      results: undefined,
    }));
    setMapValue(
      () => this.state.queryTabsByConnection,
      (m) => (this.state.queryTabsByConnection = m),
      connectionId,
      queryTabs
    );

    // Restore explain tabs (without results - they'll need to be re-executed)
    const explainTabs: ExplainTab[] = persistedState.explainTabs.map((pt) => ({
      id: pt.id,
      name: pt.name,
      sourceQuery: pt.sourceQuery,
      isExecuting: false,
      result: undefined,
    }));
    setMapValue(
      () => this.state.explainTabsByConnection,
      (m) => (this.state.explainTabsByConnection = m),
      connectionId,
      explainTabs
    );

    // Restore schema tabs - need to match against current schema
    this.restoreSchemaTabs(connectionId, persistedState.schemaTabs);

    // Restore active tab IDs (only if tabs exist)
    if (persistedState.activeQueryTabId && queryTabs.some((t) => t.id === persistedState.activeQueryTabId)) {
      setMapValue(
        () => this.state.activeQueryTabIdByConnection,
        (m) => (this.state.activeQueryTabIdByConnection = m),
        connectionId,
        persistedState.activeQueryTabId
      );
    } else if (queryTabs.length > 0) {
      setMapValue(
        () => this.state.activeQueryTabIdByConnection,
        (m) => (this.state.activeQueryTabIdByConnection = m),
        connectionId,
        queryTabs[0].id
      );
    }

    const schemaTabs = this.state.schemaTabsByConnection.get(connectionId) || [];
    if (persistedState.activeSchemaTabId && schemaTabs.some((t) => t.id === persistedState.activeSchemaTabId)) {
      setMapValue(
        () => this.state.activeSchemaTabIdByConnection,
        (m) => (this.state.activeSchemaTabIdByConnection = m),
        connectionId,
        persistedState.activeSchemaTabId
      );
    } else if (schemaTabs.length > 0) {
      setMapValue(
        () => this.state.activeSchemaTabIdByConnection,
        (m) => (this.state.activeSchemaTabIdByConnection = m),
        connectionId,
        schemaTabs[0].id
      );
    }

    if (persistedState.activeExplainTabId && explainTabs.some((t) => t.id === persistedState.activeExplainTabId)) {
      setMapValue(
        () => this.state.activeExplainTabIdByConnection,
        (m) => (this.state.activeExplainTabIdByConnection = m),
        connectionId,
        persistedState.activeExplainTabId
      );
    } else if (explainTabs.length > 0) {
      setMapValue(
        () => this.state.activeExplainTabIdByConnection,
        (m) => (this.state.activeExplainTabIdByConnection = m),
        connectionId,
        explainTabs[0].id
      );
    }

    // Restore tab order (if available, otherwise will use timestamp ordering)
    if (persistedState.tabOrder && persistedState.tabOrder.length > 0) {
      setMapValue(
        () => this.state.tabOrderByConnection,
        (m) => (this.state.tabOrderByConnection = m),
        connectionId,
        persistedState.tabOrder
      );
    }

    // Restore active view
    this.state.activeView = persistedState.activeView;

    return queryTabs.length > 0;
  }
}
