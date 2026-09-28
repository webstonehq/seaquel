import type { SqliteDatabase } from "../sqlite-types";
import { createRepo, col, bool, json } from "../create-repo";
import type { PersistedQueryHistoryItem } from "$lib/types";
import { HISTORY_KEEP } from "../client";

const _historyRepo = createRepo<PersistedQueryHistoryItem>({
  table: "query_history",
  id: "id",
  columns: {
    id: col("id"),
    query: col("query"),
    timestamp: col("timestamp"),
    executionTime: col("execution_time"),
    rowCount: col("row_count"),
    connectionId: col("connection_id"),
    favorite: bool("favorite"),
    connectionLabelsSnapshot: json("connection_labels_snapshot", []),
    connectionNameSnapshot: col("connection_name_snapshot"),
  },
});

export const queryHistoryRepo = {
  async loadByConnection(
    db: SqliteDatabase,
    connectionId: string,
  ): Promise<PersistedQueryHistoryItem[]> {
    const rows = await db.query(
      `SELECT * FROM query_history WHERE connection_id = ? ORDER BY timestamp DESC, rowid DESC`,
      [connectionId],
    );
    return rows.map((r) => _historyRepo.mapRow(r as Record<string, unknown>));
  },

  /**
   * `seaquel_storage::query_history::append` for the demo: adds the row and
   * removes the connection's non-favourite rows past the newest
   * `HISTORY_KEEP`, in one transaction.
   */
  async append(db: SqliteDatabase, item: PersistedQueryHistoryItem): Promise<void> {
    await db.transaction([
      { sql: _historyRepo.insertSql, params: _historyRepo.toParams(item) },
      {
        sql: `DELETE FROM query_history WHERE connection_id = ?1 AND favorite IS NOT 1 AND rowid IN (
                SELECT rowid FROM query_history WHERE connection_id = ?1
                ORDER BY timestamp DESC, rowid DESC LIMIT -1 OFFSET ?2)`,
        params: [item.connectionId, HISTORY_KEEP],
      },
    ]);
  },

  /** Sets (not toggles) a row's favourite flag; an unknown id changes nothing. */
  async setFavorite(db: SqliteDatabase, id: string, favorite: boolean): Promise<void> {
    await db.execute("UPDATE query_history SET favorite = ? WHERE id = ?", [favorite ? 1 : 0, id]);
  },

  async removeByConnection(db: SqliteDatabase, connectionId: string): Promise<void> {
    await _historyRepo.removeBy(db, "connection_id = ?", [connectionId]);
  },
};
