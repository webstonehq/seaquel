import type { CanvasTab } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { BaseTabManager, type TabStateAccessors } from './base-tab-manager.svelte.js';

/**
 * Manages canvas workspace tabs.
 * Tabs are organized per-project.
 */
export class CanvasTabManager extends BaseTabManager<CanvasTab> {
	private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'canvas') => void;

	constructor(
		state: DatabaseState,
		tabOrdering: TabOrderingManager,
		schedulePersistence: (projectId: string | null) => void,
		setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'canvas') => void
	) {
		super(state, tabOrdering, schedulePersistence);
		this.setActiveView = setActiveView;
	}

	protected get accessors(): TabStateAccessors<CanvasTab> {
		return {
			getTabs: () => this.state.canvasTabsByProject,
			setTabs: (r) => (this.state.canvasTabsByProject = r),
			getActiveId: () => this.state.activeCanvasTabIdByProject,
			setActiveId: (r) => (this.state.activeCanvasTabIdByProject = r)
		};
	}

	/**
	 * Add a canvas tab for the current connection.
	 * Returns the tab ID or null if no active project/connection.
	 */
	add(): string | null {
		if (!this.state.activeProjectId || !this.state.activeConnectionId || !this.state.activeConnection) return null;

		const tabs = this.getProjectTabs();

		// Check if a canvas tab already exists for this connection
		const existingTab = tabs.find((t) => t.connectionId === this.state.activeConnectionId);
		if (existingTab) {
			// Just switch to the existing tab
			this.setActiveTabId(existingTab.id);
			this.setActiveView('canvas');
			return existingTab.id;
		}

		const newCanvasTab: CanvasTab = {
			id: `canvas-${Date.now()}`,
			name: `Canvas: ${this.state.activeConnection.name}`,
			connectionId: this.state.activeConnectionId
		};

		this.appendTab(newCanvasTab);
		this.setActiveView('canvas');

		return newCanvasTab.id;
	}

	/**
	 * Remove a canvas tab by ID.
	 */
	override remove(id: string): void {
		super.remove(id);

		// If no more canvas tabs, switch back to query view
		const remainingTabs = this.state.canvasTabsByProject[this.state.activeProjectId!] ?? [];
		if (remainingTabs.length === 0) {
			this.setActiveView('query');
		}
	}
}
