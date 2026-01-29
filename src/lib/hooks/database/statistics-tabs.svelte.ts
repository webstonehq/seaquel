import type { StatisticsTab, DatabaseStatistics } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { BaseTabManager, type TabStateAccessors } from './base-tab-manager.svelte.js';
import { getAdapter } from '$lib/db/index.js';

/**
 * Manages Statistics dashboard tabs.
 * Tabs are organized per-project.
 */
export class StatisticsTabManager extends BaseTabManager<StatisticsTab> {
	private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics') => void;
	private executeQuery: (query: string) => Promise<Record<string, unknown>[]>;

	constructor(
		state: DatabaseState,
		tabOrdering: TabOrderingManager,
		schedulePersistence: (projectId: string | null) => void,
		setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics') => void,
		executeQuery: (query: string) => Promise<Record<string, unknown>[]>
	) {
		super(state, tabOrdering, schedulePersistence);
		this.setActiveView = setActiveView;
		this.executeQuery = executeQuery;
	}

	protected get accessors(): TabStateAccessors<StatisticsTab> {
		return {
			getTabs: () => this.state.statisticsTabsByProject,
			setTabs: (r) => (this.state.statisticsTabsByProject = r),
			getActiveId: () => this.state.activeStatisticsTabIdByProject,
			setActiveId: (r) => (this.state.activeStatisticsTabIdByProject = r)
		};
	}

	/**
	 * Add a Statistics tab for the current connection.
	 * Returns the tab ID or null if no active project/connection.
	 */
	add(): string | null {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return null;

		const projectId = this.state.activeProjectId;
		const tabs = this.getProjectTabs();

		// Check if a Statistics tab already exists for this connection
		const existingTab = tabs.find((t) => t.connectionId === this.state.activeConnectionId);
		if (existingTab) {
			// Just switch to the existing tab
			this.setActiveTabId(existingTab.id);
			this.setActiveView('statistics');
			// Refresh the data
			this.refresh(existingTab.id);
			return existingTab.id;
		}

		const newTab: StatisticsTab = {
			id: `stats-${Date.now()}`,
			name: `Stats: ${this.state.activeConnection.name}`,
			connectionId: this.state.activeConnectionId,
			isLoading: true
		};

		this.appendTab(newTab);
		this.setActiveView('statistics');

		// Load the statistics data
		this.loadStatistics(newTab.id);

		return newTab.id;
	}

	/**
	 * Remove a Statistics tab by ID.
	 */
	override remove(id: string): void {
		super.remove(id);

		// If no more statistics tabs, switch back to query view
		const remainingTabs = this.state.statisticsTabsByProject[this.state.activeProjectId!] ?? [];
		if (remainingTabs.length === 0) {
			this.setActiveView('query');
		}
	}

	/**
	 * Refresh statistics data for a tab.
	 */
	async refresh(tabId: string): Promise<void> {
		await this.loadStatistics(tabId);
	}

	/**
	 * Load statistics data for a tab.
	 */
	private async loadStatistics(tabId: string): Promise<void> {
		const projectId = this.state.activeProjectId;
		if (!projectId) return;

		const tabs = this.getProjectTabs();
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab) return;

		const connection = this.state.connections.find((c) => c.id === tab.connectionId);
		if (!connection) return;

		// Set loading state
		this.updateTab(tabId, (t) => ({ ...t, isLoading: true, error: undefined }));

		try {
			const adapter = getAdapter(connection.type);

			// Execute all statistics queries in parallel
			const [tableSizesRows, indexUsageRows, overviewRows] = await Promise.all([
				adapter.getTableSizesQuery?.() ? this.executeQuery(adapter.getTableSizesQuery!()) : Promise.resolve([]),
				adapter.getIndexUsageQuery?.() ? this.executeQuery(adapter.getIndexUsageQuery!()) : Promise.resolve([]),
				adapter.getDatabaseOverviewQuery?.() ? this.executeQuery(adapter.getDatabaseOverviewQuery!()) : Promise.resolve([])
			]);

			let tableSizes = adapter.parseTableSizesResult?.(tableSizesRows) ?? [];

			// For databases that need per-table row count queries (like SQLite),
			// fetch row counts separately
			if (adapter.getTableRowCountQuery && tableSizes.length > 0) {
				const rowCountPromises = tableSizes.map(async (table) => {
					try {
						const query = adapter.getTableRowCountQuery!(table.name, table.schema);
						const result = await this.executeQuery(query);
						const rowCount = Number((result[0] as { row_count?: number })?.row_count) || 0;
						return { ...table, rowCount };
					} catch {
						return table; // Keep original if query fails
					}
				});
				tableSizes = await Promise.all(rowCountPromises);
			}

			const statistics: DatabaseStatistics = {
				overview: adapter.parseDatabaseOverviewResult?.(overviewRows) ?? {
					databaseName: connection.name,
					totalSize: 'N/A',
					tableCount: 0,
					indexCount: 0
				},
				tableSizes,
				indexUsage: adapter.parseIndexUsageResult?.(indexUsageRows) ?? []
			};

			this.updateTab(tabId, (t) => ({
				...t,
				data: statistics,
				isLoading: false,
				lastRefreshed: new Date()
			}));
		} catch (error) {
			this.updateTab(tabId, (t) => ({
				...t,
				isLoading: false,
				error: error instanceof Error ? error.message : 'Failed to load statistics'
			}));
		}
	}
}
