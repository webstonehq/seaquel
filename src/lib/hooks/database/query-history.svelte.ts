import type {
  QueryResult,
  QueryHistoryItem,
  ConnectionLabel,
  PersistedQueryHistoryItem,
} from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { HistoryContext } from "$lib/types/generated/HistoryContext";
import { licenseNudgeStore } from "$lib/stores/license-nudge.svelte.js";
import { getStorage } from "$lib/storage";
import { HISTORY_KEEP } from "$lib/storage/client";
import { log } from "$lib/utils/logger";
import { extractErrorMessage } from "$lib/errors";

export { HISTORY_KEEP };

/**
 * The storage cap applied to the in-memory list (newest first): the first
 * `HISTORY_KEEP`, then the favourites past them. The same rule
 * `queryHistory.append` applies to the file.
 */
export function trimHistory(list: QueryHistoryItem[]): QueryHistoryItem[] {
  if (list.length <= HISTORY_KEEP) return list;
  return [...list.slice(0, HISTORY_KEEP), ...list.slice(HISTORY_KEEP).filter((h) => h.favorite)];
}

export function fromPersisted(h: PersistedQueryHistoryItem): QueryHistoryItem {
  return {
    id: h.id,
    query: h.query,
    timestamp: new Date(h.timestamp),
    executionTime: h.executionTime,
    rowCount: h.rowCount,
    connectionId: h.connectionId,
    favorite: h.favorite,
    connectionLabelsSnapshot: h.connectionLabelsSnapshot || [],
    connectionNameSnapshot: h.connectionNameSnapshot || "",
  };
}

/**
 * Query history: a read cache of each connection's stored history, written
 * by targeted storage calls (`append`, `setFavorite`). Nothing here replaces
 * a connection's whole list, so a failed load can't wipe stored rows.
 * Note: loadQueryFromHistory is in UseDatabase as it orchestrates multiple services.
 */
export class QueryHistoryManager {
  constructor(
    private state: DatabaseState,
    private getConnectionLabels: (connectionId: string) => ConnectionLabel[],
    private getConnectionName: (connectionId: string) => string,
  ) {}

  /**
   * Add a query to the history for the active connection: appended to
   * storage and put at the top of the cache. Captures a snapshot of the
   * connection's labels and name at execution time.
   */
  addToHistory(query: string, results: QueryResult) {
    if (!this.state.activeConnectionId) return;

    const connectionId = this.state.activeConnectionId;
    const item: PersistedQueryHistoryItem = {
      id: `hist-${crypto.randomUUID()}`,
      query,
      timestamp: new Date().toISOString(),
      executionTime: results.executionTime,
      rowCount: results.affectedRows ?? results.totalRows,
      connectionId,
      favorite: false,
      connectionLabelsSnapshot: this.getConnectionLabels(connectionId).map((l) => ({ ...l })),
      connectionNameSnapshot: this.getConnectionName(connectionId),
    };
    this.insertRecorded(item);
    // The storage client queues writes in the order issued. A row that
    // wasn't stored (an unsaved connection, say) leaves the cache too, so it
    // can't be starred.
    getStorage()
      .queryHistory.append(item)
      .catch((error: unknown) => {
        void log.error(`Failed to append query history: ${extractErrorMessage(error)}`);
        const list = this.state.queryHistoryByConnection[connectionId];
        if (!list?.some((h) => h.id === item.id)) return;
        this.state.queryHistoryByConnection = {
          ...this.state.queryHistoryByConnection,
          [connectionId]: list.filter((h) => h.id !== item.id),
        };
      });
    licenseNudgeStore.recordQuery();
  }

  /**
   * Whose history a run goes in: the saved connection's id, and a snapshot
   * of its name and labels now. Core records the row (`db.run`'s `history`).
   */
  contextFor(connectionId: string): HistoryContext {
    return {
      connectionId,
      connectionName: this.getConnectionName(connectionId),
      connectionLabels: this.getConnectionLabels(connectionId).map((l) => ({ ...l })),
    };
  }

  /**
   * Put a row that is already stored (Core appended it) at the top of its
   * connection's cache, trimmed like the file. Writes nothing.
   */
  insertRecorded(item: PersistedQueryHistoryItem) {
    const list = this.state.queryHistoryByConnection[item.connectionId] ?? [];
    this.state.queryHistoryByConnection = {
      ...this.state.queryHistoryByConnection,
      [item.connectionId]: trimHistory([fromPersisted(item), ...list]),
    };
  }

  /**
   * Toggle the favorite status of a history item.
   */
  toggleQueryFavorite(id: string) {
    if (!this.state.activeConnectionId) return;

    const connectionId = this.state.activeConnectionId;
    const queryHistory = this.state.queryHistoryByConnection[connectionId] ?? [];
    const item = queryHistory.find((h: QueryHistoryItem) => h.id === id);
    if (!item) return;

    const favorite = !item.favorite;
    this.state.queryHistoryByConnection = {
      ...this.state.queryHistoryByConnection,
      [connectionId]: queryHistory.map((h) => (h.id === id ? { ...h, favorite } : h)),
    };
    getStorage()
      .queryHistory.setFavorite(id, favorite)
      .catch((error: unknown) => {
        void log.error(`Failed to save a history favourite: ${extractErrorMessage(error)}`);
      });
  }
}
