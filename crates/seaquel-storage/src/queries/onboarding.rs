//! `onboardingRepo`: `onboarding_state`, one row holding JSON.

use crate::db;
use serde_json::value::RawValue;

use super::codec::{load_singleton_json, parse_json, save_singleton_json, Result};
use crate::{Reader, Storage, WriteTx};

const TABLE: &str = "onboarding_state";

/// The onboarding state as its stored JSON. `None` (JSON `null`) when there's no
/// row or the stored text doesn't parse; stored `null` loads as `null` too.
pub async fn load(st: &Storage) -> Result<Option<Box<RawValue>>> {
    load_singleton_json(st, TABLE).await
}

/// Stores `data` as the JSON given (`null` is stored as `'null'`).
pub async fn save(st: &Storage, data: &RawValue) -> Result<()> {
    save_singleton_json(st, TABLE, data).await
}

/// [`load`] on the pool or inside a write: `None` when there's no row or
/// the stored text doesn't read (not UTF-8, not JSON); stored `null` reads
/// as JSON `null`. Core reads a record that isn't an object as the
/// defaults.
pub async fn get(r: impl Into<Reader<'_>>) -> Result<Option<Box<RawValue>>> {
    let mut conn = r.into().conn().await?;
    let row: Option<(Option<Vec<u8>>,)> =
        db::query_as("SELECT CAST(data AS BLOB) FROM onboarding_state WHERE id = 1")
            .fetch_optional(&mut *conn)
            .await?;
    Ok(row
        .and_then(|(data,)| String::from_utf8(data?).ok())
        .and_then(|text| parse_json(&text)))
}

/// Stores `data` as the JSON given, inside a write transaction.
pub async fn set(tx: &mut WriteTx, data: &RawValue) -> Result<()> {
    db::query("INSERT OR REPLACE INTO onboarding_state (id, data) VALUES (1, ?)")
        .bind(data.get())
        .execute(tx.conn())
        .await?;
    Ok(())
}
