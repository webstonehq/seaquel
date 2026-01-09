import type { ErdTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';

/**
 * Manages ERD (Entity Relationship Diagram) tabs.
 */
export class ErdTabManager {
	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (connectionId: string | null) => void,
		private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd') => void
	) {}

	/**
	 * Add an ERD tab for the current connection.
	 * Returns the tab ID or null if no active connection.
	 */
	add(): string | null {
		if (!this.state.activeConnectionId || !this.state.activeConnection) return null;

		const connectionId = this.state.activeConnectionId;
		const tabs = this.state.erdTabsByConnection[connectionId] ?? [];

		// Check if an ERD tab already exists for this connection
		const existingTab = tabs.find((t) => t.name === `ERD: ${this.state.activeConnection!.name}`);
		if (existingTab) {
			// Just switch to the existing tab
			this.state.activeErdTabIdByConnection = {
				...this.state.activeErdTabIdByConnection,
				[connectionId]: existingTab.id
			};
			this.setActiveView('erd');
			return existingTab.id;
		}

		const erdTabId = `erd-${Date.now()}`;
		const newErdTab: ErdTab = {
			id: erdTabId,
			name: `ERD: ${this.state.activeConnection.name}`
		};

		this.state.erdTabsByConnection = {
			...this.state.erdTabsByConnection,
			[connectionId]: [...tabs, newErdTab]
		};

		this.tabOrdering.add(erdTabId);

		this.state.activeErdTabIdByConnection = {
			...this.state.activeErdTabIdByConnection,
			[connectionId]: erdTabId
		};

		this.setActiveView('erd');
		this.schedulePersistence(connectionId);

		return erdTabId;
	}

	/**
	 * Remove an ERD tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.erdTabsByConnection,
			(r) => (this.state.erdTabsByConnection = r),
			() => this.state.activeErdTabIdByConnection,
			(r) => (this.state.activeErdTabIdByConnection = r),
			id
		);
		this.schedulePersistence(this.state.activeConnectionId);

		// If no more ERD tabs, switch back to query view
		const remainingTabs = this.state.erdTabsByConnection[this.state.activeConnectionId!] ?? [];
		if (remainingTabs.length === 0) {
			this.setActiveView('query');
		}
	}

	/**
	 * Set the active ERD tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeConnectionId) return;
		this.state.activeErdTabIdByConnection = {
			...this.state.activeErdTabIdByConnection,
			[this.state.activeConnectionId]: id
		};
		this.schedulePersistence(this.state.activeConnectionId);
	}
}
