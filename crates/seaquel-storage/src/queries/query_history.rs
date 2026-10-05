//! `queryHistoryRepo`: `query_history`.

use crate::db;
use seaquel_types::storage::PersistedQueryHistoryItem;
use seaquel_types::Value;

use super::codec::{
    begin, bit, encode_error, flag, insert_sql, json_or, json_text, number, opt_text, text, Result,
};
use crate::Storage;

const COLUMNS: [&str; 10] = [
    "id",
    "query",
    "timestamp",
    "execution_time",
    "row_count",
    "connection_id",
    "favorite",
    "connection_labels_snapshot",
    "connection_name_snapshot",
    "params",
];

/// How many of a connection's newest rows [`append`] keeps, favourites
/// counted. Favourites past them are kept too. This is the rule
/// `serializeQueryHistory` applied before every whole-list save, so the
/// first append on an existing file removes what that save would have,
/// with one accepted difference: rows with the same `timestamp` rank by
/// `rowid DESC` (the one appended last is newer). `replace_all` inserted
/// newest first, so in a file it wrote the lower rowid is the newer row,
/// and on a tie straddling the cap the prune keeps the older of the two
/// where the old serializer kept the newer. Only millisecond-equal
/// timestamps at exactly the 500th place are affected.
pub const HISTORY_KEEP: usize = 500;

/// A connection's history, newest first: by `timestamp`, then the row
/// appended last, the order [`append`]'s cap ranks by.
pub async fn load_by_connection(
    st: &Storage,
    connection_id: &str,
) -> Result<Vec<PersistedQueryHistoryItem>> {
    let rows = db::query(
        "SELECT * FROM query_history WHERE connection_id = ? ORDER BY timestamp DESC, rowid DESC",
    )
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
                params: read_params(opt_text(row, "params")?),
            })
        })
        .collect()
}

/// `query_history.params` as stored (migration `0006`): a JSON array of
/// values in the cell wire format. NULL, and anything that doesn't read as
/// such an array (a hand-edited file), is `None`: the row loads without its
/// values rather than failing the whole list.
fn read_params(stored: Option<String>) -> Option<Vec<Value>> {
    serde_json::from_str(&stored?).ok()
}

/// What [`read_params`] reads: `None` and an empty list store NULL.
fn params_text(params: &Option<Vec<Value>>) -> Result<Option<String>> {
    match params {
        Some(values) if !values.is_empty() => serde_json::to_string(values)
            .map(Some)
            // The message is serde's, which never quotes a value.
            .map_err(|e| encode_error(format!("query_history.params: {e}"))),
        _ => Ok(None),
    }
}

/// Adds one row and removes the connection's non-favourite rows past the
/// newest [`HISTORY_KEEP`] (by `timestamp`, then insertion order), in one
/// transaction. Fails, removing nothing, when the connection isn't saved
/// (the foreign key) or the id is taken.
pub async fn append(st: &Storage, item: &PersistedQueryHistoryItem) -> Result<()> {
    let params = params_text(&item.params)?;
    let mut tx = begin(st).await?;
    bind_item(
        db::query(&insert_sql("query_history", &COLUMNS)),
        item,
        params,
    )
    .execute(&mut *tx)
    .await?;
    // `favorite IS NOT 1` is "not a favourite" as `flag` reads it.
    db::query(
        "DELETE FROM query_history WHERE connection_id = ?1 AND favorite IS NOT 1 AND rowid IN (\
           SELECT rowid FROM query_history WHERE connection_id = ?1 \
           ORDER BY timestamp DESC, rowid DESC LIMIT -1 OFFSET ?2)",
    )
    .bind(&item.connection_id)
    .bind(HISTORY_KEEP as i64)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Adds `items` in order and prunes each connection they name once, as
/// [`append`] does, all in one transaction: every row or none. An applied
/// batch of pending changes records one row per change this way (phase 5c).
/// Nothing to add is a no-op. Fails, adding nothing, when a
/// connection isn't saved or an id is taken.
pub async fn append_many(st: &Storage, items: &[PersistedQueryHistoryItem]) -> Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    let params = items
        .iter()
        .map(|item| params_text(&item.params))
        .collect::<Result<Vec<_>>>()?;
    let mut tx = begin(st).await?;
    let insert = insert_sql("query_history", &COLUMNS);
    for (item, params) in items.iter().zip(params) {
        bind_item(db::query(&insert), item, params)
            .execute(&mut *tx)
            .await?;
    }
    let mut pruned: Vec<&str> = Vec::new();
    for item in items {
        let connection_id = item.connection_id.as_str();
        if pruned.contains(&connection_id) {
            continue;
        }
        pruned.push(connection_id);
        db::query(
            "DELETE FROM query_history WHERE connection_id = ?1 AND favorite IS NOT 1 AND rowid IN (\
               SELECT rowid FROM query_history WHERE connection_id = ?1 \
               ORDER BY timestamp DESC, rowid DESC LIMIT -1 OFFSET ?2)",
        )
        .bind(connection_id)
        .bind(HISTORY_KEEP as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Sets (not toggles) a row's favourite flag, so two writes queued in
/// either order agree. An unknown id changes nothing.
pub async fn set_favorite(st: &Storage, id: &str, favorite: bool) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query("UPDATE query_history SET favorite = ? WHERE id = ?")
        .bind(bit(favorite))
        .bind(id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(())
}

type Query<'q> = crate::db::SqliteQuery<'q>;

/// Binds `h` in [`COLUMNS`]' order, with `params` as [`params_text`] wrote it.
fn bind_item<'q>(
    q: Query<'q>,
    h: &'q PersistedQueryHistoryItem,
    params: Option<String>,
) -> Query<'q> {
    q.bind(&h.id)
        .bind(&h.query)
        .bind(&h.timestamp)
        .bind(h.execution_time)
        .bind(h.row_count)
        .bind(&h.connection_id)
        .bind(bit(h.favorite))
        .bind(json_text(&h.connection_labels_snapshot))
        .bind(&h.connection_name_snapshot)
        .bind(params)
}

/// Replaces a connection's history with `items`, in one transaction. The
/// app no longer calls it (phase 5b); it stays for the frozen repo fixtures.
pub async fn replace_all(
    st: &Storage,
    connection_id: &str,
    items: &[PersistedQueryHistoryItem],
) -> Result<()> {
    let params = items
        .iter()
        .map(|h| params_text(&h.params))
        .collect::<Result<Vec<_>>>()?;
    let mut tx = begin(st).await?;
    db::query("DELETE FROM query_history WHERE connection_id = ?")
        .bind(connection_id)
        .execute(&mut *tx)
        .await?;
    let insert = insert_sql("query_history", &COLUMNS);
    for (h, params) in items.iter().zip(params) {
        bind_item(db::query(&insert), h, params)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Deletes a connection's history.
pub async fn remove_by_connection(st: &Storage, connection_id: &str) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query("DELETE FROM query_history WHERE connection_id = ?")
        .bind(connection_id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(())
}
