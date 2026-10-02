//! `importStateRepo`: `import_state`, per import source.

use crate::db;
use seaquel_types::storage::ImportState;

use super::codec::{bit, flag, opt_text, Result};
use crate::{Reader, Storage, WriteTx};

/// The state for one source, or `None` before it's first saved.
pub async fn load(st: &Storage, source: &str) -> Result<Option<ImportState>> {
    get(st, source).await
}

/// [`load`] on the pool or inside a write.
pub async fn get(r: impl Into<Reader<'_>>, source: &str) -> Result<Option<ImportState>> {
    let mut conn = r.into().conn().await?;
    let row = db::query(
        "SELECT has_offered_import, last_check_timestamp FROM import_state WHERE source = ?",
    )
    .bind(source)
    .fetch_optional(&mut *conn)
    .await?;
    row.map(|row| {
        Ok(ImportState {
            has_offered_import: flag(&row, "has_offered_import")?,
            last_check_timestamp: opt_text(&row, "last_check_timestamp")?,
        })
    })
    .transpose()
}

pub async fn save(
    st: &Storage,
    source: &str,
    has_offered_import: bool,
    last_check_timestamp: Option<&str>,
) -> Result<()> {
    db::query(
        "INSERT OR REPLACE INTO import_state (source, has_offered_import, last_check_timestamp) \
         VALUES (?, ?, ?)",
    )
    .bind(source)
    .bind(bit(has_offered_import))
    .bind(last_check_timestamp)
    .execute(st.pool())
    .await?;
    Ok(())
}

/// [`save`] inside a write transaction.
pub async fn save_in(
    tx: &mut WriteTx,
    source: &str,
    has_offered_import: bool,
    last_check_timestamp: Option<&str>,
) -> Result<()> {
    db::query(
        "INSERT OR REPLACE INTO import_state (source, has_offered_import, last_check_timestamp) \
         VALUES (?, ?, ?)",
    )
    .bind(source)
    .bind(bit(has_offered_import))
    .bind(last_check_timestamp)
    .execute(tx.conn())
    .await?;
    Ok(())
}
