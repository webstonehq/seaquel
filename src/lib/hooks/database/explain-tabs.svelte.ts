import { toast } from 'svelte-sonner';
import type { ExplainTab, ExplainResult, ExplainPlanNode } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { getAdapter, type ExplainNode } from '$lib/db';
import { getStatementAtOffset } from '$lib/db/sql-parser';

/**
 * Manages EXPLAIN/ANALYZE tabs: execute, remove, set active.
 */
export class ExplainTabManager {
	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (connectionId: string | null) => void,
		private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd') => void
	) {}

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
		if (!this.state.activeConnectionId || !this.state.activeConnection) return;

		const connectionId = this.state.activeConnectionId;
		const tabs = this.state.queryTabsByConnection[connectionId] ?? [];
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
		const explainTabs = this.state.explainTabsByConnection[connectionId] ?? [];
		const explainTabId = `explain-${Date.now()}`;
		const queryPreview = queryToExplain.substring(0, 30).replace(/\s+/g, ' ').trim();
		const newExplainTab: ExplainTab = $state({
			id: explainTabId,
			name: analyze ? `Analyze: ${queryPreview}...` : `Explain: ${queryPreview}...`,
			sourceQuery: queryToExplain,
			result: undefined,
			isExecuting: true
		});

		this.state.explainTabsByConnection = {
			...this.state.explainTabsByConnection,
			[connectionId]: [...explainTabs, newExplainTab]
		};

		this.tabOrdering.add(explainTabId);

		// Set as active and switch view
		this.state.activeExplainTabIdByConnection = {
			...this.state.activeExplainTabIdByConnection,
			[connectionId]: explainTabId
		};
		this.setActiveView('explain');
		this.schedulePersistence(connectionId);

		try {
			const adapter = getAdapter(this.state.activeConnection.type);
			const explainQuery = adapter.getExplainQuery(queryToExplain, analyze);

			// For SQLite with analyze=true, we need to actually execute the query
			// to get real row counts and timing, since SQLite's EXPLAIN QUERY PLAN
			// doesn't provide this information
			let actualRowCount: number | undefined;
			let executionTime: number | undefined;

			if (dbType === 'sqlite' && analyze) {
				const startTime = performance.now();
				const queryResult = (await this.state.activeConnection.database!.select(
					queryToExplain
				)) as unknown[];
				executionTime = performance.now() - startTime;
				actualRowCount = queryResult.length;
			}

			const result = (await this.state.activeConnection.database!.select(
				explainQuery
			)) as unknown[];

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
			const currentTabs = this.state.explainTabsByConnection[connectionId] ?? [];
			this.state.explainTabsByConnection = {
				...this.state.explainTabsByConnection,
				[connectionId]: [...currentTabs]
			};
		} catch (error) {
			// Remove failed explain tab
			const currentTabs = this.state.explainTabsByConnection[connectionId] ?? [];
			this.state.explainTabsByConnection = {
				...this.state.explainTabsByConnection,
				[connectionId]: currentTabs.filter((t) => t.id !== explainTabId)
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
			() => this.state.explainTabsByConnection,
			(r) => (this.state.explainTabsByConnection = r),
			() => this.state.activeExplainTabIdByConnection,
			(r) => (this.state.activeExplainTabIdByConnection = r),
			id
		);
		this.schedulePersistence(this.state.activeConnectionId);
		// Switch to query view if no explain tabs left
		if (this.state.activeConnectionId && this.state.explainTabs.length === 0) {
			this.setActiveView('query');
		}
	}

	/**
	 * Set the active explain tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeConnectionId) return;

		this.state.activeExplainTabIdByConnection = {
			...this.state.activeExplainTabIdByConnection,
			[this.state.activeConnectionId]: id
		};
		this.schedulePersistence(this.state.activeConnectionId);
	}
}
