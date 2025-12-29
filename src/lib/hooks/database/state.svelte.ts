import type {
  DatabaseConnection,
  SchemaTable,
  QueryTab,
  QueryHistoryItem,
  AIMessage,
  SchemaTab,
  SavedQuery,
  ExplainTab,
  ErdTab,
} from "$lib/types";

/**
 * Central state container for the database module.
 * All reactive state and derived values are declared here.
 * Modules receive this instance and read/write state through it.
 */
export class DatabaseState {
  // Core state
  connections = $state<DatabaseConnection[]>([]);
  activeConnectionId = $state<string | null>(null);
  schemas = $state<Map<string, SchemaTable[]>>(new Map());

  // Query tabs state
  queryTabsByConnection = $state<Map<string, QueryTab[]>>(new Map());
  activeQueryTabIdByConnection = $state<Map<string, string | null>>(new Map());

  // Schema tabs state
  schemaTabsByConnection = $state<Map<string, SchemaTab[]>>(new Map());
  activeSchemaTabIdByConnection = $state<Map<string, string | null>>(new Map());

  // Query history and saved queries
  queryHistoryByConnection = $state<Map<string, QueryHistoryItem[]>>(new Map());
  savedQueriesByConnection = $state<Map<string, SavedQuery[]>>(new Map());

  // Explain tabs state
  explainTabsByConnection = $state<Map<string, ExplainTab[]>>(new Map());
  activeExplainTabIdByConnection = $state<Map<string, string | null>>(new Map());

  // ERD tabs state
  erdTabsByConnection = $state<Map<string, ErdTab[]>>(new Map());
  activeErdTabIdByConnection = $state<Map<string, string | null>>(new Map());

  // Tab ordering state (stores ordered array of all tab IDs per connection)
  tabOrderByConnection = $state<Map<string, string[]>>(new Map());

  // AI state
  aiMessages = $state<AIMessage[]>([]);
  isAIOpen = $state(false);

  // View state
  activeView = $state<"query" | "schema" | "explain" | "erd">("query");

  // Derived: active connection object
  activeConnection = $derived(
    this.connections.find((c) => c.id === this.activeConnectionId) || null,
  );

  // Derived: query tabs for active connection
  queryTabs = $derived(
    this.activeConnectionId
      ? this.queryTabsByConnection.get(this.activeConnectionId) || []
      : [],
  );

  // Derived: active query tab ID for active connection
  activeQueryTabId = $derived(
    this.activeConnectionId
      ? this.activeQueryTabIdByConnection.get(this.activeConnectionId) || null
      : null,
  );

  // Derived: active query tab object
  activeQueryTab = $derived(
    this.queryTabs.find((t) => t.id === this.activeQueryTabId) || null,
  );

  // Derived: schema tabs for active connection
  schemaTabs = $derived(
    this.activeConnectionId
      ? this.schemaTabsByConnection.get(this.activeConnectionId) || []
      : [],
  );

  // Derived: active schema tab ID for active connection
  activeSchemaTabId = $derived(
    this.activeConnectionId
      ? this.activeSchemaTabIdByConnection.get(this.activeConnectionId) || null
      : null,
  );

  // Derived: active schema tab object
  activeSchemaTab = $derived(
    this.schemaTabs.find((t) => t.id === this.activeSchemaTabId) || null,
  );

  // Derived: schema for active connection
  activeSchema = $derived(
    this.activeConnectionId
      ? this.schemas.get(this.activeConnectionId) || []
      : [],
  );

  // Derived: query history for active connection
  activeConnectionQueryHistory = $derived(
    this.activeConnectionId
      ? this.queryHistoryByConnection.get(this.activeConnectionId) || []
      : [],
  );

  // Derived: saved queries for active connection
  activeConnectionSavedQueries = $derived(
    this.activeConnectionId
      ? this.savedQueriesByConnection.get(this.activeConnectionId) || []
      : [],
  );

  // Derived: explain tabs for active connection
  explainTabs = $derived(
    this.activeConnectionId
      ? this.explainTabsByConnection.get(this.activeConnectionId) || []
      : [],
  );

  // Derived: active explain tab ID for active connection
  activeExplainTabId = $derived(
    this.activeConnectionId
      ? this.activeExplainTabIdByConnection.get(this.activeConnectionId) || null
      : null,
  );

  // Derived: active explain tab object
  activeExplainTab = $derived(
    this.explainTabs.find((t) => t.id === this.activeExplainTabId) || null,
  );

  // Derived: ERD tabs for active connection
  erdTabs = $derived(
    this.activeConnectionId
      ? this.erdTabsByConnection.get(this.activeConnectionId) || []
      : [],
  );

  // Derived: active ERD tab ID for active connection
  activeErdTabId = $derived(
    this.activeConnectionId
      ? this.activeErdTabIdByConnection.get(this.activeConnectionId) || null
      : null,
  );

  // Derived: active ERD tab object
  activeErdTab = $derived(
    this.erdTabs.find((t) => t.id === this.activeErdTabId) || null,
  );
}
