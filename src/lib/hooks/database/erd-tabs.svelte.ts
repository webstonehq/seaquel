import type { ErdTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { BaseTabManager, type TabStateAccessors } from './base-tab-manager.svelte.js';

/**
 * Manages ERD (Entity Relationship Diagram) tabs.
 * Tabs are organized per-project.
 */
export class ErdTabManager extends BaseTabManager<ErdTab> {
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

	protected get accessors(): TabStateAccessors<ErdTab> {
		return {
			getTabs: () => this.state.erdTabsByProject,
			setTabs: (r) => (this.state.erdTabsByProject = r),
			getActiveId: () => this.state.activeErdTabIdByProject,
			setActiveId: (r) => (this.state.activeErdTabIdByProject = r)
		};
	}

	/**
	 * Add an ERD tab for the current connection.
	 * Returns the tab ID or null if no active project/connection.
	 */
	add(): string | null {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return null;

		const projectId = this.state.activeProjectId;
		const tabs = this.getProjectTabs();

		// Check if an ERD tab already exists for this connection
		const existingTab = tabs.find((t) => t.name === `ERD: ${this.state.activeConnection!.name}`);
		if (existingTab) {
			// Just switch to the existing tab
			this.setActiveTabId(existingTab.id);
			this.setActiveView('erd');
			return existingTab.id;
		}

		const newErdTab: ErdTab = {
			id: `erd-${Date.now()}`,
			name: `ERD: ${this.state.activeConnection.name}`,
			connectionId: this.state.activeConnectionId
		};

		this.appendTab(newErdTab);
		this.setActiveView('erd');

		return newErdTab.id;
	}

	/**
	 * Remove an ERD tab by ID.
	 */
	override remove(id: string): void {
		super.remove(id);

		// If no more ERD tabs, switch back to query view
		const remainingTabs = this.state.erdTabsByProject[this.state.activeProjectId!] ?? [];
		if (remainingTabs.length === 0) {
			this.setActiveView('query');
		}
	}
}
