//! `userCredentialsRepo`: `user_credentials`, the web vault's encrypted
//! credentials.

use seaquel_types::storage::PersistedCredential;

use super::codec::{text, Result};
use crate::Storage;

pub async fn load(st: &Storage, scope: &str, key: &str) -> Result<Option<PersistedCredential>> {
    let row = sqlx::query(
        "SELECT scope, key, nonce, ciphertext, updated_at FROM user_credentials \
         WHERE scope = ? AND key = ?",
    )
    .bind(scope)
    .bind(key)
    .fetch_optional(st.pool())
    .await?;
    row.map(|row| {
        Ok(PersistedCredential {
            scope: text(&row, "scope")?,
            key: text(&row, "key")?,
            nonce: text(&row, "nonce")?,
            ciphertext: text(&row, "ciphertext")?,
            updated_at: text(&row, "updated_at")?,
        })
    })
    .transpose()
}

pub async fn save(st: &Storage, c: &PersistedCredential) -> Result<()> {
    sqlx::query(
        "INSERT OR REPLACE INTO user_credentials (scope, key, nonce, ciphertext, updated_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&c.scope)
    .bind(&c.key)
    .bind(&c.nonce)
    .bind(&c.ciphertext)
    .bind(&c.updated_at)
    .execute(st.pool())
    .await?;
    Ok(())
}

pub async fn remove(st: &Storage, scope: &str, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM user_credentials WHERE scope = ? AND key = ?")
        .bind(scope)
        .bind(key)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes every credential for `key` (a connection id, say), in every
/// scope.
pub async fn remove_all_for_key(st: &Storage, key: &str) -> Result<()> {
    sqlx::query("DELETE FROM user_credentials WHERE key = ?")
        .bind(key)
        .execute(st.pool())
        .await?;
    Ok(())
}
