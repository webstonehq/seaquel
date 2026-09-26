//! `connectionOverridesRepo`: `connection_overrides`, this machine's
//! settings for shared connections.

use seaquel_types::storage::PersistedConnectionOverride;
use sqlx::sqlite::SqliteRow;

use super::codec::{bit, flag, opt_number, opt_text, select_sql, text, upsert_sql, Result};
use crate::Storage;

const TABLE: &str = "connection_overrides";
const COLUMNS: [&str; 7] = [
    "shared_connection_id",
    "username",
    "host_override",
    "port_override",
    "save_password",
    "save_ssh_password",
    "save_ssh_key_passphrase",
];

fn map_row(row: &SqliteRow) -> Result<PersistedConnectionOverride> {
    Ok(PersistedConnectionOverride {
        shared_connection_id: text(row, "shared_connection_id")?,
        username: opt_text(row, "username")?,
        host_override: opt_text(row, "host_override")?,
        port_override: opt_number(row, "port_override")?,
        save_password: flag(row, "save_password")?,
        save_ssh_password: flag(row, "save_ssh_password")?,
        save_ssh_key_passphrase: flag(row, "save_ssh_key_passphrase")?,
    })
}

/// The override for one shared connection, if there is one.
pub async fn load(
    st: &Storage,
    shared_connection_id: &str,
) -> Result<Option<PersistedConnectionOverride>> {
    let row = sqlx::query(&select_sql(TABLE, &COLUMNS, "shared_connection_id = ?"))
        .bind(shared_connection_id)
        .fetch_optional(st.pool())
        .await?;
    row.as_ref().map(map_row).transpose()
}

/// Every override, in rowid order.
pub async fn load_all(st: &Storage) -> Result<Vec<PersistedConnectionOverride>> {
    let rows = sqlx::query(&select_sql(TABLE, &COLUMNS, ""))
        .fetch_all(st.pool())
        .await?;
    rows.iter().map(map_row).collect()
}

/// Upserts an override.
pub async fn save(st: &Storage, o: &PersistedConnectionOverride) -> Result<()> {
    sqlx::query(&upsert_sql(TABLE, &COLUMNS, "shared_connection_id"))
        .bind(&o.shared_connection_id)
        .bind(&o.username)
        .bind(&o.host_override)
        .bind(o.port_override)
        .bind(bit(o.save_password))
        .bind(bit(o.save_ssh_password))
        .bind(bit(o.save_ssh_key_passphrase))
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes one override.
pub async fn remove(st: &Storage, shared_connection_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM connection_overrides WHERE shared_connection_id = ?")
        .bind(shared_connection_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
