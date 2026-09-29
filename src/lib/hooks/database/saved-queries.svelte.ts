import type { Query, QueryTab, QueryParameter, QueryVersion } from "$lib/types";
import type { ResolvedQueryVersion } from "$lib/types";
import { resolveVersions } from "$lib/utils/query-versions";
import type { DatabaseState } from "./state.svelte.js";
import type { PersistenceManager } from "./persistence-manager.svelte.js";
import { queryNameToFilename } from "$lib/services/query-file-parser";
import { extractErrorMessage } from "$lib/errors";
import { errorToast } from "$lib/utils/toast";
import { m } from "$lib/paraglide/messages.js";
import {
  NEW,
  getLibrary,
  rowKey,
  type ChangeSeq,
  type SavedQueryUpdated,
  type WireQueryVersion,
  type WireSavedQuery,
} from "./library/index.js";
import {
  isEmptyPatch,
  queryVersionFromWire,
  savedQueryFromWire,
  savedQueryPatch,
  type SavedQueryFields,
} from "./library/convert.js";
import { libraryError } from "./library/messages.js";
import { libraryNameOf, refreshQueryVersions } from "./library/view.js";

/**
 * Manages queries (both local and shared): save, delete, share, unshare.
 * Queries are per-project. The library (Core on desktop and web) is the
 * source of truth: every change is one targeted call, written at once, and
 * Core numbers and prunes the versions. When shared=true, a .sql file is
 * maintained as a git projection.
 */
export class SavedQueryManager {
  private removeTab: ((id: string) => void) | null = null;
  private writeQueryFile: ((query: Query) => Promise<void>) | null = null;
  private deleteQueryFile: ((query: Query) => Promise<void>) | null = null;

