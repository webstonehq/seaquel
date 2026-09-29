//! `appStateRepo`: the `app_state` key/value table.

use super::codec::Result;
use crate::{Reader, Storage, WriteTx};

/// The value for `key`: `None` when there's no row or its value is NULL.
/// Reads on the pool (`&storage`) or inside a write (`&mut tx`).
pub async fn get(r: impl Into<Reader<'_>>, key: &str) -> Result<Option<String>> {
    let mut conn = r.into().conn().await?;
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM app_state WHERE key = ?")
            .bind(key)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(row.and_then(|r| r.0))
}

/// [`set`] inside a write transaction.
pub async fn set_in(tx: &mut WriteTx, key: &str, value: Option<&str>) -> Result<()> {
    set_with(tx.conn(), key, value).await
}

/// Sets `key`. `None` keeps a row whose value is NULL.
pub async fn set(st: &Storage, key: &str, value: Option<&str>) -> Result<()> {
    set_with(st.pool(), key, value).await
}

pub(crate) async fn set_with<'e, E>(db: E, key: &str, value: Option<&str>) -> Result<()>
where
    E: sqlx::SqliteExecutor<'e>,
{
    sqlx::query("INSERT OR REPLACE INTO app_state (key, value) VALUES (?, ?)")
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
    Ok(())
}
