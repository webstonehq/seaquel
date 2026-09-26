import type { Query, QueryTab, QueryParameter, QueryVersion } from "$lib/types";
import type { ResolvedQueryVersion } from "$lib/types";
import { createVersionEntry, resolveVersions } from "$lib/utils/query-versions";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";

/**
 * Manages queries (both local and shared): save, delete, share, unshare.
 * Queries are per-project. SQLite is always the source of truth.
 * When shared=true, a .sql file is maintained as a git projection.
 */
export class SavedQueryManager {
  private removeTab: ((id: string) => void) | null = null;
  private writeQueryFile: ((query: Query) => Promise<void>) | null = null;
  private deleteQueryFile: ((query: Query) => Promise<void>) | null = null;

  constructor(
    private state: DatabaseState,
    private scheduleProjectPersistence: (projectId: string | null) => void,
    private persistence: PersistenceManager,
  ) {}

  setRemoveTab(fn: (id: string) => void) {
    this.removeTab = fn;
  }

  setFileProjection(fns: {
    writeQueryFile: (query: Query) => Promise<void>;
    deleteQueryFile: (query: Query) => Promise<void>;
  }) {
    this.writeQueryFile = fns.writeQueryFile;
    this.deleteQueryFile = fns.deleteQueryFile;
  }

  saveQuery(
    name: string,
    query: string,
    tabId?: string,
    parameters?: QueryParameter[],
    forceNew?: boolean,
  ): string | null {
    if (!this.state.activeProjectId) return null;

    const projectId = this.state.activeProjectId;

    // Check if this tab is already linked to a query
    let existingQueryId: string | undefined;
    if (tabId && !forceNew) {
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      const tab = tabs.find((t: QueryTab) => t.id === tabId);
      existingQueryId = tab?.queryId;
    }

    if (existingQueryId) {
      // Update existing query with new object for proper reactivity
      const queries = this.state.queriesByProject[projectId] ?? [];
      const existingQuery = queries.find((q) => q.id === existingQueryId);
      if (existingQuery) {
        // Create a version for the previous state before updating
        const versions = this.getVersionsForQuery(existingQueryId!);
        const nextVersion =
          versions.length > 0 ? Math.max(...versions.map((v) => v.version)) + 1 : 1;

        // For delta versions, we need the previous version's resolved text
        let previousVersionQuery: string | undefined;
        if (nextVersion > 1) {
          const resolved = resolveVersions(versions);
          previousVersionQuery =
            resolved.length > 0 ? resolved[resolved.length - 1].query : existingQuery.query;
        }

        const newVersion = createVersionEntry(
          existingQueryId!,
          nextVersion,
          existingQuery.query,
          previousVersionQuery,
        );

        // Add to state
        const projectVersions = this.state.queryVersionsByProject[projectId] ?? [];
        this.state.queryVersionsByProject = {
          ...this.state.queryVersionsByProject,
          [projectId]: [...projectVersions, newVersion],
        };

        // Persist immediately
        this.persistence
          .persistQueryVersion({
            id: newVersion.id,
            queryId: newVersion.queryId,
            version: newVersion.version,
            snapshot: newVersion.snapshot,
            diff: newVersion.diff,
            createdAt: newVersion.createdAt.toISOString(),
          })
          .catch((err) => console.error("[saved-queries] Failed to persist query version:", err));

        // Prune old versions if over limit
        this.pruneIfNeeded(existingQueryId!, projectId).catch((err) =>
          console.error("[saved-queries] Failed to prune query versions:", err),
        );

        // Then update the query
        const updatedQueries = queries.map((q) =>
          q.id === existingQueryId ? { ...q, name, query, parameters, updatedAt: new Date() } : q,
        );
        this.state.queriesByProject = {
          ...this.state.queriesByProject,
          [projectId]: updatedQueries,
        };

        // If shared, also update the .sql file
        if (existingQuery.shared) {
          const updated = updatedQueries.find((q) => q.id === existingQueryId);
          if (updated) {
            this.writeQueryFile?.(updated)?.catch((err) =>
              console.error("[saved-queries] Failed to write shared query file:", err),
            );
          }
        }

        // Also update tab name if it differs
        if (tabId) {
          const tabs = this.state.queryTabsByProject[projectId] ?? [];
          const tab = tabs.find((t: QueryTab) => t.id === tabId);
          if (tab && tab.name !== name) {
            const updatedTabs = tabs.map((t: QueryTab) => (t.id === tabId ? { ...t, name } : t));
            this.state.queryTabsByProject = {
              ...this.state.queryTabsByProject,
              [projectId]: updatedTabs,
            };
          }
        }

        this.scheduleProjectPersistence(projectId);
        return existingQueryId;
      }
    }

    // Create new query
    const newQuery: Query = {
      id: `saved-${crypto.randomUUID()}`,
      name,
      query,
      projectId,
      createdAt: new Date(),
      updatedAt: new Date(),
      parameters,
      shared: false,
    };

    const queries = this.state.queriesByProject[projectId] ?? [];
    this.state.queriesByProject = {
      ...this.state.queriesByProject,
      [projectId]: [...queries, newQuery],
    };

    // Link tab to query if tabId provided (skip for forceNew — caller handles it)
    if (tabId && !forceNew) {
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      const updatedTabs = tabs.map((t: QueryTab) =>
        t.id === tabId ? { ...t, queryId: newQuery.id, name } : t,
      );
      this.state.queryTabsByProject = {
        ...this.state.queryTabsByProject,
        [projectId]: updatedTabs,
      };
    }

    this.scheduleProjectPersistence(projectId);
    return newQuery.id;
  }

