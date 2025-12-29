import type { QueryTab } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

/**
 * Manages query tabs: create, remove, rename, update content.
 * Note: focusOrCreateQueryTab is in UseDatabase as it orchestrates with setActiveView.
 */
export class QueryTabManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (connectionId: string | null) => void,
    private removeTabGeneric: <T extends { id: string }>(
      tabsGetter: () => Map<string, T[]>,
      tabsSetter: (m: Map<string, T[]>) => void,
      activeIdGetter: () => Map<string, string | null>,
      activeIdSetter: (m: Map<string, string | null>) => void,
      tabId: string
    ) => void,
    private addToTabOrder: (tabId: string) => void
  ) {}

  addQueryTab(name?: string, query?: string, savedQueryId?: string): string | null {
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

    this.addToTabOrder(newTab.id);
    this.schedulePersistence(this.state.activeConnectionId);
    return newTab.id;
  }

  removeQueryTab(id: string) {
    this.removeTabGeneric(
      () => this.state.queryTabsByConnection,
      (m) => (this.state.queryTabsByConnection = m),
      () => this.state.activeQueryTabIdByConnection,
      (m) => (this.state.activeQueryTabIdByConnection = m),
      id
    );
    this.schedulePersistence(this.state.activeConnectionId);
  }

  renameQueryTab(id: string, newName: string) {
    if (!this.state.activeConnectionId) return;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === id);
    if (tab) {
      // Create new array with updated tab object for proper reactivity
      const updatedTabs = tabs.map((t) =>
        t.id === id ? { ...t, name: newName } : t
      );
      const newQueryTabs = new Map(this.state.queryTabsByConnection);
      newQueryTabs.set(this.state.activeConnectionId, updatedTabs);
      this.state.queryTabsByConnection = newQueryTabs;

      // Also update linked saved query name if exists
      if (tab.savedQueryId) {
        const savedQueries =
          this.state.savedQueriesByConnection.get(this.state.activeConnectionId) || [];
        const updatedSavedQueries = savedQueries.map((q) =>
          q.id === tab.savedQueryId
            ? { ...q, name: newName, updatedAt: new Date() }
            : q
        );
        const newSavedQueries = new Map(this.state.savedQueriesByConnection);
        newSavedQueries.set(this.state.activeConnectionId, updatedSavedQueries);
        this.state.savedQueriesByConnection = newSavedQueries;
      }

      this.schedulePersistence(this.state.activeConnectionId);
    }
  }

  setActiveQueryTab(id: string) {
    if (!this.state.activeConnectionId) return;

    const newActiveQueryIds = new Map(this.state.activeQueryTabIdByConnection);
    newActiveQueryIds.set(this.state.activeConnectionId, id);
    this.state.activeQueryTabIdByConnection = newActiveQueryIds;
    this.schedulePersistence(this.state.activeConnectionId);
  }

  hasUnsavedChanges(tabId: string): boolean {
    const tab = this.state.queryTabs.find((t) => t.id === tabId);
    if (!tab) return false;

    // Empty tabs are not considered "unsaved"
    if (!tab.query.trim()) return false;

    // Tab not linked to a saved query = unsaved
    if (!tab.savedQueryId) return true;

    // Tab linked to saved query - compare content
    const savedQuery = this.state.activeConnectionSavedQueries.find(
      (q) => q.id === tab.savedQueryId
    );
    if (!savedQuery) return true;

    return tab.query !== savedQuery.query;
  }

  updateQueryTabContent(id: string, query: string) {
    if (!this.state.activeConnectionId) return;

    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    const tab = tabs.find((t) => t.id === id);
    if (tab && tab.query !== query) {
      // Create new objects for proper reactivity
      const updatedTabs = tabs.map((t) =>
        t.id === id ? { ...t, query } : t
      );
      const newQueryTabs = new Map(this.state.queryTabsByConnection);
      newQueryTabs.set(this.state.activeConnectionId, updatedTabs);
      this.state.queryTabsByConnection = newQueryTabs;
      this.schedulePersistence(this.state.activeConnectionId);
    }
  }
}
