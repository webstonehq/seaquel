import type { CanvasTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';

/**
 * Manages canvas workspace tabs.
 * Tabs are organized per-project.
 */
export class CanvasTabManager {
	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (projectId: string | null) => void,
		private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'canvas') => void
	) {}

	/**
	 * Add a canvas tab for the current connection.
	 * Returns the tab ID or null if no active project/connection.
	 */
	add(): string | null {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return null;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.canvasTabsByProject[projectId] ?? [];

		// Check if a canvas tab already exists for this connection
		const existingTab = tabs.find((t) => t.connectionId === this.state.activeConnectionId);
		if (existingTab) {
			// Just switch to the existing tab
			this.state.activeCanvasTabIdByProject = {
				...this.state.activeCanvasTabIdByProject,
				[projectId]: existingTab.id
			};
			this.setActiveView('canvas');
			return existingTab.id;
		}

		const canvasTabId = `canvas-${Date.now()}`;
		const newCanvasTab: CanvasTab = {
			id: canvasTabId,
			name: `Canvas: ${this.state.activeConnection.name}`,
			connectionId: this.state.activeConnectionId
		};

		this.state.canvasTabsByProject = {
			...this.state.canvasTabsByProject,
			[projectId]: [...tabs, newCanvasTab]
		};

		this.tabOrdering.add(canvasTabId);

		this.state.activeCanvasTabIdByProject = {
			...this.state.activeCanvasTabIdByProject,
			[projectId]: canvasTabId
		};

		this.setActiveView('canvas');
		this.schedulePersistence(projectId);

		return canvasTabId;
	}

	/**
	 * Remove a canvas tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.canvasTabsByProject,
			(r) => (this.state.canvasTabsByProject = r),
			() => this.state.activeCanvasTabIdByProject,
			(r) => (this.state.activeCanvasTabIdByProject = r),
			id
		);
		this.schedulePersistence(this.state.activeProjectId);

		// If no more canvas tabs, switch back to query view
		const remainingTabs = this.state.canvasTabsByProject[this.state.activeProjectId!] ?? [];
		if (remainingTabs.length === 0) {
			this.setActiveView('query');
		}
	}

	/**
	 * Set the active canvas tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;
		this.state.activeCanvasTabIdByProject = {
			...this.state.activeCanvasTabIdByProject,
			[this.state.activeProjectId]: id
		};
		this.schedulePersistence(this.state.activeProjectId);
	}
}
