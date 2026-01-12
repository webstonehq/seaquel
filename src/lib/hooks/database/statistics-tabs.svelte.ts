import type { StatisticsTab, DatabaseStatistics } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { getAdapter } from '$lib/db/index.js';

/**
 * Manages Statistics dashboard tabs.
 * Tabs are organized per-project.
 */
export class StatisticsTabManager {
	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (projectId: string | null) => void,
		private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics') => void,
		private executeQuery: (query: string) => Promise<Record<string, unknown>[]>
	) {}

	/**
	 * Add a Statistics tab for the current connection.
	 * Returns the tab ID or null if no active project/connection.
	 */
	add(): string | null {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return null;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.statisticsTabsByProject[projectId] ?? [];

		// Check if a Statistics tab already exists for this connection
		const existingTab = tabs.find((t) => t.connectionId === this.state.activeConnectionId);
		if (existingTab) {
			// Just switch to the existing tab
			this.state.activeStatisticsTabIdByProject = {
				...this.state.activeStatisticsTabIdByProject,
				[projectId]: existingTab.id
			};
			this.setActiveView('statistics');
			// Refresh the data
			this.refresh(existingTab.id);
			return existingTab.id;
		}

		const tabId = `stats-${Date.now()}`;
		const newTab: StatisticsTab = {
			id: tabId,
			name: `Stats: ${this.state.activeConnection.name}`,
			connectionId: this.state.activeConnectionId,
			isLoading: true
		};

		this.state.statisticsTabsByProject = {
			...this.state.statisticsTabsByProject,
			[projectId]: [...tabs, newTab]
		};

		this.tabOrdering.add(tabId);

		this.state.activeStatisticsTabIdByProject = {
			...this.state.activeStatisticsTabIdByProject,
			[projectId]: tabId
		};

		this.setActiveView('statistics');
		this.schedulePersistence(projectId);

		// Load the statistics data
		this.loadStatistics(tabId);

		return tabId;
	}

	/**
	 * Remove a Statistics tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.statisticsTabsByProject,
			(r) => (this.state.statisticsTabsByProject = r),
			() => this.state.activeStatisticsTabIdByProject,
			(r) => (this.state.activeStatisticsTabIdByProject = r),
			id
		);
		this.schedulePersistence(this.state.activeProjectId);

		// If no more statistics tabs, switch back to query view
		const remainingTabs = this.state.statisticsTabsByProject[this.state.activeProjectId!] ?? [];
		if (remainingTabs.length === 0) {
			this.setActiveView('query');
		}
	}

	/**
	 * Set the active Statistics tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;
		this.state.activeStatisticsTabIdByProject = {
			...this.state.activeStatisticsTabIdByProject,
			[this.state.activeProjectId]: id
		};
		this.schedulePersistence(this.state.activeProjectId);
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

		const tabs = this.state.statisticsTabsByProject[projectId] ?? [];
		const tabIndex = tabs.findIndex((t) => t.id === tabId);
		if (tabIndex === -1) return;

		const tab = tabs[tabIndex];
		const connection = this.state.connections.find((c) => c.id === tab.connectionId);
		if (!connection) return;

		// Set loading state
		this.updateTab(projectId, tabId, { isLoading: true, error: undefined });

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

			this.updateTab(projectId, tabId, {
				data: statistics,
				isLoading: false,
				lastRefreshed: new Date()
			});
		} catch (error) {
			this.updateTab(projectId, tabId, {
				isLoading: false,
				error: error instanceof Error ? error.message : 'Failed to load statistics'
			});
		}
	}

	/**
	 * Update a specific tab's properties.
	 */
	private updateTab(projectId: string, tabId: string, updates: Partial<StatisticsTab>): void {
		const tabs = this.state.statisticsTabsByProject[projectId] ?? [];
		const tabIndex = tabs.findIndex((t) => t.id === tabId);
		if (tabIndex === -1) return;

		const updatedTabs = [...tabs];
		updatedTabs[tabIndex] = { ...updatedTabs[tabIndex], ...updates };

		this.state.statisticsTabsByProject = {
			...this.state.statisticsTabsByProject,
			[projectId]: updatedTabs
		};
	}
}
