import type { QueryHistoryItem, ConnectionLabel, PersistedQueryHistoryItem } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";
import type { HistoryContext } from "$lib/types/generated/HistoryContext";
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
    ...(h.params?.length ? { params: h.params } : {}),
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
