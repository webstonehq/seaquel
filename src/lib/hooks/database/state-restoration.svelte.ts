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
   * Initialize all per-connection records for a new connection.
   */
  initializeConnectionMaps(connectionId: string): void {
    this.state.queryTabsByConnection = {
      ...this.state.queryTabsByConnection,
      [connectionId]: [],
    };
    this.state.schemaTabsByConnection = {
      ...this.state.schemaTabsByConnection,
      [connectionId]: [],
    };
    this.state.activeQueryTabIdByConnection = {
      ...this.state.activeQueryTabIdByConnection,
      [connectionId]: null,
    };
    this.state.activeSchemaTabIdByConnection = {
      ...this.state.activeSchemaTabIdByConnection,
      [connectionId]: null,
    };
    this.state.queryHistoryByConnection = {
      ...this.state.queryHistoryByConnection,
      [connectionId]: [],
    };
    this.state.savedQueriesByConnection = {
      ...this.state.savedQueriesByConnection,
      [connectionId]: [],
    };
    this.state.explainTabsByConnection = {
      ...this.state.explainTabsByConnection,
      [connectionId]: [],
    };
    this.state.activeExplainTabIdByConnection = {
      ...this.state.activeExplainTabIdByConnection,
      [connectionId]: null,
    };
    this.state.erdTabsByConnection = {
      ...this.state.erdTabsByConnection,
      [connectionId]: [],
    };
    this.state.activeErdTabIdByConnection = {
      ...this.state.activeErdTabIdByConnection,
      [connectionId]: null,
    };
    this.state.tabOrderByConnection = {
      ...this.state.tabOrderByConnection,
      [connectionId]: [],
    };
    this.state.schemas = {
      ...this.state.schemas,
      [connectionId]: [],
    };
  }

  /**
   * Clean up all per-connection records when removing a connection.
   */
  cleanupConnectionMaps(connectionId: string): void {
    const { [connectionId]: _, ...restQueryTabs } = this.state.queryTabsByConnection;
    this.state.queryTabsByConnection = restQueryTabs;

    const { [connectionId]: _2, ...restSchemaTabs } = this.state.schemaTabsByConnection;
    this.state.schemaTabsByConnection = restSchemaTabs;

    const { [connectionId]: _3, ...restActiveQueryTab } = this.state.activeQueryTabIdByConnection;
    this.state.activeQueryTabIdByConnection = restActiveQueryTab;

    const { [connectionId]: _4, ...restActiveSchemaTab } = this.state.activeSchemaTabIdByConnection;
    this.state.activeSchemaTabIdByConnection = restActiveSchemaTab;

    const { [connectionId]: _5, ...restQueryHistory } = this.state.queryHistoryByConnection;
    this.state.queryHistoryByConnection = restQueryHistory;

    const { [connectionId]: _6, ...restSavedQueries } = this.state.savedQueriesByConnection;
    this.state.savedQueriesByConnection = restSavedQueries;

    const { [connectionId]: _7, ...restExplainTabs } = this.state.explainTabsByConnection;
    this.state.explainTabsByConnection = restExplainTabs;

    const { [connectionId]: _8, ...restActiveExplainTab } = this.state.activeExplainTabIdByConnection;
    this.state.activeExplainTabIdByConnection = restActiveExplainTab;

    const { [connectionId]: _9, ...restErdTabs } = this.state.erdTabsByConnection;
    this.state.erdTabsByConnection = restErdTabs;

    const { [connectionId]: _10, ...restActiveErdTab } = this.state.activeErdTabIdByConnection;
    this.state.activeErdTabIdByConnection = restActiveErdTab;

    const { [connectionId]: _11, ...restTabOrder } = this.state.tabOrderByConnection;
    this.state.tabOrderByConnection = restTabOrder;

    const { [connectionId]: _12, ...restSchemas } = this.state.schemas;
    this.state.schemas = restSchemas;
  }

  /**
   * Ensure records exist for a connection (used during reconnect to preserve existing state).
   */
  ensureConnectionMapsExist(connectionId: string): void {
    if (!(connectionId in this.state.queryTabsByConnection)) {
      this.state.queryTabsByConnection = {
        ...this.state.queryTabsByConnection,
        [connectionId]: [],
      };
    }
    if (!(connectionId in this.state.schemaTabsByConnection)) {
      this.state.schemaTabsByConnection = {
        ...this.state.schemaTabsByConnection,
        [connectionId]: [],
      };
    }
    if (!(connectionId in this.state.activeQueryTabIdByConnection)) {
      this.state.activeQueryTabIdByConnection = {
        ...this.state.activeQueryTabIdByConnection,
        [connectionId]: null,
      };
    }
    if (!(connectionId in this.state.activeSchemaTabIdByConnection)) {
      this.state.activeSchemaTabIdByConnection = {
        ...this.state.activeSchemaTabIdByConnection,
        [connectionId]: null,
      };
    }
    if (!(connectionId in this.state.queryHistoryByConnection)) {
      this.state.queryHistoryByConnection = {
        ...this.state.queryHistoryByConnection,
        [connectionId]: [],
      };
    }
    if (!(connectionId in this.state.savedQueriesByConnection)) {
      this.state.savedQueriesByConnection = {
        ...this.state.savedQueriesByConnection,
        [connectionId]: [],
      };
    }
    if (!(connectionId in this.state.explainTabsByConnection)) {
      this.state.explainTabsByConnection = {
        ...this.state.explainTabsByConnection,
        [connectionId]: [],
      };
    }
    if (!(connectionId in this.state.activeExplainTabIdByConnection)) {
      this.state.activeExplainTabIdByConnection = {
        ...this.state.activeExplainTabIdByConnection,
        [connectionId]: null,
      };
    }
    if (!(connectionId in this.state.erdTabsByConnection)) {
      this.state.erdTabsByConnection = {
        ...this.state.erdTabsByConnection,
        [connectionId]: [],
      };
    }
    if (!(connectionId in this.state.activeErdTabIdByConnection)) {
      this.state.activeErdTabIdByConnection = {
        ...this.state.activeErdTabIdByConnection,
        [connectionId]: null,
      };
    }
    if (!(connectionId in this.state.tabOrderByConnection)) {
      this.state.tabOrderByConnection = {
        ...this.state.tabOrderByConnection,
        [connectionId]: [],
      };
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
    this.state.savedQueriesByConnection = {
      ...this.state.savedQueriesByConnection,
      [connectionId]: savedQueries,
    };
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
    this.state.queryHistoryByConnection = {
      ...this.state.queryHistoryByConnection,
      [connectionId]: history,
    };
  }

  /**
   * Restore schema tabs matching current schema.
   */
  private restoreSchemaTabs(connectionId: string, persistedTabs: PersistedSchemaTab[]): void {
    const schemas = this.state.schemas[connectionId] ?? [];
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

    this.state.schemaTabsByConnection = {
      ...this.state.schemaTabsByConnection,
      [connectionId]: schemaTabs,
    };
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
    this.state.queryTabsByConnection = {
      ...this.state.queryTabsByConnection,
      [connectionId]: queryTabs,
    };

    // Restore explain tabs (without results - they'll need to be re-executed)
    const explainTabs: ExplainTab[] = persistedState.explainTabs.map((pt) => ({
      id: pt.id,
      name: pt.name,
      sourceQuery: pt.sourceQuery,
      isExecuting: false,
      result: undefined,
    }));
    this.state.explainTabsByConnection = {
      ...this.state.explainTabsByConnection,
      [connectionId]: explainTabs,
    };

    // Restore schema tabs - need to match against current schema
    this.restoreSchemaTabs(connectionId, persistedState.schemaTabs);

    // Restore active tab IDs (only if tabs exist)
    if (persistedState.activeQueryTabId && queryTabs.some((t) => t.id === persistedState.activeQueryTabId)) {
      this.state.activeQueryTabIdByConnection = {
        ...this.state.activeQueryTabIdByConnection,
        [connectionId]: persistedState.activeQueryTabId,
      };
    } else if (queryTabs.length > 0) {
      this.state.activeQueryTabIdByConnection = {
        ...this.state.activeQueryTabIdByConnection,
        [connectionId]: queryTabs[0].id,
      };
    }

    const schemaTabs = this.state.schemaTabsByConnection[connectionId] ?? [];
    if (persistedState.activeSchemaTabId && schemaTabs.some((t) => t.id === persistedState.activeSchemaTabId)) {
      this.state.activeSchemaTabIdByConnection = {
        ...this.state.activeSchemaTabIdByConnection,
        [connectionId]: persistedState.activeSchemaTabId,
      };
    } else if (schemaTabs.length > 0) {
      this.state.activeSchemaTabIdByConnection = {
        ...this.state.activeSchemaTabIdByConnection,
        [connectionId]: schemaTabs[0].id,
      };
    }

    if (persistedState.activeExplainTabId && explainTabs.some((t) => t.id === persistedState.activeExplainTabId)) {
      this.state.activeExplainTabIdByConnection = {
        ...this.state.activeExplainTabIdByConnection,
        [connectionId]: persistedState.activeExplainTabId,
      };
    } else if (explainTabs.length > 0) {
      this.state.activeExplainTabIdByConnection = {
        ...this.state.activeExplainTabIdByConnection,
        [connectionId]: explainTabs[0].id,
      };
    }

    // Restore tab order (if available, otherwise will use timestamp ordering)
    if (persistedState.tabOrder && persistedState.tabOrder.length > 0) {
      this.state.tabOrderByConnection = {
        ...this.state.tabOrderByConnection,
        [connectionId]: persistedState.tabOrder,
      };
    }

    // Restore active view
    this.state.activeView = persistedState.activeView;

    return queryTabs.length > 0;
  }
}
