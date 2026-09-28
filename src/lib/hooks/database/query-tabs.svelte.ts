import type { QueryTab, ExplainResult, ParsedQueryVisual } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import type { SharedQueryManager } from "./shared-query-manager.svelte.js";
import { BaseTabManager, type TabStateAccessors } from "./base-tab-manager.svelte.js";

/**
 * Manages query tabs: add, remove, rename, update content.
 * Tabs are organized per-project.
 */
export class QueryTabManager extends BaseTabManager<QueryTab> {
  private sharedQueryManager: SharedQueryManager | null = null;

  constructor(
    state: DatabaseState,
    tabOrdering: TabOrderingManager,
    schedulePersistence: (projectId: string | null) => void,
  ) {
    super(state, tabOrdering, schedulePersistence);
  }

  setSharedQueryManager(manager: SharedQueryManager): void {
    this.sharedQueryManager = manager;
  }

  /** Told when a tab closes (its run is cancelled) and when one becomes active. */
  private lifecycle: { removed?: (id: string) => void; activated?: (id: string) => void } = {};

  setLifecycleListener(listener: {
    removed?: (id: string) => void;
    activated?: (id: string) => void;
  }): void {
    this.lifecycle = listener;
  }

  override remove(id: string): void {
    super.remove(id);
    this.lifecycle.removed?.(id);
  }

  override setActive(id: string): void {
    super.setActive(id);
    this.lifecycle.activated?.(id);
  }

  protected get accessors(): TabStateAccessors<QueryTab> {
    return {
      getTabs: () => this.state.queryTabsByProject,
      setTabs: (r) => (this.state.queryTabsByProject = r),
      getActiveId: () => this.state.activeQueryTabIdByProject,
      setActiveId: (r) => (this.state.activeQueryTabIdByProject = r),
    };
  }

  /**
   * Add a new query tab.
   */
  add(name?: string, query?: string, queryId?: string): string | null {
    if (!this.state.activeProjectId) return null;

    const tabs = this.getProjectTabs();
    const newTab: QueryTab = $state({
      id: `tab-${crypto.randomUUID()}`,
      name: name || `Query ${tabs.length + 1}`,
      query: query || "",
      isExecuting: false,
      queryId,
    });

    const id = this.appendTab(newTab);
    // A new tab is the active one: tell the listener as a switch.
    this.lifecycle.activated?.(id);
    return id;
  }

  /**
   * Rename a query tab.
   */
  async rename(id: string, newName: string): Promise<void> {
    if (!this.state.activeProjectId) return;

    const tabs = this.getProjectTabs();
    const tab = tabs.find((t) => t.id === id);
    if (tab) {
      this.updateTab(id, (t) => ({ ...t, name: newName }));

      // Also update linked query name if exists
      if (tab.queryId && this.state.activeProjectId) {
        const projectId = this.state.activeProjectId;
        const queries = this.state.queriesByProject[projectId] ?? [];
        const query = queries.find((q) => q.id === tab.queryId);
        if (query) {
          const updatedQueries = queries.map((q) =>
            q.id === tab.queryId ? { ...q, name: newName, updatedAt: new Date() } : q,
          );
          this.state.queriesByProject = {
            ...this.state.queriesByProject,
            [projectId]: updatedQueries,
          };

          // If shared, also update the .sql file
          if (query.shared && this.sharedQueryManager) {
            const updatedQuery = updatedQueries.find((q) => q.id === tab.queryId);
            if (updatedQuery) {
              await this.sharedQueryManager.writeQueryFile(updatedQuery);
            }
          }
        }
      }

      this.schedulePersistence(this.state.activeProjectId);
    }
  }

  /**
   * Check if a tab has unsaved changes.
   */
  hasUnsavedChanges(tabId: string): boolean {
    const tab = this.state.queryTabs.find((t) => t.id === tabId);
    if (!tab) return false;

    // Empty tabs are not considered "unsaved"
    if (!tab.query.trim()) return false;

    // Tab linked to a query - compare content
    if (tab.queryId) {
      const query = this.state.projectQueries.find((q) => q.id === tab.queryId);
      if (!query) return true;
      return tab.query !== query.query;
    }

    // Tab not linked to any query = unsaved
    return true;
  }

  /**
   * Update the query content in a tab.
   */
  updateContent(id: string, query: string): void {
    if (!this.state.activeProjectId) return;

    const tabs = this.getProjectTabs();
    const tab = tabs.find((t) => t.id === id);
    if (tab && tab.query !== query) {
      this.updateTab(id, (t) => ({ ...t, query }));
      this.schedulePersistence(this.state.activeProjectId);
    }
  }

