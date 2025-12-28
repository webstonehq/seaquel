import type { SavedQuery } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

/**
 * Manages saved queries: save, delete.
 * Note: loadSavedQuery is in UseDatabase as it orchestrates with tabs and views.
 */
export class SavedQueryManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (connectionId: string | null) => void
  ) {}

  saveQuery(name: string, query: string, tabId?: string): string | null {
    if (!this.state.activeConnectionId) return null;

    // Check if this tab is already linked to a saved query
    let savedQueryId: string | undefined;
    if (tabId) {
      const tabs =
        this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
      const tab = tabs.find((t) => t.id === tabId);
      savedQueryId = tab?.savedQueryId;
    }

    if (savedQueryId) {
      // Update existing saved query with new object for proper reactivity
      const savedQueries =
        this.state.savedQueriesByConnection.get(this.state.activeConnectionId) || [];
      const savedQuery = savedQueries.find((q) => q.id === savedQueryId);
      if (savedQuery) {
        const updatedSavedQueries = savedQueries.map((q) =>
          q.id === savedQueryId
            ? { ...q, name, query, updatedAt: new Date() }
            : q
        );
        const newSavedQueries = new Map(this.state.savedQueriesByConnection);
        newSavedQueries.set(this.state.activeConnectionId, updatedSavedQueries);
        this.state.savedQueriesByConnection = newSavedQueries;

        // Also update tab name if it differs
        if (tabId) {
          const tabs =
            this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
          const tab = tabs.find((t) => t.id === tabId);
          if (tab && tab.name !== name) {
            const updatedTabs = tabs.map((t) =>
              t.id === tabId ? { ...t, name } : t
            );
            const newQueryTabs = new Map(this.state.queryTabsByConnection);
            newQueryTabs.set(this.state.activeConnectionId, updatedTabs);
            this.state.queryTabsByConnection = newQueryTabs;
          }
        }

        this.schedulePersistence(this.state.activeConnectionId);
        return savedQueryId;
      }
    }

    // Create new saved query
    const newSavedQuery: SavedQuery = {
      id: `saved-${Date.now()}`,
      name,
      query,
      connectionId: this.state.activeConnectionId,
      createdAt: new Date(),
      updatedAt: new Date(),
    };

    const savedQueries =
      this.state.savedQueriesByConnection.get(this.state.activeConnectionId) || [];
    const newSavedQueries = new Map(this.state.savedQueriesByConnection);
    newSavedQueries.set(this.state.activeConnectionId, [
      ...savedQueries,
      newSavedQuery,
    ]);
    this.state.savedQueriesByConnection = newSavedQueries;

    // Link tab to saved query if tabId provided
    if (tabId) {
      const tabs =
        this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
      const updatedTabs = tabs.map((t) =>
        t.id === tabId
          ? { ...t, savedQueryId: newSavedQuery.id, name }
          : t
      );
      const newQueryTabs = new Map(this.state.queryTabsByConnection);
      newQueryTabs.set(this.state.activeConnectionId, updatedTabs);
      this.state.queryTabsByConnection = newQueryTabs;
    }

    this.schedulePersistence(this.state.activeConnectionId);
    return newSavedQuery.id;
  }

  deleteSavedQuery(id: string) {
    if (!this.state.activeConnectionId) return;

    const savedQueries =
      this.state.savedQueriesByConnection.get(this.state.activeConnectionId) || [];
    const filtered = savedQueries.filter((q) => q.id !== id);
    const newSavedQueries = new Map(this.state.savedQueriesByConnection);
    newSavedQueries.set(this.state.activeConnectionId, filtered);
    this.state.savedQueriesByConnection = newSavedQueries;

    // Remove savedQueryId from any tabs using this query
    const tabs = this.state.queryTabsByConnection.get(this.state.activeConnectionId) || [];
    tabs.forEach((tab) => {
      if (tab.savedQueryId === id) {
        tab.savedQueryId = undefined;
      }
    });
    const newQueryTabs = new Map(this.state.queryTabsByConnection);
    newQueryTabs.set(this.state.activeConnectionId, [...tabs]);
    this.state.queryTabsByConnection = newQueryTabs;
    this.schedulePersistence(this.state.activeConnectionId);
  }
}
