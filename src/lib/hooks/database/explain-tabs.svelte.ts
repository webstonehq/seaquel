import { toast } from 'svelte-sonner';
import type { ExplainTab, ExplainResult, ExplainPlanNode, ParameterValue } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { getAdapter, type ExplainNode } from '$lib/db';
import { getStatementAtOffset } from '$lib/db/sql-parser';
import { substituteParameters } from '$lib/db/query-params';
import { getProvider, getDuckDBProvider, type DatabaseProvider } from '$lib/providers';

/**
 * Callback for setting explain result on a query tab.
 */
export type SetExplainResultCallback = (
	tabId: string,
	result: ExplainResult,
	sourceQuery: string,
	isAnalyze: boolean
) => void;

/**
 * Callback for setting explain executing state on a query tab.
 */
export type SetExplainExecutingCallback = (
	tabId: string,
	isExecuting: boolean,
	isAnalyze: boolean
) => void;

/**
 * Manages EXPLAIN/ANALYZE tabs: execute, remove, set active.
 * Tabs are organized per-project.
 */
export class ExplainTabManager {
	private setExplainResult?: SetExplainResultCallback;
	private setExplainExecuting?: SetExplainExecutingCallback;

	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (projectId: string | null) => void,
		private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd') => void
	) {}

	/**
	 * Set callbacks for embedded explain results (stored on QueryTab).
	 */
	setEmbeddedCallbacks(
		setResult: SetExplainResultCallback,
		setExecuting: SetExplainExecutingCallback
	): void {
		this.setExplainResult = setResult;
		this.setExplainExecuting = setExecuting;
	}

	/**
	 * Get the appropriate provider based on connection type.
	 */
	private async getProviderForConnection(): Promise<DatabaseProvider> {
		const connection = this.state.activeConnection;
		if (connection?.type === 'duckdb') {
			return getDuckDBProvider();
		}
		return getProvider();
	}

	/**
	 * Execute EXPLAIN or EXPLAIN ANALYZE and store result embedded in the query tab.
	 * This is the new approach where results appear below the editor instead of in a separate tab.
	 */
	async executeEmbedded(tabId: string, analyze: boolean = false, cursorOffset?: number): Promise<void> {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return;
		if (!this.setExplainResult || !this.setExplainExecuting) {
			console.warn('Embedded callbacks not set, falling back to tab-based explain');
			return this.execute(tabId, analyze, cursorOffset);
		}

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab || !tab.query.trim()) return;

		// Get the statement to explain based on cursor position
		const dbType = this.state.activeConnection.type;
		let queryToExplain = tab.query;

		if (cursorOffset !== undefined) {
			const statement = getStatementAtOffset(tab.query, cursorOffset, dbType);
			if (statement) {
				queryToExplain = statement.sql;
			}
		}

		if (!queryToExplain.trim()) return;

		// Mark as executing
		this.setExplainExecuting(tabId, true, analyze);

		try {
			const adapter = getAdapter(this.state.activeConnection.type);
			const explainQuery = adapter.getExplainQuery(queryToExplain, analyze);
			const providerConnectionId = this.state.activeConnection.providerConnectionId;

			if (!providerConnectionId) {
				throw new Error('No connection established');
			}

			const provider = await this.getProviderForConnection();

			// For SQLite with analyze=true, we need to actually execute the query
			let actualRowCount: number | undefined;
			let executionTime: number | undefined;

			if (dbType === 'sqlite' && analyze) {
				const startTime = performance.now();
				const queryResult = await provider.select(providerConnectionId, queryToExplain);
				executionTime = performance.now() - startTime;
				actualRowCount = queryResult.length;
			}

			const result = await provider.select(providerConnectionId, explainQuery);

			// Use adapter to parse the results into common format
			const parsedNode = adapter.parseExplainResult(result, analyze);

			// For SQLite analyze, inject the actual execution stats into the root node
			if (dbType === 'sqlite' && analyze && actualRowCount !== undefined) {
				parsedNode.actualRows = actualRowCount;
				parsedNode.rows = actualRowCount;
				parsedNode.actualTime = executionTime;
			}

			// Convert to ExplainResult format for rendering
			const explainResult: ExplainResult = this.convertExplainNodeToResult(parsedNode, analyze);

			// For SQLite analyze, set execution time on the result
			if (dbType === 'sqlite' && analyze && executionTime !== undefined) {
				explainResult.executionTime = executionTime;
			}

			// Store result on query tab (use full query for staleness detection)
			this.setExplainResult(tabId, explainResult, tab.query, analyze);
		} catch (error) {
			this.setExplainExecuting(tabId, false, analyze);
			toast.error(`Explain failed: ${error}`);
		}
	}

	/**
	 * Execute EXPLAIN or EXPLAIN ANALYZE with parameter substitution (embedded version).
	 */
	async executeEmbeddedWithParams(
		tabId: string,
		parameterValues: ParameterValue[],
		analyze: boolean = false,
		cursorOffset?: number
	): Promise<void> {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return;
		if (!this.setExplainResult || !this.setExplainExecuting) {
			console.warn('Embedded callbacks not set, falling back to tab-based explain');
			return this.executeWithParams(tabId, parameterValues, analyze, cursorOffset);
		}

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab || !tab.query.trim()) return;

		// Get the statement to explain based on cursor position
		const dbType = this.state.activeConnection.type;
		let queryToExplain = tab.query;

		if (cursorOffset !== undefined) {
			const statement = getStatementAtOffset(tab.query, cursorOffset, dbType);
			if (statement) {
				queryToExplain = statement.sql;
			}
		}

		if (!queryToExplain.trim()) return;

		// Substitute parameters in the query
		const { sql: substitutedQuery, bindValues } = substituteParameters(
			queryToExplain,
			parameterValues,
			dbType
		);

		// Mark as executing
		this.setExplainExecuting(tabId, true, analyze);

		try {
			const adapter = getAdapter(this.state.activeConnection.type);
			const explainQuery = adapter.getExplainQuery(substitutedQuery, analyze);
			const providerConnectionId = this.state.activeConnection.providerConnectionId;

			if (!providerConnectionId) {
				throw new Error('No connection established');
			}

			const provider = await this.getProviderForConnection();

			// For SQLite with analyze=true, we need to actually execute the query
			let actualRowCount: number | undefined;
			let executionTime: number | undefined;

			if (dbType === 'sqlite' && analyze) {
				const startTime = performance.now();
				const queryResult = await provider.select(providerConnectionId, substitutedQuery, bindValues);
				executionTime = performance.now() - startTime;
				actualRowCount = queryResult.length;
			}

			// For EXPLAIN queries, bind values handling
			const useBindValues = dbType !== 'mssql' && dbType !== 'duckdb';
			const result = await provider.select(
				providerConnectionId,
				explainQuery,
				useBindValues ? bindValues : undefined
			);

			// Use adapter to parse the results into common format
			const parsedNode = adapter.parseExplainResult(result, analyze);

			// For SQLite analyze, inject the actual execution stats into the root node
			if (dbType === 'sqlite' && analyze && actualRowCount !== undefined) {
				parsedNode.actualRows = actualRowCount;
				parsedNode.rows = actualRowCount;
				parsedNode.actualTime = executionTime;
			}

			// Convert to ExplainResult format for rendering
			const explainResult: ExplainResult = this.convertExplainNodeToResult(parsedNode, analyze);

			// For SQLite analyze, set execution time on the result
			if (dbType === 'sqlite' && analyze && executionTime !== undefined) {
				explainResult.executionTime = executionTime;
			}

			// Store result on query tab (use full query for staleness detection)
			this.setExplainResult(tabId, explainResult, tab.query, analyze);
		} catch (error) {
			this.setExplainExecuting(tabId, false, analyze);
			toast.error(`Explain failed: ${error}`);
		}
	}

	/**
	 * Convert ExplainNode from adapter to ExplainResult for rendering.
	 */
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
				children: (n.children || []).map((child) => convertNode(child))
			};
		};

		return {
			plan: convertNode(node),
			planningTime: 0,
			executionTime: undefined,
			isAnalyze
		};
	}

	/**
	 * Execute EXPLAIN or EXPLAIN ANALYZE on a query tab.
	 * If cursorOffset is provided, explains only the statement at that cursor position.
	 */
	async execute(tabId: string, analyze: boolean = false, cursorOffset?: number): Promise<void> {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab || !tab.query.trim()) return;

		// Get the statement to explain based on cursor position
		const dbType = this.state.activeConnection.type;
		let queryToExplain = tab.query;

		if (cursorOffset !== undefined) {
			const statement = getStatementAtOffset(tab.query, cursorOffset, dbType);
			if (statement) {
				queryToExplain = statement.sql;
			}
		}

		if (!queryToExplain.trim()) return;

		// Create a new explain tab
		const explainTabs = this.state.explainTabsByProject[projectId] ?? [];
		const explainTabId = `explain-${Date.now()}`;
		const queryPreview = queryToExplain.substring(0, 30).replace(/\s+/g, ' ').trim();
		const newExplainTab: ExplainTab = $state({
			id: explainTabId,
			name: analyze ? `Analyze: ${queryPreview}...` : `Explain: ${queryPreview}...`,
			sourceQuery: queryToExplain,
			result: undefined,
			isExecuting: true
		});

		this.state.explainTabsByProject = {
			...this.state.explainTabsByProject,
			[projectId]: [...explainTabs, newExplainTab]
		};

		this.tabOrdering.add(explainTabId);

		// Set as active and switch view
		this.state.activeExplainTabIdByProject = {
			...this.state.activeExplainTabIdByProject,
			[projectId]: explainTabId
		};
		this.setActiveView('explain');
		this.schedulePersistence(projectId);

		try {
			const adapter = getAdapter(this.state.activeConnection.type);
			const explainQuery = adapter.getExplainQuery(queryToExplain, analyze);
			const providerConnectionId = this.state.activeConnection.providerConnectionId;

			if (!providerConnectionId) {
				throw new Error('No connection established');
			}

			const provider = await this.getProviderForConnection();

			// For SQLite with analyze=true, we need to actually execute the query
			// to get real row counts and timing, since SQLite's EXPLAIN QUERY PLAN
			// doesn't provide this information
			let actualRowCount: number | undefined;
			let executionTime: number | undefined;

			if (dbType === 'sqlite' && analyze) {
				const startTime = performance.now();
				const queryResult = await provider.select(providerConnectionId, queryToExplain);
				executionTime = performance.now() - startTime;
				actualRowCount = queryResult.length;
			}

			const result = await provider.select(providerConnectionId, explainQuery);

			// Use adapter to parse the results into common format
			const parsedNode = adapter.parseExplainResult(result, analyze);

			// For SQLite analyze, inject the actual execution stats into the root node
			if (dbType === 'sqlite' && analyze && actualRowCount !== undefined) {
				parsedNode.actualRows = actualRowCount;
				parsedNode.rows = actualRowCount;
				parsedNode.actualTime = executionTime;
			}

			// Convert to ExplainResult format for rendering
			const explainResult: ExplainResult = this.convertExplainNodeToResult(parsedNode, analyze);

			// For SQLite analyze, set execution time on the result
			if (dbType === 'sqlite' && analyze && executionTime !== undefined) {
				explainResult.executionTime = executionTime;
			}

			// Update the explain tab with results
			newExplainTab.result = explainResult;
			newExplainTab.isExecuting = false;

			// Trigger reactivity by creating new array
			const currentTabs = this.state.explainTabsByProject[projectId] ?? [];
			this.state.explainTabsByProject = {
				...this.state.explainTabsByProject,
				[projectId]: [...currentTabs]
			};
		} catch (error) {
			// Remove failed explain tab
			const currentTabs = this.state.explainTabsByProject[projectId] ?? [];
			this.state.explainTabsByProject = {
				...this.state.explainTabsByProject,
				[projectId]: currentTabs.filter((t) => t.id !== explainTabId)
			};

			// Switch back to query view
			this.setActiveView('query');
			toast.error(`Explain failed: ${error}`);
		}
	}

	/**
	 * Execute EXPLAIN or EXPLAIN ANALYZE with parameter substitution.
	 * Substitutes {{param}} placeholders with values before execution.
	 */
	async executeWithParams(
		tabId: string,
		parameterValues: ParameterValue[],
		analyze: boolean = false,
		cursorOffset?: number
	): Promise<void> {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab || !tab.query.trim()) return;

		// Get the statement to explain based on cursor position
		const dbType = this.state.activeConnection.type;
		let queryToExplain = tab.query;

		if (cursorOffset !== undefined) {
			const statement = getStatementAtOffset(tab.query, cursorOffset, dbType);
			if (statement) {
				queryToExplain = statement.sql;
			}
		}

		if (!queryToExplain.trim()) return;

		// Substitute parameters in the query
		const { sql: substitutedQuery, bindValues } = substituteParameters(
			queryToExplain,
			parameterValues,
			dbType
		);

		// Create a new explain tab
		const explainTabs = this.state.explainTabsByProject[projectId] ?? [];
		const explainTabId = `explain-${Date.now()}`;
		const queryPreview = queryToExplain.substring(0, 30).replace(/\s+/g, ' ').trim();
		const newExplainTab: ExplainTab = $state({
			id: explainTabId,
			name: analyze ? `Analyze: ${queryPreview}...` : `Explain: ${queryPreview}...`,
			sourceQuery: queryToExplain, // Keep original with {{}} for display
			result: undefined,
			isExecuting: true
		});

		this.state.explainTabsByProject = {
			...this.state.explainTabsByProject,
			[projectId]: [...explainTabs, newExplainTab]
		};

		this.tabOrdering.add(explainTabId);

		// Set as active and switch view
		this.state.activeExplainTabIdByProject = {
			...this.state.activeExplainTabIdByProject,
			[projectId]: explainTabId
		};
		this.setActiveView('explain');
		this.schedulePersistence(projectId);

		try {
			const adapter = getAdapter(this.state.activeConnection.type);
			const explainQuery = adapter.getExplainQuery(substitutedQuery, analyze);
			const providerConnectionId = this.state.activeConnection.providerConnectionId;

			if (!providerConnectionId) {
				throw new Error('No connection established');
			}

			const provider = await this.getProviderForConnection();

			// For SQLite with analyze=true, we need to actually execute the query
			// to get real row counts and timing
			let actualRowCount: number | undefined;
			let executionTime: number | undefined;

			if (dbType === 'sqlite' && analyze) {
				const startTime = performance.now();
				const queryResult = await provider.select(providerConnectionId, substitutedQuery, bindValues);
				executionTime = performance.now() - startTime;
				actualRowCount = queryResult.length;
			}

			// For EXPLAIN queries, bind values are already inlined for MSSQL/DuckDB,
			// and for PostgreSQL/SQLite we need to pass them
			const useBindValues = dbType !== 'mssql' && dbType !== 'duckdb';
			const result = await provider.select(
				providerConnectionId,
				explainQuery,
				useBindValues ? bindValues : undefined
			);

			// Use adapter to parse the results into common format
			const parsedNode = adapter.parseExplainResult(result, analyze);

			// For SQLite analyze, inject the actual execution stats into the root node
			if (dbType === 'sqlite' && analyze && actualRowCount !== undefined) {
				parsedNode.actualRows = actualRowCount;
				parsedNode.rows = actualRowCount;
				parsedNode.actualTime = executionTime;
			}

			// Convert to ExplainResult format for rendering
			const explainResult: ExplainResult = this.convertExplainNodeToResult(parsedNode, analyze);

			// For SQLite analyze, set execution time on the result
			if (dbType === 'sqlite' && analyze && executionTime !== undefined) {
				explainResult.executionTime = executionTime;
			}

			// Update the explain tab with results
			newExplainTab.result = explainResult;
			newExplainTab.isExecuting = false;

			// Trigger reactivity by creating new array
			const currentTabs = this.state.explainTabsByProject[projectId] ?? [];
			this.state.explainTabsByProject = {
				...this.state.explainTabsByProject,
				[projectId]: [...currentTabs]
			};
		} catch (error) {
			// Remove failed explain tab
			const currentTabs = this.state.explainTabsByProject[projectId] ?? [];
			this.state.explainTabsByProject = {
				...this.state.explainTabsByProject,
				[projectId]: currentTabs.filter((t) => t.id !== explainTabId)
			};

			// Switch back to query view
			this.setActiveView('query');
			toast.error(`Explain failed: ${error}`);
		}
	}

	/**
	 * Remove an explain tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.explainTabsByProject,
			(r) => (this.state.explainTabsByProject = r),
			() => this.state.activeExplainTabIdByProject,
			(r) => (this.state.activeExplainTabIdByProject = r),
			id
		);
		this.schedulePersistence(this.state.activeProjectId);
		// Switch to query view if no explain tabs left
		if (this.state.activeProjectId && this.state.explainTabs.length === 0) {
			this.setActiveView('query');
		}
	}

	/**
	 * Set the active explain tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;

		this.state.activeExplainTabIdByProject = {
			...this.state.activeExplainTabIdByProject,
			[this.state.activeProjectId]: id
		};
		this.schedulePersistence(this.state.activeProjectId);
	}
}
