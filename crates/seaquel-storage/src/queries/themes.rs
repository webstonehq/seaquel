//! `themeRepo`: `theme_preferences` (one row) and `user_themes`, each
//! theme stored as JSON.

use seaquel_types::storage::ThemePreferences;
use serde_json::value::RawValue;

use super::codec::{begin, bind_json_id, is_null, parse_json, Result};
use crate::Storage;

/// The light and dark theme ids, or `None` before they're first saved.
pub async fn load_preferences(st: &Storage) -> Result<Option<ThemePreferences>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT light_theme_id, dark_theme_id FROM theme_preferences WHERE id = 1")
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
    sqlx::query("INSERT OR REPLACE INTO theme_preferences (id, light_theme_id, dark_theme_id) VALUES (1, ?, ?)")
        .bind(light_theme_id)
        .bind(dark_theme_id)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Every user theme, as its stored JSON, in rowid order. Rows that don't
/// parse or hold `null` are skipped; any other JSON (a scalar too) is kept.
pub async fn load_user_themes(st: &Storage) -> Result<Vec<Box<RawValue>>> {
    let rows: Vec<(Option<String>,)> = sqlx::query_as("SELECT data FROM user_themes")
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
    sqlx::query("DELETE FROM user_themes")
        .execute(&mut *tx)
        .await?;
    for theme in themes {
        let insert = sqlx::query("INSERT INTO user_themes (id, data) VALUES (?, ?)");
        let insert = bind_json_id(insert, theme, None)?;
        insert.bind(theme.get()).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}
