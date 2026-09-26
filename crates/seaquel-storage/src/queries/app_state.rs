//! `appStateRepo`: the `app_state` key/value table.

use super::codec::Result;
use crate::Storage;

/// The value for `key`: `None` when there's no row or its value is NULL.
pub async fn get(st: &Storage, key: &str) -> Result<Option<String>> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("SELECT value FROM app_state WHERE key = ?")
            .bind(key)
            .fetch_optional(st.pool())
            .await?;
    Ok(row.and_then(|r| r.0))
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