  deleteQuery(id: string) {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const queries = this.state.queriesByProject[projectId] ?? [];
    const query = queries.find((q) => q.id === id);

    // If shared, delete the .sql file too
    if (query?.shared) {
      this.deleteQueryFile?.(query)?.catch((err) =>
        console.error("[saved-queries] Failed to delete shared query file:", err),
      );
    }

    const filtered = queries.filter((q) => q.id !== id);
    this.state.queriesByProject = {
      ...this.state.queriesByProject,
      [projectId]: filtered,
    };

    // Remove versions from state (SQLite CASCADE handles persistence)
    const projectVersions = this.state.queryVersionsByProject[projectId] ?? [];
    const filteredVersions = projectVersions.filter((v) => v.queryId !== id);
    this.state.queryVersionsByProject = {
      ...this.state.queryVersionsByProject,
      [projectId]: filteredVersions,
    };

    // Close any tabs linked to this query
    const tabs = this.state.queryTabsByProject[projectId] ?? [];
    for (const tab of tabs) {
      if (tab.queryId === id && this.removeTab) {
        this.removeTab(tab.id);
      }
    }

    this.scheduleProjectPersistence(projectId);
  }

  /** @deprecated Use deleteQuery instead */
  deleteSavedQuery(id: string) {
    this.deleteQuery(id);
  }

  toggleQueryStarred(id: string) {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const queries = this.state.queriesByProject[projectId] ?? [];
    const updatedQueries = queries.map((q) => (q.id === id ? { ...q, starred: !q.starred } : q));

    this.state.queriesByProject = {
      ...this.state.queriesByProject,
      [projectId]: updatedQueries,
    };
    this.scheduleProjectPersistence(projectId);
  }

  /** @deprecated Use toggleQueryStarred instead */
  toggleSavedQueryStarred(id: string) {
    this.toggleQueryStarred(id);
  }

  /** @deprecated Use toggleQueryStarred instead */
  toggleSharedQueryStarred(id: string) {
    this.toggleQueryStarred(id);
  }

  /**
   * Share a query: set shared=true and write the .sql file.
   */
  async shareQuery(queryId: string): Promise<void> {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const queries = this.state.queriesByProject[projectId] ?? [];
    const query = queries.find((q) => q.id === queryId);
    if (!query || query.shared) return;

    const updatedQuery = { ...query, shared: true, updatedAt: new Date() };
    this.state.queriesByProject = {
      ...this.state.queriesByProject,
      [projectId]: queries.map((q) => (q.id === queryId ? updatedQuery : q)),
    };

    // Write the .sql file
    await this.writeQueryFile?.(updatedQuery);

    this.scheduleProjectPersistence(projectId);
  }

  /**
   * Unshare a query: set shared=false and delete the .sql file.
   */
  async unshareQuery(queryId: string): Promise<void> {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const queries = this.state.queriesByProject[projectId] ?? [];
    const query = queries.find((q) => q.id === queryId);
    if (!query || !query.shared) return;

    // Delete the .sql file first
    await this.deleteQueryFile?.(query);

    const updatedQuery = { ...query, shared: false, updatedAt: new Date() };
    this.state.queriesByProject = {
      ...this.state.queriesByProject,
      [projectId]: queries.map((q) => (q.id === queryId ? updatedQuery : q)),
    };

    this.scheduleProjectPersistence(projectId);
  }

  getVersionsForQuery(queryId: string): QueryVersion[] {
    if (!this.state.activeProjectId) return [];
    const versions = this.state.queryVersionsByProject[this.state.activeProjectId] ?? [];
    return versions.filter((v) => v.queryId === queryId);
  }

  getResolvedVersionsForQuery(queryId: string): ResolvedQueryVersion[] {
    return resolveVersions(this.getVersionsForQuery(queryId));
  }

  private async pruneIfNeeded(queryId: string, projectId: string): Promise<void> {
    const { getStorage } = await import("$lib/storage");
    const limitStr = await getStorage().appState.get("query_version_limit");
    const limit = limitStr ? parseInt(limitStr, 10) : 100;
    await this.persistence.pruneQueryVersions(queryId, limit);

    // Also update state to reflect pruning
    const versions = this.state.queryVersionsByProject[projectId] ?? [];
    const queryVersions = versions.filter((v) => v.queryId === queryId);
    if (queryVersions.length > limit) {
      const sorted = [...queryVersions].sort((a, b) => b.version - a.version);
      const keepIds = new Set(sorted.slice(0, limit).map((v) => v.id));
      const filtered = versions.filter((v) => v.queryId !== queryId || keepIds.has(v.id));
      this.state.queryVersionsByProject = {
        ...this.state.queryVersionsByProject,
        [projectId]: filtered,
      };
    }
  }
}
