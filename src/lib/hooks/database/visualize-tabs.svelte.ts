import { toast } from 'svelte-sonner';
import type { VisualizeTab, ParsedQueryVisual } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { parseQueryForVisualization, getParseError } from '$lib/db/sql-ast-parser';
import { getStatementAtOffset } from '$lib/db/sql-parser';

/**
 * Callback for setting visualize result on a query tab.
 */
export type SetVisualizeResultCallback = (
	tabId: string,
	parsedQuery: ParsedQueryVisual | null,
	sourceQuery: string,
	parseError?: string
) => void;

/**
 * Manages query visualizer tabs: parse, visualize, remove, set active.
 * Tabs are organized per-project.
 */
export class VisualizeTabManager {
	private setVisualizeResult?: SetVisualizeResultCallback;

	constructor(
		private state: DatabaseState,
		private tabOrdering: TabOrderingManager,
		private schedulePersistence: (projectId: string | null) => void,
		private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'canvas' | 'visualize') => void
	) {}

	/**
	 * Set callback for embedded visualize results (stored on QueryTab).
	 */
	setEmbeddedCallback(setResult: SetVisualizeResultCallback): void {
		this.setVisualizeResult = setResult;
	}

	/**
	 * Visualize a query and store result embedded in the query tab.
	 * This is the new approach where results appear below the editor instead of in a separate tab.
	 */
	visualizeEmbedded(tabId: string, cursorOffset?: number): boolean {
		if (!this.state.activeProjectId || !this.state.activeConnection) return false;
		if (!this.setVisualizeResult) {
			console.warn('Embedded callback not set, falling back to tab-based visualize');
			this.visualize(tabId, cursorOffset);
			return true;
		}

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab || !tab.query.trim()) {
			toast.error('No query to visualize');
			return false;
		}

		// Get the statement to visualize based on cursor position
		const dbType = this.state.activeConnection.type;
		let queryToVisualize = tab.query;

		if (cursorOffset !== undefined) {
			const statement = getStatementAtOffset(tab.query, cursorOffset, dbType);
			if (statement) {
				queryToVisualize = statement.sql;
			}
		}

		if (!queryToVisualize.trim()) {
			toast.error('No query to visualize');
			return false;
		}

		// Parse the query
		const parsedQuery = parseQueryForVisualization(queryToVisualize, dbType);
		const parseError = parsedQuery ? undefined : getParseError(queryToVisualize, dbType) || 'Unable to parse query';

		// Store result on query tab (use full query for staleness detection)
		this.setVisualizeResult(tabId, parsedQuery, tab.query, parseError);

		if (parseError) {
			toast.error(`Parse warning: ${parseError}`);
		}

		return true;
	}

	/**
	 * Visualize a query from a query tab.
	 * If cursorOffset is provided, visualizes only the statement at that cursor position.
	 */
	visualize(tabId: string, cursorOffset?: number): void {
		if (!this.state.activeProjectId || !this.state.activeConnection) return;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab || !tab.query.trim()) {
			toast.error('No query to visualize');
			return;
		}

		// Get the statement to visualize based on cursor position
		const dbType = this.state.activeConnection.type;
		let queryToVisualize = tab.query;

		if (cursorOffset !== undefined) {
			const statement = getStatementAtOffset(tab.query, cursorOffset, dbType);
			if (statement) {
				queryToVisualize = statement.sql;
			}
		}

		if (!queryToVisualize.trim()) {
			toast.error('No query to visualize');
			return;
		}

		// Parse the query
		const parsedQuery = parseQueryForVisualization(queryToVisualize, dbType);
		const parseError = parsedQuery ? undefined : getParseError(queryToVisualize, dbType) || 'Unable to parse query';

		// Create a new visualize tab
		const visualizeTabs = this.state.visualizeTabsByProject[projectId] ?? [];
		const visualizeTabId = `visualize-${Date.now()}`;
		const queryPreview = queryToVisualize.substring(0, 30).replace(/\s+/g, ' ').trim();
		const newVisualizeTab: VisualizeTab = $state({
			id: visualizeTabId,
			name: `Visual: ${queryPreview}...`,
			sourceQuery: queryToVisualize,
			parsedQuery,
			parseError
		});

		this.state.visualizeTabsByProject = {
			...this.state.visualizeTabsByProject,
			[projectId]: [...visualizeTabs, newVisualizeTab]
		};

		this.tabOrdering.add(visualizeTabId);

		// Set as active and switch view
		this.state.activeVisualizeTabIdByProject = {
			...this.state.activeVisualizeTabIdByProject,
			[projectId]: visualizeTabId
		};
		this.setActiveView('visualize');
		this.schedulePersistence(projectId);

		if (parseError) {
			toast.error(`Parse warning: ${parseError}`);
		}
	}

	/**
	 * Remove a visualize tab by ID.
	 */
	remove(id: string): void {
		this.tabOrdering.removeTabGeneric(
			() => this.state.visualizeTabsByProject,
			(r) => (this.state.visualizeTabsByProject = r),
			() => this.state.activeVisualizeTabIdByProject,
			(r) => (this.state.activeVisualizeTabIdByProject = r),
			id
		);
		this.schedulePersistence(this.state.activeProjectId);
		// Switch to query view if no visualize tabs left
		if (this.state.activeProjectId && this.state.visualizeTabs.length === 0) {
			this.setActiveView('query');
		}
	}

	/**
	 * Set the active visualize tab by ID.
	 */
	setActive(id: string): void {
		if (!this.state.activeProjectId) return;

		this.state.activeVisualizeTabIdByProject = {
			...this.state.activeVisualizeTabIdByProject,
			[this.state.activeProjectId]: id
		};
		this.schedulePersistence(this.state.activeProjectId);
	}
}
