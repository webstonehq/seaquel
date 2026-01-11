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
	Project,
	StarterTab
} from '$lib/types';

/**
 * Central state container for the database module.
 * All reactive state and derived values are declared here.
 * Modules receive this instance and read/write state through it.
 *
 * State is organized using Records (objects) instead of Maps for simpler
 * reactivity updates using spread syntax.
 *
 * Tabs are organized per-PROJECT (not per-connection) to allow:
 * - Switching between projects with separate tab sets
 * - Executing queries against different connections within the same project
 *
 * Query history and saved queries remain per-CONNECTION since they are
 * tied to the specific connection that executed them.
 */
export class DatabaseState {
	// === PROJECT STATE ===
	projects = $state<Project[]>([]);
	projectsLoading = $state(true);
	activeProjectId = $state<string | null>(null);

	// === CONNECTION STATE ===
	connections = $state<DatabaseConnection[]>([]);
	connectionsLoading = $state(true);
	schemas = $state<Record<string, SchemaTable[]>>({});

	// Active connection tracked per project
	activeConnectionIdByProject = $state<Record<string, string | null>>({});

	// === TABS STATE (per-project) ===
	queryTabsByProject = $state<Record<string, QueryTab[]>>({});
	activeQueryTabIdByProject = $state<Record<string, string | null>>({});

	schemaTabsByProject = $state<Record<string, SchemaTab[]>>({});
	activeSchemaTabIdByProject = $state<Record<string, string | null>>({});

	explainTabsByProject = $state<Record<string, ExplainTab[]>>({});
	activeExplainTabIdByProject = $state<Record<string, string | null>>({});

	erdTabsByProject = $state<Record<string, ErdTab[]>>({});
	activeErdTabIdByProject = $state<Record<string, string | null>>({});

	// === STARTER TABS STATE (per-project) ===
	// Shown when no connection is active
	starterTabsByProject = $state<Record<string, StarterTab[]>>({});
	activeStarterTabIdByProject = $state<Record<string, string | null>>({});

	// Tab ordering state (stores ordered array of all tab IDs per project)
	tabOrderByProject = $state<Record<string, string[]>>({});

	// === QUERY DATA STATE (per-connection) ===
	queryHistoryByConnection = $state<Record<string, QueryHistoryItem[]>>({});
	savedQueriesByConnection = $state<Record<string, SavedQuery[]>>({});

	// === AI STATE ===
	aiMessages = $state<AIMessage[]>([]);
	isAIOpen = $state(false);

	// === VIEW STATE ===
	activeView = $state<'query' | 'schema' | 'explain' | 'erd'>('query');

	// === PROJECT DERIVED VALUES ===

	// Derived: active project object
	activeProject = $derived(this.projects.find((p) => p.id === this.activeProjectId) || null);

	// Derived: connections for active project
	projectConnections = $derived(
		this.activeProjectId
			? this.connections.filter((c) => c.projectId === this.activeProjectId)
			: []
	);

	// === CONNECTION DERIVED VALUES ===

	// Derived: active connection ID for current project
	activeConnectionId = $derived(
		this.activeProjectId
			? (this.activeConnectionIdByProject[this.activeProjectId] ?? null)
			: null
	);

	// Derived: active connection object
	activeConnection = $derived(
		this.connections.find((c) => c.id === this.activeConnectionId) || null
	);

	// Derived: schema for active connection
	activeSchema = $derived(
		this.activeConnectionId ? (this.schemas[this.activeConnectionId] ?? []) : []
	);

	// === QUERY TAB DERIVED VALUES ===

	// Derived: query tabs for active project
	queryTabs = $derived(
		this.activeProjectId ? (this.queryTabsByProject[this.activeProjectId] ?? []) : []
	);

	// Derived: active query tab ID for active project
	activeQueryTabId = $derived(
		this.activeProjectId
			? (this.activeQueryTabIdByProject[this.activeProjectId] ?? null)
			: null
	);

	// Derived: active query tab object
	activeQueryTab = $derived(this.queryTabs.find((t) => t.id === this.activeQueryTabId) || null);

	// Derived: active query result (for multi-statement support)
	activeQueryResult = $derived(
		this.activeQueryTab?.results?.[this.activeQueryTab.activeResultIndex ?? 0] || null
	);

	// === SCHEMA TAB DERIVED VALUES ===

	// Derived: schema tabs for active project
	schemaTabs = $derived(
		this.activeProjectId ? (this.schemaTabsByProject[this.activeProjectId] ?? []) : []
	);

	// Derived: active schema tab ID for active project
	activeSchemaTabId = $derived(
		this.activeProjectId
			? (this.activeSchemaTabIdByProject[this.activeProjectId] ?? null)
			: null
	);

	// Derived: active schema tab object
	activeSchemaTab = $derived(this.schemaTabs.find((t) => t.id === this.activeSchemaTabId) || null);

	// === EXPLAIN TAB DERIVED VALUES ===

	// Derived: explain tabs for active project
	explainTabs = $derived(
		this.activeProjectId ? (this.explainTabsByProject[this.activeProjectId] ?? []) : []
	);

	// Derived: active explain tab ID for active project
	activeExplainTabId = $derived(
		this.activeProjectId
			? (this.activeExplainTabIdByProject[this.activeProjectId] ?? null)
			: null
	);

	// Derived: active explain tab object
	activeExplainTab = $derived(
		this.explainTabs.find((t) => t.id === this.activeExplainTabId) || null
	);

	// === ERD TAB DERIVED VALUES ===

	// Derived: ERD tabs for active project
	erdTabs = $derived(
		this.activeProjectId ? (this.erdTabsByProject[this.activeProjectId] ?? []) : []
	);

	// Derived: active ERD tab ID for active project
	activeErdTabId = $derived(
		this.activeProjectId ? (this.activeErdTabIdByProject[this.activeProjectId] ?? null) : null
	);

	// Derived: active ERD tab object
	activeErdTab = $derived(this.erdTabs.find((t) => t.id === this.activeErdTabId) || null);

	// === STARTER TAB DERIVED VALUES ===

	// Derived: starter tabs for active project
	starterTabs = $derived(
		this.activeProjectId ? (this.starterTabsByProject[this.activeProjectId] ?? []) : []
	);

	// Derived: active starter tab ID for active project
	activeStarterTabId = $derived(
		this.activeProjectId
			? (this.activeStarterTabIdByProject[this.activeProjectId] ?? null)
			: null
	);

	// Derived: active starter tab object
	activeStarterTab = $derived(
		this.starterTabs.find((t) => t.id === this.activeStarterTabId) || null
	);

	// === QUERY DATA DERIVED VALUES ===

	// Derived: query history for active connection
	activeConnectionQueryHistory = $derived(
		this.activeConnectionId ? (this.queryHistoryByConnection[this.activeConnectionId] ?? []) : []
	);

	// Derived: saved queries for active connection
	activeConnectionSavedQueries = $derived(
		this.activeConnectionId ? (this.savedQueriesByConnection[this.activeConnectionId] ?? []) : []
	);
}
