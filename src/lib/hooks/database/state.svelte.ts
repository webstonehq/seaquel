import type {
	DatabaseConnection,
	SchemaTable,
	QueryTab,
	QueryHistoryItem,
	AIMessage,
	SchemaTab,
	SavedQuery,
	ExplainTab,
	ErdTab
} from '$lib/types';

/**
 * Central state container for the database module.
 * All reactive state and derived values are declared here.
 * Modules receive this instance and read/write state through it.
 *
 * State is organized using Records (objects) instead of Maps for simpler
 * reactivity updates using spread syntax.
 */
export class DatabaseState {
	// Core state
	connections = $state<DatabaseConnection[]>([]);
	connectionsLoading = $state(true);
	activeConnectionId = $state<string | null>(null);
	schemas = $state<Record<string, SchemaTable[]>>({});

	// Query tabs state
	queryTabsByConnection = $state<Record<string, QueryTab[]>>({});
	activeQueryTabIdByConnection = $state<Record<string, string | null>>({});

	// Schema tabs state
	schemaTabsByConnection = $state<Record<string, SchemaTab[]>>({});
	activeSchemaTabIdByConnection = $state<Record<string, string | null>>({});

	// Query history and saved queries
	queryHistoryByConnection = $state<Record<string, QueryHistoryItem[]>>({});
	savedQueriesByConnection = $state<Record<string, SavedQuery[]>>({});

	// Explain tabs state
	explainTabsByConnection = $state<Record<string, ExplainTab[]>>({});
	activeExplainTabIdByConnection = $state<Record<string, string | null>>({});

	// ERD tabs state
	erdTabsByConnection = $state<Record<string, ErdTab[]>>({});
	activeErdTabIdByConnection = $state<Record<string, string | null>>({});

	// Tab ordering state (stores ordered array of all tab IDs per connection)
	tabOrderByConnection = $state<Record<string, string[]>>({});

	// AI state
	aiMessages = $state<AIMessage[]>([]);
	isAIOpen = $state(false);

	// View state
	activeView = $state<'query' | 'schema' | 'explain' | 'erd'>('query');

	// Derived: active connection object
	activeConnection = $derived(
		this.connections.find((c) => c.id === this.activeConnectionId) || null
	);

	// Derived: query tabs for active connection
	queryTabs = $derived(
		this.activeConnectionId ? (this.queryTabsByConnection[this.activeConnectionId] ?? []) : []
	);

	// Derived: active query tab ID for active connection
	activeQueryTabId = $derived(
		this.activeConnectionId
			? (this.activeQueryTabIdByConnection[this.activeConnectionId] ?? null)
			: null
	);

	// Derived: active query tab object
	activeQueryTab = $derived(this.queryTabs.find((t) => t.id === this.activeQueryTabId) || null);

	// Derived: active query result (for multi-statement support)
	activeQueryResult = $derived(
		this.activeQueryTab?.results?.[this.activeQueryTab.activeResultIndex ?? 0] || null
	);

	// Derived: schema tabs for active connection
	schemaTabs = $derived(
		this.activeConnectionId ? (this.schemaTabsByConnection[this.activeConnectionId] ?? []) : []
	);

	// Derived: active schema tab ID for active connection
	activeSchemaTabId = $derived(
		this.activeConnectionId
			? (this.activeSchemaTabIdByConnection[this.activeConnectionId] ?? null)
			: null
	);

	// Derived: active schema tab object
	activeSchemaTab = $derived(this.schemaTabs.find((t) => t.id === this.activeSchemaTabId) || null);

	// Derived: schema for active connection
	activeSchema = $derived(
		this.activeConnectionId ? (this.schemas[this.activeConnectionId] ?? []) : []
	);

	// Derived: query history for active connection
	activeConnectionQueryHistory = $derived(
		this.activeConnectionId ? (this.queryHistoryByConnection[this.activeConnectionId] ?? []) : []
	);

	// Derived: saved queries for active connection
	activeConnectionSavedQueries = $derived(
		this.activeConnectionId ? (this.savedQueriesByConnection[this.activeConnectionId] ?? []) : []
	);

	// Derived: explain tabs for active connection
	explainTabs = $derived(
		this.activeConnectionId ? (this.explainTabsByConnection[this.activeConnectionId] ?? []) : []
	);

	// Derived: active explain tab ID for active connection
	activeExplainTabId = $derived(
		this.activeConnectionId
			? (this.activeExplainTabIdByConnection[this.activeConnectionId] ?? null)
			: null
	);

	// Derived: active explain tab object
	activeExplainTab = $derived(
		this.explainTabs.find((t) => t.id === this.activeExplainTabId) || null
	);

	// Derived: ERD tabs for active connection
	erdTabs = $derived(
		this.activeConnectionId ? (this.erdTabsByConnection[this.activeConnectionId] ?? []) : []
	);

	// Derived: active ERD tab ID for active connection
	activeErdTabId = $derived(
		this.activeConnectionId
			? (this.activeErdTabIdByConnection[this.activeConnectionId] ?? null)
			: null
	);

	// Derived: active ERD tab object
	activeErdTab = $derived(this.erdTabs.find((t) => t.id === this.activeErdTabId) || null);
}
