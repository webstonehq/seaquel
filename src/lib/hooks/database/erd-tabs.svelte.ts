import type { ErdTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';

/**
 * Manages ERD (Entity Relationship Diagram) tabs.
 * Tabs are organized per-project.
 */
export class ErdTabManager {
	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (projectId: string | null) => void,
		private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd') => void
	) {}

	/**
	 * Add an ERD tab for the current connection.
	 * Returns the tab ID or null if no active project/connection.
	 */
	add(): string | null {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return null;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.erdTabsByProject[projectId] ?? [];

		// Check if an ERD tab already exists for this connection
		const existingTab = tabs.find((t) => t.name === `ERD: ${this.state.activeConnection!.name}`);
		if (existingTab) {
			// Just switch to the existing tab
			this.state.activeErdTabIdByProject = {
				...this.state.activeErdTabIdByProject,
				[projectId]: existingTab.id
			};
			this.setActiveView('erd');
			return existingTab.id;
		}

		const erdTabId = `erd-${Date.now()}`;
		const newErdTab: ErdTab = {
			id: erdTabId,
			name: `ERD: ${this.state.activeConnection.name}`,
			connectionId: this.state.activeConnectionId
		};

		this.state.erdTabsByProject = {
			...this.state.erdTabsByProject,
			[projectId]: [...tabs, newErdTab]
		};

		this.tabOrdering.add(erdTabId);

		this.state.activeErdTabIdByProject = {
			...this.state.activeErdTabIdByProject,
			[projectId]: erdTabId
		};

		this.setActiveView('erd');
		this.schedulePersistence(projectId);

		return erdTabId;
	}

	/**
	 * Remove an ERD tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.erdTabsByProject,
			(r) => (this.state.erdTabsByProject = r),
			() => this.state.activeErdTabIdByProject,
			(r) => (this.state.activeErdTabIdByProject = r),
			id
		);
		this.schedulePersistence(this.state.activeProjectId);

		// If no more ERD tabs, switch back to query view
		const remainingTabs = this.state.erdTabsByProject[this.state.activeProjectId!] ?? [];
		if (remainingTabs.length === 0) {
			this.setActiveView('query');
		}
	}

	/**
	 * Set the active ERD tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;
		this.state.activeErdTabIdByProject = {
			...this.state.activeErdTabIdByProject,
			[this.state.activeProjectId]: id
		};
		this.schedulePersistence(this.state.activeProjectId);
	}
}
