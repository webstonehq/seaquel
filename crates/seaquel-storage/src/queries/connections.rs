//! `connectionsRepo`: `connections` and their `connection_labels`.

use seaquel_types::storage::PersistedConnection;

use super::codec::{
    begin, bit, flag, json, number, opt_bit, opt_flag, opt_text, select_sql, text,
    truthy_json_text, upsert_sql, Result,
};
use crate::{strip_connection_string_password, Storage};

const TABLE: &str = "connections";
const COLUMNS: [&str; 21] = [
    "id",
    "project_id",
    "name",
    "type",
    "host",
    "port",
    "database_name",
    "username",
    "ssl_mode",
    "connection_string",
    "last_connected",
    "ssh_tunnel",
    "save_password",
    "save_ssh_password",
    "save_ssh_key_passphrase",
    "is_local_only",
    "shared_connection_id",
    "ai_share_schema",
    "ai_share_data",
    "active_ai_provider_id",
    "active_ai_model",
];

/// Every connection with its label ids, in rowid order. The label ids come
/// in the order of `connection_labels`' primary key, not the order they were
/// saved in.
pub async fn load_all(st: &Storage) -> Result<Vec<PersistedConnection>> {
    let rows = sqlx::query(&select_sql(TABLE, &COLUMNS, ""))
        .fetch_all(st.pool())
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        let id = text(row, "id")?;
        let label_ids: Vec<(String,)> =
            sqlx::query_as("SELECT label_id FROM connection_labels WHERE connection_id = ?")
                .bind(&id)
                .fetch_all(st.pool())
                .await?;
        out.push(PersistedConnection {
            id,
            project_id: text(row, "project_id")?,
            name: text(row, "name")?,
            ty: text(row, "type")?,
            host: text(row, "host")?,
            port: number(row, "port")?,
            database_name: text(row, "database_name")?,
            username: text(row, "username")?,
            ssl_mode: opt_text(row, "ssl_mode")?,
            connection_string: opt_text(row, "connection_string")?,
            // `v ? new Date(v) : undefined`: the TypeScript makes the Date.
            last_connected: opt_text(row, "last_connected")?.filter(|t| !t.is_empty()),
            ssh_tunnel: json(row, "ssh_tunnel")?,
            save_password: flag(row, "save_password")?,
            save_ssh_password: flag(row, "save_ssh_password")?,
            save_ssh_key_passphrase: flag(row, "save_ssh_key_passphrase")?,
            label_ids: label_ids.into_iter().map(|l| l.0).collect(),
            // `v === 1 ? true : undefined`
            is_local_only: flag(row, "is_local_only")?.then_some(true),
            shared_connection_id: opt_text(row, "shared_connection_id")?,
            ai_share_schema: opt_flag(row, "ai_share_schema")?,
            ai_share_data: opt_flag(row, "ai_share_data")?,
            active_ai_provider_id: opt_text(row, "active_ai_provider_id")?,
            active_ai_model: opt_text(row, "active_ai_model")?,
        });
    }
    Ok(out)
}

/// Upserts the connection and replaces its labels, in one transaction. A
/// label id listed twice is saved once.
///
/// The connection string is saved without its password
/// ([`strip_connection_string_password`]), so a row an older build saved
/// with one is cleaned the next time it's saved.
pub async fn save(st: &Storage, c: &PersistedConnection) -> Result<()> {
    let connection_string = c
        .connection_string
        .as_deref()
        .map(strip_connection_string_password);
    let mut tx = begin(st).await?;
    sqlx::query(&upsert_sql(TABLE, &COLUMNS, "id"))
        .bind(&c.id)
        .bind(&c.project_id)
        .bind(&c.name)
        .bind(&c.ty)
        .bind(&c.host)
        .bind(c.port)
        .bind(&c.database_name)
        .bind(&c.username)
        .bind(&c.ssl_mode)
        .bind(connection_string)
        .bind(&c.last_connected)
        .bind(truthy_json_text(&c.ssh_tunnel))
        .bind(bit(c.save_password))
        .bind(bit(c.save_ssh_password))
        .bind(bit(c.save_ssh_key_passphrase))
        .bind(bit(c.is_local_only == Some(true)))
        .bind(&c.shared_connection_id)
        .bind(opt_bit(c.ai_share_schema))
        .bind(opt_bit(c.ai_share_data))
        .bind(&c.active_ai_provider_id)
        .bind(&c.active_ai_model)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM connection_labels WHERE connection_id = ?")
        .bind(&c.id)
        .execute(&mut *tx)
        .await?;
    // A repeated id would fail the primary key and roll the whole save
    // back; keep its first occurrence.
    let mut seen = std::collections::HashSet::new();
    for label_id in c.label_ids.iter().filter(|id| seen.insert(id.as_str())) {
        sqlx::query("INSERT INTO connection_labels (connection_id, label_id) VALUES (?, ?)")
            .bind(&c.id)
            .bind(label_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Deletes the connection. Its labels, history and AI chats cascade.
pub async fn remove(st: &Storage, connection_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM connections WHERE id = ?")
        .bind(connection_id)
        .execute(st.pool())
        .await?;
    Ok(())
}
