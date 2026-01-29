import { errorToast } from '$lib/utils/toast';
import type { VisualizeTab, ParsedQueryVisual, ParameterValue } from '$lib/types';
import type { DatabaseState } from './state.svelte.js';
import type { TabOrderingManager } from './tab-ordering.svelte.js';
import { BaseTabManager, type TabStateAccessors } from './base-tab-manager.svelte.js';
import { parseQueryForVisualization, getParseError } from '$lib/db/sql-ast-parser';
import { getStatementAtOffset } from '$lib/db/sql-parser';
import { substituteParameters } from '$lib/db/query-params';

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
export class VisualizeTabManager extends BaseTabManager<VisualizeTab> {
	private setVisualizeResult?: SetVisualizeResultCallback;
	private setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'canvas' | 'visualize') => void;

	constructor(
		state: DatabaseState,
		tabOrdering: TabOrderingManager,
		schedulePersistence: (projectId: string | null) => void,
		setActiveView: (view: 'query' | 'schema' | 'explain' | 'erd' | 'statistics' | 'canvas' | 'visualize') => void
	) {
		super(state, tabOrdering, schedulePersistence);
		this.setActiveView = setActiveView;
	}

	protected get accessors(): TabStateAccessors<VisualizeTab> {
		return {
			getTabs: () => this.state.visualizeTabsByProject,
			setTabs: (r) => (this.state.visualizeTabsByProject = r),
			getActiveId: () => this.state.activeVisualizeTabIdByProject,
			setActiveId: (r) => (this.state.activeVisualizeTabIdByProject = r)
		};
	}

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
			errorToast('No query to visualize');
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
			errorToast('No query to visualize');
			return false;
		}

		// Parse the query
		const parsedQuery = parseQueryForVisualization(queryToVisualize, dbType);
		const parseError = parsedQuery ? undefined : getParseError(queryToVisualize, dbType) || 'Unable to parse query';

		// Store result on query tab (use full query for staleness detection)
		this.setVisualizeResult(tabId, parsedQuery, tab.query, parseError);

		if (parseError) {
			errorToast(`Parse warning: ${parseError}`);
		}

		return true;
	}

	/**
	 * Visualize a query with parameter substitution (embedded version).
	 * Substitutes {{param}} placeholders with values before parsing.
	 */
	visualizeEmbeddedWithParams(
		tabId: string,
		parameterValues: ParameterValue[],
		cursorOffset?: number
	): boolean {
		if (!this.state.activeProjectId || !this.state.activeConnection) return false;
		if (!this.setVisualizeResult) {
			console.warn('Embedded callback not set, falling back to tab-based visualize');
			this.visualizeWithParams(tabId, parameterValues, cursorOffset);
			return true;
		}

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab || !tab.query.trim()) {
			errorToast('No query to visualize');
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
			errorToast('No query to visualize');
			return false;
		}

		// Substitute parameters in the query
		// For visualization, we always inline values since we're just parsing, not executing
		const { sql: substitutedQuery } = substituteParameters(
			queryToVisualize,
			parameterValues,
			dbType,
			true // forceInline: always inline for visualization
		);

		// Parse the substituted query
		const parsedQuery = parseQueryForVisualization(substitutedQuery, dbType);
		const parseError = parsedQuery ? undefined : getParseError(substitutedQuery, dbType) || 'Unable to parse query';

		// Store result on query tab (use full query for staleness detection)
		this.setVisualizeResult(tabId, parsedQuery, tab.query, parseError);

		if (parseError) {
			errorToast(`Parse warning: ${parseError}`);
		}

		return true;
	}

	/**
	 * Visualize a query with parameter substitution (tab-based version).
	 */
	visualizeWithParams(tabId: string, parameterValues: ParameterValue[], cursorOffset?: number): void {
		if (!this.state.activeProjectId || !this.state.activeConnection) return;

		const projectId = this.state.activeProjectId;
		const tabs = this.state.queryTabsByProject[projectId] ?? [];
		const tab = tabs.find((t) => t.id === tabId);
		if (!tab || !tab.query.trim()) {
			errorToast('No query to visualize');
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
			errorToast('No query to visualize');
			return;
		}

		// Substitute parameters in the query
		// For visualization, we always inline values since we're just parsing, not executing
		const { sql: substitutedQuery } = substituteParameters(
			queryToVisualize,
			parameterValues,
			dbType,
			true // forceInline: always inline for visualization
		);

		// Parse the substituted query
		const parsedQuery = parseQueryForVisualization(substitutedQuery, dbType);
		const parseError = parsedQuery ? undefined : getParseError(substitutedQuery, dbType) || 'Unable to parse query';

		// Create a new visualize tab
		const queryPreview = queryToVisualize.substring(0, 30).replace(/\s+/g, ' ').trim();
		const newVisualizeTab: VisualizeTab = $state({
			id: `visualize-${Date.now()}`,
			name: `Visual: ${queryPreview}...`,
			sourceQuery: queryToVisualize, // Keep original with {{}} for display
			parsedQuery,
			parseError
		});

		this.appendTab(newVisualizeTab);
		this.setActiveView('visualize');

		if (parseError) {
			errorToast(`Parse warning: ${parseError}`);
		}
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
			errorToast('No query to visualize');
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
			errorToast('No query to visualize');
			return;
		}

		// Parse the query
		const parsedQuery = parseQueryForVisualization(queryToVisualize, dbType);
		const parseError = parsedQuery ? undefined : getParseError(queryToVisualize, dbType) || 'Unable to parse query';

		// Create a new visualize tab
		const queryPreview = queryToVisualize.substring(0, 30).replace(/\s+/g, ' ').trim();
		const newVisualizeTab: VisualizeTab = $state({
			id: `visualize-${Date.now()}`,
			name: `Visual: ${queryPreview}...`,
			sourceQuery: queryToVisualize,
			parsedQuery,
			parseError
		});

		this.appendTab(newVisualizeTab);
		this.setActiveView('visualize');

		if (parseError) {
			errorToast(`Parse warning: ${parseError}`);
		}
	}

	/**
	 * Remove a visualize tab by ID.
	 */
	override remove(id: string): void {
		super.remove(id);
		// Switch to query view if no visualize tabs left
		if (this.state.activeProjectId && this.state.visualizeTabs.length === 0) {
			this.setActiveView('query');
		}
	}
}