  /**
   * Find a query tab by its query content and focus it, or create a new one if not found.
   * Returns the tab ID.
   */
  focusOrCreate(query: string, name?: string, setActiveView?: () => void): string | null {
    if (!this.state.activeProjectId) return null;

    const tabs = this.getProjectTabs();
    const existingTab = tabs.find((t) => t.query.trim() === query.trim());

    if (existingTab) {
      this.setActive(existingTab.id);
      setActiveView?.();
      return existingTab.id;
    }

    // Create new tab if not found
    const newTabId = this.add(name, query);
    setActiveView?.();
    return newTabId;
  }

  /**
   * Load a query into a tab (or switch to existing tab).
   */
  loadQuery(queryId: string, setActiveView?: () => void): void {
    if (!this.state.activeProjectId) return;

    const queries = this.state.queriesByProject[this.state.activeProjectId] ?? [];
    const query = queries.find((q) => q.id === queryId);
    if (!query) return;

    // Check if a tab with this query is already open
    const tabs = this.getProjectTabs();
    const existingTab = tabs.find((t) => t.queryId === queryId);

    if (existingTab) {
      this.setActive(existingTab.id);
      setActiveView?.();
    } else {
      this.add(query.name, query.query, queryId);
      setActiveView?.();
    }
  }

  /** @deprecated Use loadQuery instead */
  loadSaved(queryId: string, setActiveView?: () => void): void {
    this.loadQuery(queryId, setActiveView);
  }

  /** @deprecated Use loadQuery instead */
  loadSharedQuery(
    queryId: string,
    _name: string,
    _query: string,
    setActiveView?: () => void,
  ): void {
    this.loadQuery(queryId, setActiveView);
  }

  /**
   * Load a query from history into a tab (or switch to existing tab).
   * Note: Query history is per-connection, so we need an active connection.
   */
  loadFromHistory(historyId: string, setActiveView?: () => void): void {
    if (!this.state.activeProjectId || !this.state.activeConnectionId) return;

    const queryHistory = this.state.queryHistoryByConnection[this.state.activeConnectionId] ?? [];
    const item = queryHistory.find((h) => h.id === historyId);
    if (!item) return;

    // Check if a tab with the exact same query is already open
    const tabs = this.getProjectTabs();
    const existingTab = tabs.find((t) => t.query.trim() === item.query.trim());

    if (existingTab) {
      // Switch to existing tab
      this.setActive(existingTab.id);
      setActiveView?.();
    } else {
      // Create new tab
      this.add(`History: ${item.query.substring(0, 20)}...`, item.query);
      setActiveView?.();
    }
  }

  /**
   * Set the explain result on a query tab.
   */
  setExplainResult(
    tabId: string,
    result: ExplainResult,
    sourceQuery: string,
    isAnalyze: boolean,
  ): void {
    if (!this.state.activeProjectId) return;

    this.updateTab(tabId, (t) => ({
      ...t,
      explainResult: { result, sourceQuery, isAnalyze, isExecuting: false },
    }));
    this.schedulePersistence(this.state.activeProjectId);
  }

  /**
   * Set the explain executing state on a query tab.
   */
  setExplainExecuting(tabId: string, isExecuting: boolean, isAnalyze: boolean = false): void {
    if (!this.state.activeProjectId) return;

    this.updateTab(tabId, (t) => ({
      ...t,
      explainResult: t.explainResult
        ? { ...t.explainResult, isExecuting }
        : {
            result: undefined as unknown as ExplainResult,
            sourceQuery: "",
            isAnalyze,
            isExecuting,
          },
    }));
  }

  /**
   * Clear the explain result from a query tab.
   */
  clearExplainResult(tabId: string): void {
    if (!this.state.activeProjectId) return;

    this.updateTab(tabId, (t) => ({ ...t, explainResult: undefined }));
    this.schedulePersistence(this.state.activeProjectId);
  }

  /**
   * Set the visualize result on a query tab.
   */
  setVisualizeResult(
    tabId: string,
    parsedQuery: ParsedQueryVisual | null,
    sourceQuery: string,
    parseError?: string,
  ): void {
    if (!this.state.activeProjectId) return;

    this.updateTab(tabId, (t) => ({
      ...t,
      visualizeResult: { parsedQuery, sourceQuery, parseError },
    }));
    this.schedulePersistence(this.state.activeProjectId);
  }

  /**
   * Clear the visualize result from a query tab.
   */
  clearVisualizeResult(tabId: string): void {
    if (!this.state.activeProjectId) return;

    this.updateTab(tabId, (t) => ({ ...t, visualizeResult: undefined }));
    this.schedulePersistence(this.state.activeProjectId);
  }
}
