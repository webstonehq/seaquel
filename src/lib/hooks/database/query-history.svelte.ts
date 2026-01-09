import type { QueryResult, QueryHistoryItem } from "$lib/types";
import type { DatabaseState } from "./state.svelte.js";

/**
 * Manages query history: adding entries, toggling favorites.
 * Note: loadQueryFromHistory is in UseDatabase as it orchestrates multiple services.
 */
export class QueryHistoryManager {
  constructor(
    private state: DatabaseState,
    private schedulePersistence: (connectionId: string | null) => void
  ) {}

  /**
   * Add a query to the history for the active connection.
   */
  addToHistory(query: string, results: QueryResult) {
    if (!this.state.activeConnectionId) return;

    const connectionId = this.state.activeConnectionId;
    const queryHistory = this.state.queryHistoryByConnection[connectionId] ?? [];

    this.state.queryHistoryByConnection = {
      ...this.state.queryHistoryByConnection,
      [connectionId]: [
        {
          id: `hist-${Date.now()}`,
          query,
          timestamp: new Date(),
          executionTime: results.executionTime,
          rowCount: results.affectedRows ?? results.totalRows,
          connectionId,
          favorite: false,
        },
        ...queryHistory,
      ],
    };
    this.schedulePersistence(connectionId);
  }

  /**
   * Toggle the favorite status of a history item.
   */
  toggleQueryFavorite(id: string) {
    if (!this.state.activeConnectionId) return;

    const connectionId = this.state.activeConnectionId;
    const queryHistory = this.state.queryHistoryByConnection[connectionId] ?? [];
    const item = queryHistory.find((h: QueryHistoryItem) => h.id === id);

    if (item) {
      const updatedHistory = queryHistory.map((h) =>
        h.id === id ? { ...h, favorite: !h.favorite } : h
      );
      this.state.queryHistoryByConnection = {
        ...this.state.queryHistoryByConnection,
        [connectionId]: updatedHistory,
      };
      this.schedulePersistence(connectionId);
    }
  }
}
