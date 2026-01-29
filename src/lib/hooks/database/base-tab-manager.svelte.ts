import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';

/**
 * Configuration for how a tab manager maps to DatabaseState properties.
 * Each tab type has its own tabs-by-project and active-tab-id-by-project records.
 */
export interface TabStateAccessors<T extends { id: string }> {
	getTabs: () => Record<string, T[]>;
	setTabs: (r: Record<string, T[]>) => void;
	getActiveId: () => Record<string, string | null>;
	setActiveId: (r: Record<string, string | null>) => void;
}

/**
 * Base class for tab managers that share common CRUD lifecycle operations.
 *
 * Provides:
 * - remove(id) — removes a tab and updates active selection
 * - setActive(id) — sets the active tab
 * - helper methods for common state update patterns
 *
 * Subclasses provide their own add() and domain-specific methods.
 */
export abstract class BaseTabManager<T extends { id: string }> {
	constructor(
		protected state: DatabaseState,
		protected tabOrdering: TabOrderingManager,
		protected schedulePersistence: (projectId: string | null) => void
	) {}

	/**
	 * Subclasses must define how their tab data maps to DatabaseState.
	 */
	protected abstract get accessors(): TabStateAccessors<T>;

	/**
	 * Get tabs for the active project.
	 */
	protected getProjectTabs(): T[] {
		if (!this.state.activeProjectId) return [];
		return this.accessors.getTabs()[this.state.activeProjectId] ?? [];
	}

	/**
	 * Set tabs for the active project using spread syntax for reactivity.
	 */
	protected setProjectTabs(tabs: T[]): void {
		if (!this.state.activeProjectId) return;
		this.accessors.setTabs({
			...this.accessors.getTabs(),
			[this.state.activeProjectId]: tabs
		});
	}

	/**
	 * Set the active tab ID for the active project using spread syntax.
	 */
	protected setActiveTabId(id: string | null): void {
		if (!this.state.activeProjectId) return;
		this.accessors.setActiveId({
			...this.accessors.getActiveId(),
			[this.state.activeProjectId]: id
		});
	}

	/**
	 * Append a new tab, set it active, and register with tab ordering.
	 * Returns the tab ID.
	 */
	protected appendTab(tab: T): string {
		const tabs = this.getProjectTabs();
		this.setProjectTabs([...tabs, tab]);
		this.setActiveTabId(tab.id);
		this.tabOrdering.add(tab.id);
		this.schedulePersistence(this.state.activeProjectId);
		return tab.id;
	}

	/**
	 * Update a specific tab's properties by ID.
	 */
	protected updateTab(id: string, updater: (tab: T) => T): void {
		const tabs = this.getProjectTabs();
		this.setProjectTabs(tabs.map((t) => (t.id === id ? updater(t) : t)));
	}

	/**
	 * Remove a tab by ID. Delegates to TabOrderingManager for proper
	 * active tab selection after removal.
	 */
	remove(id: string): void {
		const { getTabs, setTabs, getActiveId, setActiveId } = this.accessors;
		this.tabOrdering.removeTabGeneric(getTabs, setTabs, getActiveId, setActiveId, id);
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Set the active tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;
		this.setActiveTabId(id);
		this.schedulePersistence(this.state.activeProjectId);
	}
}
