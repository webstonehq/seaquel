//! `appStateRepo`: the `app_state` key/value table.

use super::codec::Result;
use crate::db;
use crate::{Reader, Storage, WriteTx};

/// The value for `key`: `None` when there's no row or its value is NULL.
/// Reads on the pool (`&storage`) or inside a write (`&mut tx`).
pub async fn get(r: impl Into<Reader<'_>>, key: &str) -> Result<Option<String>> {
    let mut conn = r.into().conn().await?;
    let row: Option<(Option<String>,)> = db::query_as("SELECT value FROM app_state WHERE key = ?")
        .bind(key)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.and_then(|r| r.0))
}

/// [`set`] inside a write transaction.
pub async fn set_in(tx: &mut WriteTx, key: &str, value: Option<&str>) -> Result<()> {
    set_with(tx.conn(), key, value).await
}

/// Deletes `key`'s row inside a write transaction (phase 5d-2: a setting
/// set to `null` has no row, where [`set`] with `None` keeps one holding
/// NULL; loads read both as unset). `false` when there was no row.
pub async fn delete_in(tx: &mut WriteTx, key: &str) -> Result<bool> {
    let done = db::query("DELETE FROM app_state WHERE key = ?")
        .bind(key)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Sets `key`. `None` keeps a row whose value is NULL.
pub async fn set(st: &Storage, key: &str, value: Option<&str>) -> Result<()> {
    let mut tx = super::codec::begin(st).await?;
    set_with(tx.conn(), key, value).await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn set_with<'e, E>(db: E, key: &str, value: Option<&str>) -> Result<()>
where
    E: db::SqliteExecutor<'e>,
{
    db::query("INSERT OR REPLACE INTO app_state (key, value) VALUES (?, ?)")
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    Ok(())
}
