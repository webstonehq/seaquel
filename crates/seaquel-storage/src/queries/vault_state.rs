//! `vaultStateRepo`: `vault_state`, the web vault's one row.

use crate::db;
use seaquel_types::storage::PersistedVaultState;

use super::codec::{begin, decode_error, parse_json, text, Result};
use crate::Storage;

/// The vault's row, or `None` before one is set up. Fails when
/// `kdf_params` isn't JSON (the TypeScript used a bare `JSON.parse`).
pub async fn load(st: &Storage) -> Result<Option<PersistedVaultState>> {
    let row = db::query(
        "SELECT salt, kdf_params, verifier, verifier_nonce, created_at FROM vault_state WHERE id = 1",
    )
    .fetch_optional(st.pool())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let kdf_params = parse_json(&text(&row, "kdf_params")?)
        .ok_or_else(|| decode_error("vault_state.kdf_params isn't JSON"))?;
    Ok(Some(PersistedVaultState {
        salt: text(&row, "salt")?,
        kdf_params,
        verifier: text(&row, "verifier")?,
        verifier_nonce: text(&row, "verifier_nonce")?,
        created_at: text(&row, "created_at")?,
    }))
}

pub async fn save(st: &Storage, s: &PersistedVaultState) -> Result<()> {
    db::query(
        "INSERT OR REPLACE INTO vault_state (id, salt, kdf_params, verifier, verifier_nonce, created_at) \
         VALUES (1, ?, ?, ?, ?, ?)",
    )
    .bind(&s.salt)
    .bind(s.kdf_params.get())
    .bind(&s.verifier)
    .bind(&s.verifier_nonce)
    .bind(&s.created_at)
    .execute(st.pool())
    .await?;
    Ok(())
}

/// Deletes the vault and every credential encrypted under it, in one
/// transaction: they're only meaningful together.
pub async fn reset(st: &Storage) -> Result<()> {
    let mut tx = begin(st).await?;
    db::query("DELETE FROM vault_state")
        .execute(&mut *tx)
        .await?;
    db::query("DELETE FROM user_credentials")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
