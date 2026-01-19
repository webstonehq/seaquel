import type { QueryTab, SchemaTab, ExplainTab, ErdTab, StatisticsTab, CanvasTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';

/**
 * Manages tab ordering across all tab types (query, schema, explain, ERD).
 * Tabs are now organized per-project instead of per-connection.
 * Provides generic tab removal logic and ordered tab computation.
 */
export class TabOrderingManager {
	constructor(
		private state: DatabaseState,
		private schedulePersistence: (projectId: string | null) => void
	) {}

	/**
	 * Generic tab removal helper used by all tab managers.
	 * Handles removing from tab list and updating active tab selection.
	 */
	removeTabGeneric<T extends { id: string }>(
		tabsGetter: () => Record<string, T[]>,
		tabsSetter: (r: Record<string, T[]>) => void,
		activeIdGetter: () => Record<string, string | null>,
		activeIdSetter: (r: Record<string, string | null>) => void,
		tabId: string
	): void {
		if (!this.state.activeProjectId) return;

		const projectId = this.state.activeProjectId;
		const tabs = tabsGetter()[projectId] ?? [];
		const index = tabs.findIndex((t) => t.id === tabId);
		const newTabs = tabs.filter((t) => t.id !== tabId);

		// Update tabs using spread syntax
		tabsSetter({ ...tabsGetter(), [projectId]: newTabs });

		// Remove from tab order
		this.removeFromTabOrder(tabId);

		const currentActiveId = activeIdGetter()[projectId];
		if (currentActiveId === tabId) {
			let newActiveId: string | null = null;
			if (newTabs.length > 0) {
				const newIndex = Math.min(index, newTabs.length - 1);
				newActiveId = newTabs[newIndex]?.id || null;
			}
			activeIdSetter({ ...activeIdGetter(), [projectId]: newActiveId });
		}
	}

	/**
	 * Add a tab ID to the ordering array.
	 */
	add(tabId: string): void {
		if (!this.state.activeProjectId) return;
		const projectId = this.state.activeProjectId;
		const order = this.state.tabOrderByProject[projectId] ?? [];
		if (!order.includes(tabId)) {
			this.state.tabOrderByProject = {
				...this.state.tabOrderByProject,
				[projectId]: [...order, tabId]
			};
		}
	}

	/**
	 * Remove a tab ID from the ordering array.
	 */
	removeFromTabOrder(tabId: string): void {
		if (!this.state.activeProjectId) return;
		const projectId = this.state.activeProjectId;
		const order = this.state.tabOrderByProject[projectId] ?? [];
		this.state.tabOrderByProject = {
			...this.state.tabOrderByProject,
			[projectId]: order.filter((id: string) => id !== tabId)
		};
	}

	/**
	 * Reorder tabs to match the provided order array.
	 */
	reorder(newOrder: string[]): void {
		if (!this.state.activeProjectId) return;
		this.state.tabOrderByProject = {
			...this.state.tabOrderByProject,
			[this.state.activeProjectId]: newOrder
		};
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Extract timestamp from tab ID for default ordering.
	 */
	private getTabTimestamp(id: string): number {
		const match = id.match(/\d+$/);
		return match ? parseInt(match[0], 10) : 0;
	}

	/**
	 * Get all tabs ordered by user preference or creation time.
	 */
	get ordered(): Array<{
		id: string;
		type: 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'canvas';
		tab: QueryTab | SchemaTab | ExplainTab | ErdTab | StatisticsTab | CanvasTab;
	}> {
		if (!this.state.activeProjectId) return [];

		// Ensure we have arrays (defensive against undefined)
		const queryTabs = this.state.queryTabs || [];
		const schemaTabs = this.state.schemaTabs || [];
		const explainTabs = this.state.explainTabs || [];
		const erdTabs = this.state.erdTabs || [];
		const statisticsTabs = this.state.statisticsTabs || [];
		const canvasTabs = this.state.canvasTabs || [];

		const allTabsUnordered: Array<{
			id: string;
			type: 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'canvas';
			tab: QueryTab | SchemaTab | ExplainTab | ErdTab | StatisticsTab | CanvasTab;
		}> = [];

		for (const t of queryTabs) {
			allTabsUnordered.push({ id: t.id, type: 'query', tab: t });
		}
		for (const t of schemaTabs) {
			allTabsUnordered.push({ id: t.id, type: 'schema', tab: t });
		}
		for (const t of explainTabs) {
			allTabsUnordered.push({ id: t.id, type: 'explain', tab: t });
		}
		for (const t of erdTabs) {
			allTabsUnordered.push({ id: t.id, type: 'erd', tab: t });
		}
		for (const t of statisticsTabs) {
			allTabsUnordered.push({ id: t.id, type: 'statistics', tab: t });
		}
		for (const t of canvasTabs) {
			allTabsUnordered.push({ id: t.id, type: 'canvas', tab: t });
		}

		const order = this.state.tabOrderByProject[this.state.activeProjectId] ?? [];

		// Sort by order array, falling back to timestamp for new tabs
		return allTabsUnordered.sort((a, b) => {
			const aIndex = order.indexOf(a.id);
			const bIndex = order.indexOf(b.id);

			// Both in order array: use order
			if (aIndex !== -1 && bIndex !== -1) return aIndex - bIndex;

			// Only one in order: ordered comes first
			if (aIndex !== -1) return -1;
			if (bIndex !== -1) return 1;

			// Neither in order: fall back to timestamp
			return this.getTabTimestamp(a.id) - this.getTabTimestamp(b.id);
		});
	}
}
