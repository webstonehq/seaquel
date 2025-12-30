import type { QueryTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { TabOrderingManager } from "./tab-ordering.svelte.js";
import { setMapValue } from "./map-utils.js";

/**
 * Manages query tabs: add, remove, rename, update content.
 */
export class QueryTabManager {
  constructor(
    private state: DatabaseState,
    private tabOrdering: TabOrderingManager,
    private schedulePersistence: (connectionId: string | null) => void
  ) {}

  /**
   * Add a new query tab.
   */
  add(name?: string, query?: string, savedQueryId?: string): string | null {
    if (!this.state.activeConnectionId) return null;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const newTab: QueryTab = $state({
      id: `tab-${Date.now()}`,
      name: name || `Query ${tabs.length + 1}`,
      query: query || "",
      isExecuting: false,
      savedQueryId,
    });

    // Create new Map to trigger reactivity
    const newQueryTabs = new Map(this.state.queryTabsByConnection);
    newQueryTabs.set(this.state.activeConnectionId, [...tabs, newTab]);
    this.state.queryTabsByConnection = newQueryTabs;

    const newActiveQueryIds = new Map(this.state.activeQueryTabIdByConnection);
    newActiveQueryIds.set(this.state.activeConnectionId, newTab.id);
    this.state.activeQueryTabIdByConnection = newActiveQueryIds;

    this.tabOrdering.add(newTab.id);
    this.schedulePersistence(this.state.activeConnectionId);
    return newTab.id;
  }

  /**
   * Remove a query tab by ID.
   */
  remove(id: string): void {
    this.tabOrdering.removeTabGeneric(
      () => this.state.queryTabsByConnection,
      (m) => (this.state.queryTabsByConnection = m),
      () => this.state.activeQueryTabIdByConnection,
      (m) => (this.state.activeQueryTabIdByConnection = m),
      id
    );
    this.schedulePersistence(this.state.activeConnectionId);
  }

  /**
   * Rename a query tab.
   */
  rename(id: string, newName: string): void {
    if (!this.state.activeConnectionId) return;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === id);
    if (tab) {
      // Create new array with updated tab object for proper reactivity
      const updatedTabs = tabs.map((t) => (t.id === id ? { ...t, name: newName } : t));
      const newQueryTabs = new Map(this.state.queryTabsByConnection);
      newQueryTabs.set(this.state.activeConnectionId, updatedTabs);
      this.state.queryTabsByConnection = newQueryTabs;

      // Also update linked saved query name if exists
      if (tab.savedQueryId) {
        const savedQueries =
          this.state.savedQueriesByConnection.get(this.state.activeConnectionId) || [];
        const updatedSavedQueries = savedQueries.map((q) =>
          q.id === tab.savedQueryId ? { ...q, name: newName, updatedAt: new Date() } : q
        );
        const newSavedQueries = new Map(this.state.savedQueriesByConnection);
        newSavedQueries.set(this.state.activeConnectionId, updatedSavedQueries);
        this.state.savedQueriesByConnection = newSavedQueries;
      }

      this.schedulePersistence(this.state.activeConnectionId);
    }
  }

  /**
   * Set the active query tab by ID.
   */
  setActive(id: string): void {
    if (!this.state.activeConnectionId) return;

    const newActiveQueryIds = new Map(this.state.activeQueryTabIdByConnection);
    newActiveQueryIds.set(this.state.activeConnectionId, id);
    this.state.activeQueryTabIdByConnection = newActiveQueryIds;
    this.schedulePersistence(this.state.activeConnectionId);
  }

  /**
   * Check if a tab has unsaved changes.
   */
  hasUnsavedChanges(tabId: string): boolean {
    const tab = this.state.queryTabs.find((t) => t.id === tabId);
    if (!tab) return false;

    // Empty tabs are not considered "unsaved"
    if (!tab.query.trim()) return false;

    // Tab not linked to a saved query = unsaved
    if (!tab.savedQueryId) return true;

    // Tab linked to saved query - compare content
    const savedQuery = this.state.activeConnectionSavedQueries.find((q) => q.id === tab.savedQueryId);
    if (!savedQuery) return true;

    return tab.query !== savedQuery.query;
  }

  /**
   * Update the query content in a tab.
   */
  updateContent(id: string, query: string): void {
    if (!this.state.activeConnectionId) return;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === id);
    if (tab && tab.query !== query) {
      // Create new objects for proper reactivity
      const updatedTabs = tabs.map((t) => (t.id === id ? { ...t, query } : t));
      const newQueryTabs = new Map(this.state.queryTabsByConnection);
      newQueryTabs.set(this.state.activeConnectionId, updatedTabs);
      this.state.queryTabsByConnection = newQueryTabs;
      this.schedulePersistence(this.state.activeConnectionId);
    }
  }

  /**
   * Find a query tab by its query content and focus it, or create a new one if not found.
   * Returns the tab ID.
   */
  focusOrCreate(query: string, name?: string, setActiveView?: () => void): string | null {
    if (!this.state.activeConnectionId) return null;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
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
   * Load a saved query into a tab (or switch to existing tab).
   */
  loadSaved(savedQueryId: string, setActiveView?: () => void): void {
    if (!this.state.activeConnectionId) return;

    const savedQueries = this.state.savedQueriesByConnection.get(this.state.activeConnectionId) || [];
    const savedQuery = savedQueries.find((q) => q.id === savedQueryId);
    if (!savedQuery) return;

    // Check if a tab with this saved query is already open
    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const existingTab = tabs.find((t) => t.savedQueryId === savedQueryId);

    if (existingTab) {
      // Switch to existing tab
      this.setActive(existingTab.id);
      setActiveView?.();
    } else {
      // Create new tab
      this.add(savedQuery.name, savedQuery.query, savedQueryId);
      setActiveView?.();
    }
  }

  /**
   * Load a query from history into a tab (or switch to existing tab).
   */
  loadFromHistory(historyId: string, setActiveView?: () => void): void {
    if (!this.state.activeConnectionId) return;

    const queryHistory = this.state.queryHistoryByConnection.get(this.state.activeConnectionId) || [];
    const item = queryHistory.find((h) => h.id === historyId);
    if (!item) return;

    // Check if a tab with the exact same query is already open
    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
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
}
