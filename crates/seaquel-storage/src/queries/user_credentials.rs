//! `userCredentialsRepo`: `user_credentials`, the web vault's encrypted
//! credentials.

use crate::db;
use seaquel_types::storage::PersistedCredential;

use super::codec::{begin, text, Result};
use crate::{Storage, WriteTx};

pub async fn load(st: &Storage, scope: &str, key: &str) -> Result<Option<PersistedCredential>> {
    let row = db::query(
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
    let mut tx = begin(st).await?;
    db::query(
        "INSERT OR REPLACE INTO user_credentials (scope, key, nonce, ciphertext, updated_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&c.scope)
    .bind(&c.key)
    .bind(&c.nonce)
    .bind(&c.ciphertext)
    .bind(&c.updated_at)
    .execute(tx.conn())
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn remove(st: &Storage, scope: &str, key: &str) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query("DELETE FROM user_credentials WHERE scope = ? AND key = ?")
        .bind(scope)
        .bind(key)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Deletes every credential for `key` (a connection id, say), in every
/// scope.
pub async fn remove_all_for_key(st: &Storage, key: &str) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query("DELETE FROM user_credentials WHERE key = ?")
        .bind(key)
        .execute(tx.conn())
        .await?;
    tx.commit().await?;
    Ok(())
}

/// A connection's vault rows, inside a write transaction (its removal, on
/// web): the `db`, `ssh` and `ssh-key` scopes under `key` (the connection
/// id) only, so a license or AI provider credential that happens to share
/// the id stays. Returns how many rows it deleted.
pub async fn remove_all_for_key_in(tx: &mut WriteTx, key: &str) -> Result<u64> {
    let done = db::query(
        "DELETE FROM user_credentials WHERE key = ? AND scope IN ('db', 'ssh', 'ssh-key')",
    )
    .bind(key)
    .execute(tx.conn())
    .await?;
    Ok(done.rows_affected())
}

/// Deletes the credential `scope`/`key` inside a write transaction (a removed AI provider's vault row on web).
/// Returns how many rows it deleted.
pub async fn remove_in(tx: &mut WriteTx, scope: &str, key: &str) -> Result<u64> {
    let done = db::query("DELETE FROM user_credentials WHERE scope = ? AND key = ?")
        .bind(scope)
        .bind(key)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected())
}