  /**
   * Saved queries are stored through the library, one targeted call per
   * change (phase 5d-1). `scheduleProjectPersistence` saves the tabs a
   * change links or renames.
   */
  constructor(
    private state: DatabaseState,
    private scheduleProjectPersistence: (projectId: string | null) => void,
    _persistence?: PersistenceManager,
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

  /**
   * Save the query a tab holds: a tab linked to a saved query updates it
   * with what changed (Core appends a keyframe of the previous text when
   * the text changed, and prunes; Decision 11); otherwise, or with
   * `forceNew`, a new saved query is created with Core's id and the tab is
   * linked to it. Returns the query's id; a refusal (`NAME_TAKEN`, a query
   * deleted elsewhere) throws, worded for the user, and changes nothing.
   */
  async saveQuery(
    name: string,
    query: string,
    tabId?: string,
    parameters?: QueryParameter[],
    forceNew?: boolean,
  ): Promise<string | null> {
    if (!this.state.activeProjectId) return null;

    const projectId = this.state.activeProjectId;

    // Check if this tab is already linked to a query
    let existingQueryId: string | undefined;
    if (tabId && !forceNew) {
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      const tab = tabs.find((t: QueryTab) => t.id === tabId);
      existingQueryId = tab?.queryId;
    }

    const existingQuery = existingQueryId
      ? (this.state.queriesByProject[projectId] ?? []).find((q) => q.id === existingQueryId)
      : undefined;
    if (existingQuery) {
      const updated = await this.update(projectId, existingQuery, { name, query, parameters });

      // If shared, also update the .sql file
      if (existingQuery.shared && updated) {
        this.projectRenamedFile(existingQuery, updated).catch((err) =>
          errorToast(m.shared_query_save_failed({ message: extractErrorMessage(err) })),
        );
      }

      // Also update tab name if it differs
      if (tabId) this.renameTab(projectId, tabId, name);
      return existingQuery.id;
    }

    // Create new query
    let created: Query;
    try {
      const { value, seq } = await this.state.librarySeqs.write([rowKey("savedQuery", NEW)], () =>
        getLibrary().createSavedQuery({
          projectId,
          name,
          query,
          ...(parameters ? { parameters } : {}),
        }),
      );
      this.state.librarySeqs.note(rowKey("savedQuery", value.id), seq);
      created = savedQueryFromWire(value);
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
    this.show(projectId, created);

    // Link tab to query if tabId provided (skip for forceNew — caller handles it)
    if (tabId && !forceNew) {
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      const updatedTabs = tabs.map((t: QueryTab) =>
        t.id === tabId ? { ...t, queryId: created.id, name } : t,
      );
      this.state.queryTabsByProject = {
        ...this.state.queryTabsByProject,
        [projectId]: updatedTabs,
      };
      this.scheduleProjectPersistence(projectId);
    }

    return created.id;
  }

  /** The names the page holds, for a `NAME_TAKEN` message. */
  private nameOf = (id: string) => libraryNameOf(this.state, id);

  /** Show saved query `query` in its project's list (replacing or adding). */
  private show(projectId: string, query: Query): void {
    const queries = this.state.queriesByProject[projectId] ?? [];
    this.state.queriesByProject = {
      ...this.state.queriesByProject,
      [projectId]: queries.some((q) => q.id === query.id)
        ? queries.map((q) => (q.id === query.id ? query : q))
        : [...queries, query],
    };
  }

  /** Rename tab `tabId` to `name` if it's called something else. */
  private renameTab(projectId: string, tabId: string, name: string): void {
    const tabs = this.state.queryTabsByProject[projectId] ?? [];
    const tab = tabs.find((t: QueryTab) => t.id === tabId);
    if (tab && tab.name !== name) {
      this.state.queryTabsByProject = {
        ...this.state.queryTabsByProject,
        [projectId]: tabs.map((t: QueryTab) => (t.id === tabId ? { ...t, name } : t)),
      };
      this.scheduleProjectPersistence(projectId);
    }
  }

  /**
   * Store `changes` to saved query `before` (only the fields that differ)
   * and show Core's answer: the row, the version it appended and the ones
   * it pruned. Returns the updated query, or `before` when nothing changed.
   */
  private async update(
    projectId: string,
    before: Query,
    changes: Partial<SavedQueryFields>,
  ): Promise<Query> {
    const patch = savedQueryPatch(before, { ...before, ...changes });
    if (isEmptyPatch(patch)) return before;
    let result: SavedQueryUpdated;
    try {
      const { value, seq } = await this.state.librarySeqs.write(
        [rowKey("savedQuery", before.id)],
        () => getLibrary().updateSavedQuery(before.id, patch),
      );
      this.state.librarySeqs.note(rowKey("savedQuery", before.id), seq);
      result = value;
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }
    const updated = savedQueryFromWire(result.query);
    this.show(projectId, updated);
    if (result.version || result.prunedVersionIds.length > 0) {
      const pruned = new Set(result.prunedVersionIds);
      const versions = (this.state.queryVersionsByProject[projectId] ?? []).filter(
        (v) => !pruned.has(v.id),
      );
      if (result.version) versions.push(queryVersionFromWire(result.version));
      // Shown at once, then the whole list read again: its `seq` is recorded
      // only with the whole list (C1).
      this.state.queryVersionsByProject = {
        ...this.state.queryVersionsByProject,
        [projectId]: versions,
      };
      await refreshQueryVersions(this.state, projectId);
    }
    return updated;
  }

  /**
   * Delete a saved query (its versions go with it) and close the tabs
   * linked to it. A shared query's `.sql` file goes too. A refusal throws,
   * worded for the user, and changes nothing.
   */
  async deleteQuery(id: string): Promise<void> {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const queries = this.state.queriesByProject[projectId] ?? [];
    const query = queries.find((q) => q.id === id);

    try {
      const { seq } = await this.state.librarySeqs.write([rowKey("savedQuery", id)], () =>
        getLibrary().removeSavedQuery(id),
      );
      this.state.librarySeqs.note(rowKey("savedQuery", id), seq);
    } catch (error) {
      throw libraryError(error, this.nameOf);
    }

    // If shared, delete the .sql file too
    if (query?.shared) {
      this.deleteQueryFile?.(query)?.catch((err) =>
        console.error("[saved-queries] Failed to delete shared query file:", err),
      );
    }

    this.forget(projectId, id);

    // Close any tabs linked to this query
    const tabs = this.state.queryTabsByProject[projectId] ?? [];
    for (const tab of tabs) {
      if (tab.queryId === id && this.removeTab) {
        this.removeTab(tab.id);
      }
    }
  }

  /** Take saved query `id` and its versions out of the page. */
  private forget(projectId: string, id: string): void {
    this.state.queriesByProject = {
      ...this.state.queriesByProject,
      [projectId]: (this.state.queriesByProject[projectId] ?? []).filter((q) => q.id !== id),
    };
    this.state.queryVersionsByProject = {
      ...this.state.queryVersionsByProject,
      [projectId]: (this.state.queryVersionsByProject[projectId] ?? []).filter(
        (v) => v.queryId !== id,
      ),
    };
  }

  /**
   * Rename a saved query (a linked tab's rename). A shared query's file is
   * named after the query, so its old file goes and the new one is written.
   */
  async renameQuery(id: string, name: string): Promise<void> {
    const projectId = this.state.activeProjectId;
    if (!projectId) return;
    const queries = this.state.queriesByProject[projectId] ?? [];
    const query = queries.find((q) => q.id === id);
    if (!query) return;

    const updated = await this.update(projectId, query, { name });
    if (query.shared) await this.projectRenamedFile(query, updated);
  }

  /**
   * Write a shared query's `.sql` file after a change, then delete the file
   * under its old name if the change moved it. Left behind, the old file
   * would come back as a second query at the next reconcile. The new file is
   * written first so a failure never leaves the query with no file, and the
   * paths are compared ignoring case: on a case-insensitive disk a rename
   * that only changes case is the same file, and deleting it would lose it.
   */
  private async projectRenamedFile(before: Query, after: Query): Promise<void> {
    await this.writeQueryFile?.(after);
    const path = (q: Query) => `${q.folder ?? ""}/${queryNameToFilename(q.name)}`.toLowerCase();
    if (path(before) !== path(after)) {
      await this.deleteQueryFile?.(before);
    }
  }

  /** @deprecated Use deleteQuery instead */
  deleteSavedQuery(id: string): Promise<void> {
    return this.deleteQuery(id);
  }

  /** Star or unstar a saved query (its `updatedAt` stays; Decision 11). */
  async toggleQueryStarred(id: string): Promise<void> {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const query = (this.state.queriesByProject[projectId] ?? []).find((q) => q.id === id);
    if (!query) return;
    await this.update(projectId, query, { starred: !query.starred });
  }

  /** @deprecated Use toggleQueryStarred instead */
  toggleSavedQueryStarred(id: string): Promise<void> {
    return this.toggleQueryStarred(id);
  }

  /** @deprecated Use toggleQueryStarred instead */
  toggleSharedQueryStarred(id: string): Promise<void> {
    return this.toggleQueryStarred(id);
  }

  /**
   * Share a query: store shared=true, then write the .sql file. The flag is
   * stored first, so a failed file write can't leave it unsaved.
   */
  async shareQuery(queryId: string): Promise<void> {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const queries = this.state.queriesByProject[projectId] ?? [];
    const query = queries.find((q) => q.id === queryId);
    if (!query || query.shared) return;

    const updatedQuery = await this.update(projectId, query, { shared: true });

    // Write the .sql file
    await this.writeQueryFile?.(updatedQuery);
  }

  /**
   * Unshare a query: delete the .sql file, then store shared=false.
   */
  async unshareQuery(queryId: string): Promise<void> {
    if (!this.state.activeProjectId) return;

    const projectId = this.state.activeProjectId;
    const queries = this.state.queriesByProject[projectId] ?? [];
    const query = queries.find((q) => q.id === queryId);
    if (!query || !query.shared) return;

    // Delete the .sql file first. If that fails nothing has changed yet.
    await this.deleteQueryFile?.(query);

    await this.update(projectId, query, { shared: false });
  }

  // -------- Other windows' changes (phase 5d-1) --------

  /**
   * Apply a `savedQueriesList` of `projectId` taken at `seq` to the queries
   * `ids` names (all when `null`), each only if `seq` is newer (Decision
   * 17). A query the list lacks was deleted: it goes, and a tab linked to
   * it keeps its text but loses the link.
   */
  applySavedQueries(
    projectId: string,
    rows: readonly WireSavedQuery[],
    seq: ChangeSeq,
    ids: readonly string[] | null,
  ): void {
    const seqs = this.state.librarySeqs;
    const byId = new Map(rows.map((r) => [r.id, r]));
    const shown = this.state.queriesByProject[projectId] ?? [];
    const scope = new Set(ids ?? [...shown.map((q) => q.id), ...byId.keys()]);
    let next = [...shown];
    const removed: string[] = [];
    for (const id of scope) {
      if (!seqs.take(rowKey("savedQuery", id), seq)) continue;
      const row = byId.get(id);
      if (row) {
        const query = savedQueryFromWire(row);
        next = next.some((q) => q.id === id)
          ? next.map((q) => (q.id === id ? query : q))
          : [...next, query];
      } else if (next.some((q) => q.id === id)) {
        next = next.filter((q) => q.id !== id);
        removed.push(id);
      }
    }
    this.state.queriesByProject = { ...this.state.queriesByProject, [projectId]: next };
    if (removed.length > 0) {
      const gone = new Set(removed);
      const tabs = this.state.queryTabsByProject[projectId] ?? [];
      if (tabs.some((t) => t.queryId && gone.has(t.queryId))) {
        this.state.queryTabsByProject = {
          ...this.state.queryTabsByProject,
          [projectId]: tabs.map((t) =>
            t.queryId && gone.has(t.queryId) ? { ...t, queryId: undefined } : t,
          ),
        };
        this.scheduleProjectPersistence(projectId);
      }
    }
  }

  /** Apply a `queryVersionsList` of `projectId` taken at `seq`, if it is newer. */
  applyQueryVersions(projectId: string, rows: readonly WireQueryVersion[], seq: ChangeSeq): void {
    if (!this.state.librarySeqs.take(versionsKey(projectId), seq)) return;
    this.state.queryVersionsByProject = {
      ...this.state.queryVersionsByProject,
      [projectId]: rows.map(queryVersionFromWire),
    };
  }

  /**
   * Refetch a project's saved queries (the ones `ids` names, or all) and
   * their versions after another window's change, once this page's own
   * writes to them have answered. A project whose queries the page hasn't
   * loaded is left alone: it loads them when it's opened.
   */
  async refreshFromLibrary(projectId: string, ids: readonly string[] | null): Promise<void> {
    if (!(projectId in this.state.queriesByProject)) return;
    await this.state.librarySeqs.settled("savedQuery:");
    const library = getLibrary();
    const [queries, versions] = await Promise.all([
      library.listSavedQueries(projectId),
      library.listQueryVersions(projectId),
    ]);
    this.applySavedQueries(projectId, queries.value, queries.seq, ids);
    this.applyQueryVersions(projectId, versions.value, versions.seq);
  }

  getVersionsForQuery(queryId: string): QueryVersion[] {
    if (!this.state.activeProjectId) return [];
    const versions = this.state.queryVersionsByProject[this.state.activeProjectId] ?? [];
    return versions.filter((v) => v.queryId === queryId);
  }

  getResolvedVersionsForQuery(queryId: string): ResolvedQueryVersion[] {
    return resolveVersions(this.getVersionsForQuery(queryId));
  }
}

/** The `seq` key of a project's version list. */
function versionsKey(projectId: string): string {
  return rowKey("queryVersion", projectId);
}
