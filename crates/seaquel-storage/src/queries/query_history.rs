//! `queryHistoryRepo`: `query_history`.

use seaquel_types::storage::PersistedQueryHistoryItem;

use super::codec::{begin, bit, flag, insert_sql, json_or, json_text, number, text, Result};
use crate::Storage;

const COLUMNS: [&str; 9] = [
    "id",
    "query",
    "timestamp",
    "execution_time",
    "row_count",
    "connection_id",
    "favorite",
    "connection_labels_snapshot",
    "connection_name_snapshot",
];

/// A connection's history, newest first.
pub async fn load_by_connection(
    st: &Storage,
    connection_id: &str,
) -> Result<Vec<PersistedQueryHistoryItem>> {
    let rows =
        sqlx::query("SELECT * FROM query_history WHERE connection_id = ? ORDER BY timestamp DESC")
            .bind(connection_id)
            .fetch_all(st.pool())
            .await?;
    rows.iter()
        .map(|row| {
            Ok(PersistedQueryHistoryItem {
                id: text(row, "id")?,
                query: text(row, "query")?,
                timestamp: text(row, "timestamp")?,
                execution_time: number(row, "execution_time")?,
                row_count: number(row, "row_count")?,
                connection_id: text(row, "connection_id")?,
                favorite: flag(row, "favorite")?,
                connection_labels_snapshot: Some(json_or(row, "connection_labels_snapshot", "[]")?),
                connection_name_snapshot: text(row, "connection_name_snapshot")?,
            })
        })
        .collect()
}

/// Replaces a connection's history with `items`, in one transaction.
pub async fn replace_all(
    st: &Storage,
    connection_id: &str,
    items: &[PersistedQueryHistoryItem],
) -> Result<()> {
    let mut tx = begin(st).await?;
    sqlx::query("DELETE FROM query_history WHERE connection_id = ?")
        .bind(connection_id)
        .execute(&mut *tx)
        .await?;
    let insert = insert_sql("query_history", &COLUMNS);
    for h in items {
        sqlx::query(&insert)
            .bind(&h.id)
            .bind(&h.query)
            .bind(&h.timestamp)
            .bind(h.execution_time)
            .bind(h.row_count)
            .bind(&h.connection_id)
            .bind(bit(h.favorite))
            .bind(json_text(&h.connection_labels_snapshot))
            .bind(&h.connection_name_snapshot)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Deletes a connection's history.
pub async fn remove_by_connection(st: &Storage, connection_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM query_history WHERE connection_id = ?")
        .bind(connection_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
