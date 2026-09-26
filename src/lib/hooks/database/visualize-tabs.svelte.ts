import type { ActiveViewType } from "$lib/types/persisted";
import { errorToast } from "$lib/utils/toast";
import type { VisualizeTab, ParsedQueryVisual, ParameterValue, DatabaseType } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";
import { parseVisualQuery } from "$lib/sql";
import { resolveQuery } from "./resolve-query.js";

/**
 * Callback for setting visualize result on a query tab.
 */
export type SetVisualizeResultCallback = (
  tabId: string,
  parsedQuery: ParsedQueryVisual | null,
  sourceQuery: string,
  parseError?: string,
) => void;

/**
 * Parse a query for the Visual tab, once: the AST, or the reason there's none.
 */
function parseVisual(
  sql: string,
  dbType: DatabaseType,
): { parsedQuery: ParsedQueryVisual | null; parseError?: string } {
  const { visual, parseError } = parseVisualQuery(sql, dbType);
  if (visual) return { parsedQuery: visual, parseError: undefined };
  return { parsedQuery: null, parseError: parseError || "Unable to parse query" };
}

/**
 * Manages query visualizer tabs: parse, visualize, remove, set active.
 * Tabs are organized per-project.
 */
export class VisualizeTabManager extends BaseTabManager<VisualizeTab> {
  private setVisualizeResult?: SetVisualizeResultCallback;

  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
    setActiveView: (view: ActiveViewType) => void,
  ) {
    super(state, tabOrdering, schedulePersistence, setActiveView);
  }

  protected get accessors(): TabStateAccessors<VisualizeTab> {
    return {
      getTabs: () => this.state.visualizeTabsByProject,
      setTabs: (r) => (this.state.visualizeTabsByProject = r),
      getActiveId: () => this.state.activeVisualizeTabIdByProject,
      setActiveId: (r) => (this.state.activeVisualizeTabIdByProject = r),
    };
  }

  /**
   * Set callback for embedded visualize results (stored on QueryTab).
   */
  setEmbeddedCallback(setResult: SetVisualizeResultCallback): void {
    this.setVisualizeResult = setResult;
  }

  /**
   * Parse a query and create a VisualizeTab from it.
   */
  private buildVisualizeTab(queryToVisualize: string, dbType: DatabaseType): VisualizeTab {
    const { parsedQuery, parseError } = parseVisual(queryToVisualize, dbType);

    const queryPreview = queryToVisualize.substring(0, 30).replace(/\s+/g, " ").trim();
    return {
      id: `visualize-${crypto.randomUUID()}`,
      name: `Visual: ${queryPreview}...`,
      sourceQuery: queryToVisualize,
      parsedQuery,
      parseError,
    };
  }

  /**
   * Parse a query and report any parse errors via toast.
   * Returns { parsedQuery, parseError } for embedding or tab creation.
   */
  private parseForVisualization(
    queryToVisualize: string,
    dbType: DatabaseType,
  ): { parsedQuery: ParsedQueryVisual | null; parseError?: string } {
    const { parsedQuery, parseError } = parseVisual(queryToVisualize, dbType);
    if (parseError) {
      errorToast(`Parse warning: ${parseError}`);
    }
    return { parsedQuery, parseError };
  }

  /**
   * Visualize a query and store result embedded in the query tab.
   */
  visualizeEmbedded(tabId: string, cursorOffset?: number): boolean {
    if (!this.state.activeProjectId || !this.state.activeConnection) return false;
    if (!this.setVisualizeResult) {
      console.warn("Embedded callback not set, falling back to tab-based visualize");
      this.visualize(tabId, cursorOffset);
      return true;
    }

    const resolved = resolveQuery(this.state, tabId, cursorOffset);
    if (!resolved) {
      errorToast("No query to visualize");
      return false;
    }

    const dbType = this.state.activeConnection.type;
    const { parsedQuery, parseError } = this.parseForVisualization(resolved.query, dbType);
    this.setVisualizeResult(tabId, parsedQuery, resolved.tab.query, parseError);
    return true;
  }

  /**
   * Visualize a query with parameter substitution (embedded version).
   */
  visualizeEmbeddedWithParams(
    tabId: string,
    parameterValues: ParameterValue[],
    cursorOffset?: number,
  ): boolean {
    if (!this.state.activeProjectId || !this.state.activeConnection) return false;
    if (!this.setVisualizeResult) {
      console.warn("Embedded callback not set, falling back to tab-based visualize");
      this.visualizeWithParams(tabId, parameterValues, cursorOffset);
      return true;
    }

    const resolved = resolveQuery(this.state, tabId, cursorOffset, parameterValues, true);
    if (!resolved) {
      errorToast("No query to visualize");
      return false;
    }

    const dbType = this.state.activeConnection.type;
    const { parsedQuery, parseError } = this.parseForVisualization(resolved.query, dbType);
    this.setVisualizeResult(tabId, parsedQuery, resolved.tab.query, parseError);
    return true;
  }

  /**
   * Visualize a query with parameter substitution (tab-based version).
   */
  visualizeWithParams(
    tabId: string,
    parameterValues: ParameterValue[],
    cursorOffset?: number,
  ): void {
    if (!this.state.activeProjectId || !this.state.activeConnection) return;

    // Resolve with the original query (for display), then separately with params (for parsing)
    const original = resolveQuery(this.state, tabId, cursorOffset);
    if (!original) {
      errorToast("No query to visualize");
      return;
    }

    const resolved = resolveQuery(this.state, tabId, cursorOffset, parameterValues, true);
    if (!resolved) {
      errorToast("No query to visualize");
      return;
    }

    const dbType = this.state.activeConnection.type;
    const tab: VisualizeTab = $state(this.buildVisualizeTab(resolved.query, dbType));
    // Keep original query with {{}} for display
    tab.sourceQuery = original.query;
    this.appendTab(tab);
    this.viewFallbackFn!("visualize");

    if (tab.parseError) {
      errorToast(`Parse warning: ${tab.parseError}`);
    }
  }

  /**
   * Visualize a query from a query tab.
   * If cursorOffset is provided, visualizes only the statement at that cursor position.
   */
  visualize(tabId: string, cursorOffset?: number): void {
    if (!this.state.activeProjectId || !this.state.activeConnection) return;

    const resolved = resolveQuery(this.state, tabId, cursorOffset);
    if (!resolved) {
      errorToast("No query to visualize");
      return;
    }

    const dbType = this.state.activeConnection.type;
    const tab: VisualizeTab = $state(this.buildVisualizeTab(resolved.query, dbType));
    this.appendTab(tab);
    this.viewFallbackFn!("visualize");

    if (tab.parseError) {
      errorToast(`Parse warning: ${tab.parseError}`);
    }
  }
}
