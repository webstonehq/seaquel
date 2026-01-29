import type { QueryTab, ExplainResult, ParsedQueryVisual } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { BaseTabManager, type TabStateAccessors } from './base-tab-manager.svelte.js';

/**
 * Manages query tabs: add, remove, rename, update content.
 * Tabs are organized per-project.
 */
export class QueryTabManager extends BaseTabManager<QueryTab> {
	constructor(
		state: DatabaseState,
		tabOrdering: TabOrderingManager,
		schedulePersistence: (projectId: string | null) => void
	) {
		super(state, tabOrdering, schedulePersistence);
	}

	protected get accessors(): TabStateAccessors<QueryTab> {
		return {
			getTabs: () => this.state.queryTabsByProject,
			setTabs: (r) => (this.state.queryTabsByProject = r),
			getActiveId: () => this.state.activeQueryTabIdByProject,
			setActiveId: (r) => (this.state.activeQueryTabIdByProject = r)
		};
	}

	/**
	 * Add a new query tab.
	 */
	add(name?: string, query?: string, savedQueryId?: string): string | null {
		if (!this.state.activeProjectId) return null;

		const tabs = this.getProjectTabs();
		const newTab: QueryTab = $state({
			id: `tab-${Date.now()}`,
			name: name || `Query ${tabs.length + 1}`,
			query: query || '',
			isExecuting: false,
			savedQueryId
		});

		return this.appendTab(newTab);
	}

	/**
	 * Rename a query tab.
	 */
	rename(id: string, newName: string): void {
		if (!this.state.activeProjectId) return;

		const tabs = this.getProjectTabs();
		const tab = tabs.find((t) => t.id === id);
		if (tab) {
			this.updateTab(id, (t) => ({ ...t, name: newName }));

			// Also update linked saved query name if exists (saved queries are per-connection)
			if (tab.savedQueryId && this.state.activeConnectionId) {
				const connectionId = this.state.activeConnectionId;
				const savedQueries = this.state.savedQueriesByConnection[connectionId] ?? [];
				const updatedSavedQueries = savedQueries.map((q) =>
					q.id === tab.savedQueryId ? { ...q, name: newName, updatedAt: new Date() } : q
				);
				this.state.savedQueriesByConnection = {
					...this.state.savedQueriesByConnection,
					[connectionId]: updatedSavedQueries
				};
			}

			this.schedulePersistence(this.state.activeProjectId);
		}
	}

	/**
	 * Check if a tab has unsaved changes.
	 */
	hasUnsavedChanges(tabId: string): boolean {
		const tab = this.state.queryTabs.find((t) => t.id === tabId);
		if (!tab) return false;

		// Empty tabs are not considered "unsaved"
		if (!tab.query.trim()) return false;

		// Tab linked to saved query - compare content
		if (tab.savedQueryId) {
			const savedQuery = this.state.activeConnectionSavedQueries.find(
				(q) => q.id === tab.savedQueryId
			);
			if (!savedQuery) return true;
			return tab.query !== savedQuery.query;
		}

		// Tab linked to shared query - compare content
		if (tab.sharedQueryId) {
			const sharedQuery = this.state.allSharedQueries.find(
				(q) => q.id === tab.sharedQueryId
			);
			if (!sharedQuery) return true;
			return tab.query !== sharedQuery.query;
		}

		// Tab not linked to any saved/shared query = unsaved
		return true;
	}

	/**
	 * Update the query content in a tab.
	 */
	updateContent(id: string, query: string): void {
		if (!this.state.activeProjectId) return;

		const tabs = this.getProjectTabs();
		const tab = tabs.find((t) => t.id === id);
		if (tab && tab.query !== query) {
			this.updateTab(id, (t) => ({ ...t, query }));
			this.schedulePersistence(this.state.activeProjectId);
		}
	}

	/**
	 * Find a query tab by its query content and focus it, or create a new one if not found.
	 * Returns the tab ID.
	 */
	focusOrCreate(query: string, name?: string, setActiveView?: () => void): string | null {
		if (!this.state.activeProjectId) return null;

		const tabs = this.getProjectTabs();
		const existingTab = tabs.find((t) => t.query.trim() === query.trim());

		if (existingTab) {
			this.setActive(existingTab.id);
			setActiveView?.();
			return existingTab.id;
		}

		// Create new tab if not found
		const newTabId = this.add(name, query);
		setActiveView?.();
		return newTabId;
	}

