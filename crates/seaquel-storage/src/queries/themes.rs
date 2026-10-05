//! `themeRepo`: `theme_preferences` (one row) and `user_themes`, each
//! theme stored as JSON.
//!
//! From phase 5d-2 Core writes one user theme at a time;
//! [`save_user_themes`], which replaces them all, stays for its frozen
//! fixtures.

use crate::db;
use seaquel_types::storage::ThemePreferences;
use serde_json::value::RawValue;

use super::codec::{begin, bind_json_id, is_null, parse_json, stored_json, Result};
use crate::{Reader, Storage, WriteTx};

/// The light and dark theme ids, or `None` before they're first saved.
pub async fn load_preferences(st: &Storage) -> Result<Option<ThemePreferences>> {
    let row: Option<(String, String)> =
        db::query_as("SELECT light_theme_id, dark_theme_id FROM theme_preferences WHERE id = 1")
            .fetch_optional(st.pool())
            .await?;
    Ok(row.map(|(light_theme_id, dark_theme_id)| ThemePreferences {
        light_theme_id,
        dark_theme_id,
    }))
}

pub async fn save_preferences(
    st: &Storage,
    light_theme_id: &str,
    dark_theme_id: &str,
) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query("INSERT OR REPLACE INTO theme_preferences (id, light_theme_id, dark_theme_id) VALUES (1, ?, ?)")
        .bind(light_theme_id)
        .bind(dark_theme_id)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Every user theme, as its stored JSON, in rowid order. Rows that don't
/// parse or hold `null` are skipped; any other JSON (a scalar too) is kept.
pub async fn load_user_themes(st: &Storage) -> Result<Vec<Box<RawValue>>> {
    let rows: Vec<(Option<String>,)> = db::query_as("SELECT data FROM user_themes")
        .fetch_all(st.pool())
        .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(data,)| parse_json(&data?))
        .filter(|t| !is_null(t))
        .collect())
}

/// Replaces every user theme, stored as the JSON given under its `id`, in
/// one transaction.
pub async fn save_user_themes(st: &Storage, themes: &[Box<RawValue>]) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query("DELETE FROM user_themes")
        .execute(&mut *tx)
        .await?;
    for theme in themes {
        let insert = db::query("INSERT INTO user_themes (id, data) VALUES (?, ?)");
        let insert = bind_json_id(insert, theme, None)?;
        insert.bind(theme.get()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// [`load_preferences`] on the pool or inside a write.
pub async fn preferences(r: impl Into<Reader<'_>>) -> Result<Option<ThemePreferences>> {
    let mut conn = r.into().conn().await?;
    type Ids = (Option<Vec<u8>>, Option<Vec<u8>>);
    let row: Option<Ids> = db::query_as(
        "SELECT CAST(light_theme_id AS BLOB), CAST(dark_theme_id AS BLOB) \
         FROM theme_preferences WHERE id = 1",
    )
    .fetch_optional(&mut *conn)
    .await?;
    let text = |b: Option<Vec<u8>>| {
        b.map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    Ok(row.map(|(light, dark)| ThemePreferences {
        light_theme_id: text(light),
        dark_theme_id: text(dark),
    }))
}

/// Sets the light and dark theme ids (the one `theme_preferences` row).
pub async fn set_preferences(
    tx: &mut WriteTx,
    light_theme_id: &str,
    dark_theme_id: &str,
) -> Result<()> {
    db::query(
        "INSERT OR REPLACE INTO theme_preferences (id, light_theme_id, dark_theme_id) \
         VALUES (1, ?, ?)",
    )
    .bind(light_theme_id)
    .bind(dark_theme_id)
    .execute(tx.conn())
    .await?;
    Ok(())
}

/// Every user theme as its stored JSON, in rowid order, without the rows
/// that don't read (not UTF-8, not JSON, `null`), as [`load_user_themes`]
/// skips the ones that don't parse.
pub async fn list(r: impl Into<Reader<'_>>) -> Result<Vec<Box<RawValue>>> {
    let mut conn = r.into().conn().await?;
    let rows: Vec<(Option<Vec<u8>>,)> =
        db::query_as("SELECT CAST(data AS BLOB) FROM user_themes ORDER BY rowid")
            .fetch_all(&mut *conn)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(data,)| stored_json(data))
        .collect())
}

/// A stored user theme. `data` is `None` when the stored text doesn't read:
/// the row exists (an update replaces it, a remove deletes it), but no
/// list shows it.
#[derive(Debug, Clone)]
pub struct ThemeRow {
    pub id: String,
    pub data: Option<Box<RawValue>>,
}

/// One user theme by its id, or `None`.
pub async fn get(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<ThemeRow>> {
    let mut conn = r.into().conn().await?;
    let row: Option<(Option<Vec<u8>>,)> =
        db::query_as("SELECT CAST(data AS BLOB) FROM user_themes WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    Ok(row.map(|(data,)| ThemeRow {
        id: id.to_string(),
        data: stored_json(data),
    }))
}

/// Inserts a user theme, stored as `data` under `id`. An id that exists
/// fails rather than overwriting.
pub async fn insert(tx: &mut WriteTx, id: &str, data: &str) -> Result<()> {
    db::query("INSERT INTO user_themes (id, data) VALUES (?, ?)")
        .bind(id)
        .bind(data)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// Replaces a user theme's data. `false` when there's no theme with that id.
pub async fn update(tx: &mut WriteTx, id: &str, data: &str) -> Result<bool> {
    let done = db::query("UPDATE user_themes SET data = ? WHERE id = ?")
        .bind(data)
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Deletes a user theme. `false` when there was none. Resetting a
/// preference that named it is Core's, in the same transaction.
pub async fn delete(tx: &mut WriteTx, id: &str) -> Result<bool> {
    let done = db::query("DELETE FROM user_themes WHERE id = ?")
        .bind(id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// How many user themes the file holds, the rows that don't read included.
pub async fn count(r: impl Into<Reader<'_>>) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = db::query_scalar("SELECT COUNT(*) FROM user_themes")
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
}
