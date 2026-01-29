import { errorToast } from '$lib/utils/toast';
import type { ExplainTab, ExplainResult, ExplainPlanNode, ParameterValue } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { BaseTabManager, type TabStateAccessors } from './base-tab-manager.svelte.js';
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
export class ExplainTabManager extends BaseTabManager<ExplainTab> {
	private setExplainResult?: SetExplainResultCallback;
	private setExplainExecuting?: SetExplainExecutingCallback;
	private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd') => void;

	constructor(
		state: DatabaseState,
		tabOrdering: TabOrderingManager,
		schedulePersistence: (projectId: string | null) => void,
		setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd') => void
	) {
		super(state, tabOrdering, schedulePersistence);
		this.setActiveView = setActiveView;
	}

	protected get accessors(): TabStateAccessors<ExplainTab> {
		return {
			getTabs: () => this.state.explainTabsByProject,
			setTabs: (r) => (this.state.explainTabsByProject = r),
			getActiveId: () => this.state.activeExplainTabIdByProject,
			setActiveId: (r) => (this.state.activeExplainTabIdByProject = r)
		};
	}

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
			errorToast(`Explain failed: ${error}`);
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
			errorToast(`Explain failed: ${error}`);
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
		const explainTabId = `explain-${Date.now()}`;
		const queryPreview = queryToExplain.substring(0, 30).replace(/\s+/g, ' ').trim();
		const newExplainTab: ExplainTab = $state({
			id: explainTabId,
			name: analyze ? `Analyze: ${queryPreview}...` : `Explain: ${queryPreview}...`,
			sourceQuery: queryToExplain,
			result: undefined,
			isExecuting: true
		});

		this.appendTab(newExplainTab);
		this.setActiveView('explain');

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
			const currentTabs = this.getProjectTabs();
			this.setProjectTabs([...currentTabs]);
		} catch (error) {
			// Remove failed explain tab
			const currentTabs = this.getProjectTabs();
			this.setProjectTabs(currentTabs.filter((t) => t.id !== explainTabId));

			// Switch back to query view
			this.setActiveView('query');
			errorToast(`Explain failed: ${error}`);
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
		const explainTabId = `explain-${Date.now()}`;
		const queryPreview = queryToExplain.substring(0, 30).replace(/\s+/g, ' ').trim();
		const newExplainTab: ExplainTab = $state({
			id: explainTabId,
			name: analyze ? `Analyze: ${queryPreview}...` : `Explain: ${queryPreview}...`,
			sourceQuery: queryToExplain, // Keep original with {{}} for display
			result: undefined,
			isExecuting: true
		});

		this.appendTab(newExplainTab);
		this.setActiveView('explain');

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
			const currentTabs = this.getProjectTabs();
			this.setProjectTabs([...currentTabs]);
		} catch (error) {
			// Remove failed explain tab
			const currentTabs = this.getProjectTabs();
			this.setProjectTabs(currentTabs.filter((t) => t.id !== explainTabId));

			// Switch back to query view
			this.setActiveView('query');
			errorToast(`Explain failed: ${error}`);
		}
	}

	/**
	 * Remove an explain tab by ID.
	 */
	override remove(id: string): void {
		super.remove(id);
		// Switch to query view if no explain tabs left
		if (this.state.activeProjectId && this.state.explainTabs.length === 0) {
			this.setActiveView('query');
		}
	}
}
