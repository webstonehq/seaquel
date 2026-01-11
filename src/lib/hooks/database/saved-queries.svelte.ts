import type { SavedQuery, QueryTab, QueryParameter } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

/**
 * Manages saved queries: save, delete.
 * Note: Saved queries are per-connection, tabs are per-project.
 * Note: loadSavedQuery is in UseDatabase as it orchestrates with tabs and views.
 */
export class SavedQueryManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (connectionId: string | null) => void,
    private scheduleProjectPersistence: (projectId: string | null) => void
  ) {}

  saveQuery(name: string, query: string, tabId?: string, parameters?: QueryParameter[]): string | null {
    if (!this.state.activeConnectionId || !this.state.activeProjectId) return null;

    const connectionId = this.state.activeConnectionId;
    const projectId = this.state.activeProjectId;

    // Check if this tab is already linked to a saved query
    let savedQueryId: string | undefined;
    if (tabId) {
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      const tab = tabs.find((t: QueryTab) => t.id === tabId);
      savedQueryId = tab?.savedQueryId;
    }

    if (savedQueryId) {
      // Update existing saved query with new object for proper reactivity
      const savedQueries = this.state.savedQueriesByConnection[connectionId] ?? [];
      const savedQuery = savedQueries.find((q) => q.id === savedQueryId);
      if (savedQuery) {
        const updatedSavedQueries = savedQueries.map((q) =>
          q.id === savedQueryId
            ? { ...q, name, query, parameters, updatedAt: new Date() }
            : q
        );
        this.state.savedQueriesByConnection = {
          ...this.state.savedQueriesByConnection,
          [connectionId]: updatedSavedQueries,
        };

        // Also update tab name if it differs
        if (tabId) {
          const tabs = this.state.queryTabsByProject[projectId] ?? [];
          const tab = tabs.find((t: QueryTab) => t.id === tabId);
          if (tab && tab.name !== name) {
            const updatedTabs = tabs.map((t: QueryTab) =>
              t.id === tabId ? { ...t, name } : t
            );
            this.state.queryTabsByProject = {
              ...this.state.queryTabsByProject,
              [projectId]: updatedTabs,
            };
            this.scheduleProjectPersistence(projectId);
          }
        }

        this.schedulePersistence(connectionId);
        return savedQueryId;
      }
    }

    // Create new saved query
    const newSavedQuery: SavedQuery = {
      id: `saved-${Date.now()}`,
      name,
      query,
      connectionId,
      createdAt: new Date(),
      updatedAt: new Date(),
      parameters,
    };

    const savedQueries = this.state.savedQueriesByConnection[connectionId] ?? [];
    this.state.savedQueriesByConnection = {
      ...this.state.savedQueriesByConnection,
      [connectionId]: [...savedQueries, newSavedQuery],
    };

    // Link tab to saved query if tabId provided
    if (tabId) {
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      const updatedTabs = tabs.map((t: QueryTab) =>
        t.id === tabId
          ? { ...t, savedQueryId: newSavedQuery.id, name }
          : t
      );
      this.state.queryTabsByProject = {
        ...this.state.queryTabsByProject,
        [projectId]: updatedTabs,
      };
      this.scheduleProjectPersistence(projectId);
    }

    this.schedulePersistence(connectionId);
    return newSavedQuery.id;
  }

  deleteSavedQuery(id: string) {
    if (!this.state.activeConnectionId || !this.state.activeProjectId) return;

    const connectionId = this.state.activeConnectionId;
    const projectId = this.state.activeProjectId;
    const savedQueries = this.state.savedQueriesByConnection[connectionId] ?? [];
    const filtered = savedQueries.filter((q) => q.id !== id);

    this.state.savedQueriesByConnection = {
      ...this.state.savedQueriesByConnection,
      [connectionId]: filtered,
    };

    // Remove savedQueryId from any tabs using this query
    const tabs = this.state.queryTabsByProject[projectId] ?? [];
    const updatedTabs = tabs.map((tab: QueryTab) =>
      tab.savedQueryId === id ? { ...tab, savedQueryId: undefined } : tab
    );

    this.state.queryTabsByProject = {
      ...this.state.queryTabsByProject,
      [projectId]: updatedTabs,
    };
    this.schedulePersistence(connectionId);
    this.scheduleProjectPersistence(projectId);
  }
}
