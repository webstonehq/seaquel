import type { QueryTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';

/**
 * Manages query tabs: add, remove, rename, update content.
 * Tabs are organized per-project.
 */
export class QueryTabManager {
	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (projectId: string | null) => void
	) {}

	/**
	 * Add a new query tab.
	 */
	add(name?: string, query?: string, savedQueryId?: string): string | null {
		if (!this.state.activeProjectId) return null;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const newTab: QueryTab = $state({
			id: `tab-${Date.now()}`,
			name: name || `Query ${tabs.length + 1}`,
			query: query || '',
			isExecuting: false,
			savedQueryId
		});

		// Update tabs using spread syntax
		this.state.queryTabsByProject = {
			...this.state.queryTabsByProject,
			[projectId]: [...tabs, newTab]
		};

		this.state.activeQueryTabIdByProject = {
			...this.state.activeQueryTabIdByProject,
			[projectId]: newTab.id
		};

		this.tabOrdering.add(newTab.id);
		this.schedulePersistence(projectId);
		return newTab.id;
	}

	/**
	 * Remove a query tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.queryTabsByProject,
			(r) => (this.state.queryTabsByProject = r),
			() => this.state.activeQueryTabIdByProject,
			(r) => (this.state.activeQueryTabIdByProject = r),
			id
		);
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Rename a query tab.
	 */
	rename(id: string, newName: string): void {
		if (!this.state.activeProjectId) return;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === id);
		if (tab) {
			// Create new array with updated tab object for proper reactivity
			const updatedTabs = tabs.map((t) => (t.id === id ? { ...t, name: newName } : t));
			this.state.queryTabsByProject = {
				...this.state.queryTabsByProject,
				[projectId]: updatedTabs
			};

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

			this.schedulePersistence(projectId);
		}
	}

	/**
	 * Set the active query tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;

		this.state.activeQueryTabIdByProject = {
			...this.state.activeQueryTabIdByProject,
			[this.state.activeProjectId]: id
		};
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Check if a tab has unsaved changes.
	 */
	hasUnsavedChanges(tabId: string): boolean {
		const tab = this.state.queryTabs.find((t) => t.id === tabId);
		if (!tab) return false;

		// Empty tabs are not considered "unsaved"
		if (!tab.query.trim()) return false;

		// Tab not linked to a saved query = unsaved
		if (!tab.savedQueryId) return true;

		// Tab linked to saved query - compare content
		const savedQuery = this.state.activeConnectionSavedQueries.find(
			(q) => q.id === tab.savedQueryId
		);
		if (!savedQuery) return true;

		return tab.query !== savedQuery.query;
	}

	/**
	 * Update the query content in a tab.
	 */
	updateContent(id: string, query: string): void {
		if (!this.state.activeProjectId) return;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === id);
		if (tab && tab.query !== query) {
			// Create new objects for proper reactivity
			const updatedTabs = tabs.map((t) => (t.id === id ? { ...t, query } : t));
			this.state.queryTabsByProject = {
				...this.state.queryTabsByProject,
				[projectId]: updatedTabs
			};
			this.schedulePersistence(projectId);
		}
	}

	/**
	 * Find a query tab by its query content and focus it, or create a new one if not found.
	 * Returns the tab ID.
	 */
	focusOrCreate(query: string, name?: string, setActiveView?: () => void): string | null {
		if (!this.state.activeProjectId) return null;

		const tabs = this.state.queryTabsByProject[this.state.activeProjectId] ?? [];
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
		const tabs = this.state.queryTabsByProject[this.state.activeProjectId] ?? [];
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
	 * Load a query from history into a tab (or switch to existing tab).
	 * Note: Query history is per-connection, so we need an active connection.
	 */
	loadFromHistory(historyId: string, setActiveView?: () => void): void {
		if (!this.state.activeProjectId || !this.state.activeConnectionId) return;

		const queryHistory = this.state.queryHistoryByConnection[this.state.activeConnectionId] ?? [];
		const item = queryHistory.find((h) => h.id === historyId);
		if (!item) return;

		// Check if a tab with the exact same query is already open
		const tabs = this.state.queryTabsByProject[this.state.activeProjectId] ?? [];
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
}
