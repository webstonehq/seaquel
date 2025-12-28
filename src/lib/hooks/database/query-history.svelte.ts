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

    const queryHistory = this.state.queryHistoryByConnection.get(this.state.activeConnectionId) || [];
    const newQueryHistory = new Map(this.state.queryHistoryByConnection);
    newQueryHistory.set(this.state.activeConnectionId, [
      {
        id: `hist-${Date.now()}`,
        query,
        timestamp: new Date(),
        executionTime: results.executionTime,
        rowCount: results.affectedRows ?? results.totalRows,
        connectionId: this.state.activeConnectionId,
        favorite: false,
      },
      ...queryHistory,
    ]);
    this.state.queryHistoryByConnection = newQueryHistory;
    this.schedulePersistence(this.state.activeConnectionId);
  }

  /**
   * Toggle the favorite status of a history item.
   */
  toggleQueryFavorite(id: string) {
    if (!this.state.activeConnectionId) return;

    const queryHistory =
      this.state.queryHistoryByConnection.get(this.state.activeConnectionId) || [];
    const item = queryHistory.find((h: QueryHistoryItem) => h.id === id);
    if (item) {
      item.favorite = !item.favorite;
      const newQueryHistory = new Map(this.state.queryHistoryByConnection);
      newQueryHistory.set(this.state.activeConnectionId, [...queryHistory]);
      this.state.queryHistoryByConnection = newQueryHistory;
      this.schedulePersistence(this.state.activeConnectionId);
    }
  }
}
