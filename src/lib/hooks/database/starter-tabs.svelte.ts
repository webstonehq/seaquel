import type { StarterTab } from '$lib/types';
import { DEFAULT_STARTER_TABS } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';

/**
 * Manages starter tabs: shown when no connection is active.
 * Provides quick actions and migration guidance.
 * Tabs are organized per-project.
 */
export class StarterTabManager {
	constructor(
		private state: DatabaseState,
		private schedulePersistence: (projectId: string | null) => void
	) {}

	/**
	 * Initialize default starter tabs for a project if none exist.
	 */
	initializeDefaults(projectId: string): void {
		const existing = this.state.starterTabsByProject[projectId];
		if (existing && existing.length > 0) return;

		// Create default starter tabs
		const tabs: StarterTab[] = DEFAULT_STARTER_TABS.map((tab) => ({ ...tab }));

		this.state.starterTabsByProject = {
			...this.state.starterTabsByProject,
			[projectId]: tabs
		};

		// Set first tab as active
		if (tabs.length > 0) {
			this.state.activeStarterTabIdByProject = {
				...this.state.activeStarterTabIdByProject,
				[projectId]: tabs[0].id
			};
		}

		this.schedulePersistence(projectId);
	}

	/**
	 * Remove a starter tab by ID.
	 */
	remove(id: string): void {
		if (!this.state.activeProjectId) return;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.starterTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === id);

		if (!tab || !tab.closable) return;

		const tabIndex = tabs.findIndex((t) => t.id === id);
		const filteredTabs = tabs.filter((t) => t.id !== id);

		this.state.starterTabsByProject = {
			...this.state.starterTabsByProject,
			[projectId]: filteredTabs
		};

		// Update active tab if we removed the current one
		if (this.state.activeStarterTabIdByProject[projectId] === id) {
			// Try to select adjacent tab
			const newActiveIndex = Math.min(tabIndex, filteredTabs.length - 1);
			const newActiveId = filteredTabs[newActiveIndex]?.id ?? null;

			this.state.activeStarterTabIdByProject = {
				...this.state.activeStarterTabIdByProject,
				[projectId]: newActiveId
			};
		}

		this.schedulePersistence(projectId);
	}

	/**
	 * Set the active starter tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;

		this.state.activeStarterTabIdByProject = {
			...this.state.activeStarterTabIdByProject,
			[this.state.activeProjectId]: id
		};
		this.schedulePersistence(this.state.activeProjectId);
	}

	/**
	 * Reset to default starter tabs.
	 * Useful when user wants to restore after closing all tabs.
	 */
	reset(): void {
		if (!this.state.activeProjectId) return;

		const projectId = this.state.activeProjectId;
		const tabs: StarterTab[] = DEFAULT_STARTER_TABS.map((tab) => ({ ...tab }));

		this.state.starterTabsByProject = {
			...this.state.starterTabsByProject,
			[projectId]: tabs
		};

		if (tabs.length > 0) {
			this.state.activeStarterTabIdByProject = {
				...this.state.activeStarterTabIdByProject,
				[projectId]: tabs[0].id
			};
		}

		this.schedulePersistence(projectId);
	}

	/**
	 * Check if any starter tabs exist for the current project.
	 */
	hasStarterTabs(): boolean {
		return this.state.starterTabs.length > 0;
	}
}