	/**
	 * Load a saved query into a tab (or switch to existing tab).
	 * Note: Saved queries are per-connection, so we need an active connection.
	 */
	loadSaved(savedQueryId: string, setActiveView?: () => void): void {
		if (!this.state.activeProjectId || !this.state.activeConnectionId) return;

		const savedQueries = this.state.savedQueriesByConnection[this.state.activeConnectionId] ?? [];
		const savedQuery = savedQueries.find((q) => q.id === savedQueryId);
		if (!savedQuery) return;

		// Check if a tab with this saved query is already open
		const tabs = this.getProjectTabs();
		const existingTab = tabs.find((t) => t.savedQueryId === savedQueryId);

		if (existingTab) {
			// Switch to existing tab
			this.setActive(existingTab.id);
			setActiveView?.();
		} else {
			// Create new tab
			this.add(savedQuery.name, savedQuery.query, savedQueryId);
			setActiveView?.();
		}
	}

	/**
	 * Load a shared query into a tab (or switch to existing tab).
	 */
	loadSharedQuery(
		sharedQueryId: string,
		name: string,
		query: string,
		setActiveView?: () => void
	): void {
		if (!this.state.activeProjectId) return;

		// Check if a tab with this shared query is already open
		const tabs = this.getProjectTabs();
		const existingTab = tabs.find((t) => t.sharedQueryId === sharedQueryId);

		if (existingTab) {
			// Switch to existing tab
			this.setActive(existingTab.id);
			setActiveView?.();
		} else {
			// Create new tab with sharedQueryId
			const newTab: QueryTab = $state({
				id: `tab-${Date.now()}`,
				name,
				query,
				isExecuting: false,
				sharedQueryId
			});

			this.appendTab(newTab);
			setActiveView?.();
		}
	}

	/**
	 * Load a query from history into a tab (or switch to existing tab).
	 * Note: Query history is per-connection, so we need an active connection.
	 */
	loadFromHistory(historyId: string, setActiveView?: () => void): void {
		if (!this.state.activeProjectId || !this.state.activeConnectionId) return;

		const queryHistory = this.state.queryHistoryByConnection[this.state.activeConnectionId] ?? [];
		const item = queryHistory.find((h) => h.id === historyId);
		if (!item) return;

		// Check if a tab with the exact same query is already open
		const tabs = this.getProjectTabs();
		const existingTab = tabs.find((t) => t.query.trim() === item.query.trim());

		if (existingTab) {
			// Switch to existing tab
			this.setActive(existingTab.id);
			setActiveView?.();
		} else {
			// Create new tab
			this.add(`History: ${item.query.substring(0, 20)}...`, item.query);
			setActiveView?.();
		}
	}

	/**
	 * Set the explain result on a query tab.
	 */
	setExplainResult(tabId: string, result: ExplainResult, sourceQuery: string, isAnalyze: boolean): void {
		if (!this.state.activeProjectId) return;

		this.updateTab(tabId, (t) => ({
			...t,
			explainResult: { result, sourceQuery, isAnalyze, isExecuting: false }
		}));
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Set the explain executing state on a query tab.
	 */
	setExplainExecuting(tabId: string, isExecuting: boolean, isAnalyze: boolean = false): void {
		if (!this.state.activeProjectId) return;

		this.updateTab(tabId, (t) => ({
			...t,
			explainResult: t.explainResult
				? { ...t.explainResult, isExecuting }
				: { result: undefined as unknown as ExplainResult, sourceQuery: '', isAnalyze, isExecuting }
		}));
	}

	/**
	 * Clear the explain result from a query tab.
	 */
	clearExplainResult(tabId: string): void {
		if (!this.state.activeProjectId) return;

		this.updateTab(tabId, (t) => ({ ...t, explainResult: undefined }));
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Set the visualize result on a query tab.
	 */
	setVisualizeResult(
		tabId: string,
		parsedQuery: ParsedQueryVisual | null,
		sourceQuery: string,
		parseError?: string
	): void {
		if (!this.state.activeProjectId) return;

		this.updateTab(tabId, (t) => ({
			...t,
			visualizeResult: { parsedQuery, sourceQuery, parseError }
		}));
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Clear the visualize result from a query tab.
	 */
	clearVisualizeResult(tabId: string): void {
		if (!this.state.activeProjectId) return;

		this.updateTab(tabId, (t) => ({ ...t, visualizeResult: undefined }));
		this.schedulePersistence(this.state.activeProjectId);
	}
}
