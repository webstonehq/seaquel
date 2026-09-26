//! `aiChatsRepo`: `ai_chats` and `ai_messages`.

use seaquel_types::storage::{PersistedAIChat, PersistedAIMessage};

use super::codec::{begin, insert_sql, opt_text, text, upsert_sql, Result};
use crate::Storage;

const CHAT_COLUMNS: [&str; 5] = ["id", "connection_id", "title", "created_at", "updated_at"];
const MESSAGE_COLUMNS: [&str; 7] = [
    "id",
    "chat_id",
    "role",
    "content",
    "timestamp",
    "query",
    "dashboard_id",
];

/// A connection's chats, most recently updated first.
pub async fn load_by_connection(st: &Storage, connection_id: &str) -> Result<Vec<PersistedAIChat>> {
    let rows =
        sqlx::query("SELECT * FROM ai_chats WHERE connection_id = ? ORDER BY updated_at DESC")
            .bind(connection_id)
            .fetch_all(st.pool())
            .await?;
    rows.iter()
        .map(|row| {
            Ok(PersistedAIChat {
                id: text(row, "id")?,
                connection_id: text(row, "connection_id")?,
                title: text(row, "title")?,
                created_at: text(row, "created_at")?,
                updated_at: text(row, "updated_at")?,
            })
        })
        .collect()
}

/// Upserts a chat.
pub async fn save_chat(st: &Storage, chat: &PersistedAIChat) -> Result<()> {
    sqlx::query(&upsert_sql("ai_chats", &CHAT_COLUMNS, "id"))
        .bind(&chat.id)
        .bind(&chat.connection_id)
        .bind(&chat.title)
        .bind(&chat.created_at)
        .bind(&chat.updated_at)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes a chat. Its messages cascade.
pub async fn remove_chat(st: &Storage, chat_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM ai_chats WHERE id = ?")
        .bind(chat_id)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// Deletes a connection's chats.
pub async fn remove_by_connection(st: &Storage, connection_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM ai_chats WHERE connection_id = ?")
        .bind(connection_id)
        .execute(st.pool())
        .await?;
    Ok(())
}

/// A chat's messages, oldest first.
pub async fn load_messages(st: &Storage, chat_id: &str) -> Result<Vec<PersistedAIMessage>> {
    let rows = sqlx::query("SELECT * FROM ai_messages WHERE chat_id = ? ORDER BY timestamp ASC")
        .bind(chat_id)
        .fetch_all(st.pool())
        .await?;
    rows.iter()
        .map(|row| {
            Ok(PersistedAIMessage {
                id: text(row, "id")?,
                chat_id: text(row, "chat_id")?,
                role: text(row, "role")?,
                content: text(row, "content")?,
                timestamp: text(row, "timestamp")?,
                query: opt_text(row, "query")?,
                dashboard_id: opt_text(row, "dashboard_id")?,
            })
        })
        .collect()
}

/// Replaces a chat's messages with `messages`, in one transaction. Each
/// message is inserted with its own `chatId`.
pub async fn replace_all_messages(
    st: &Storage,
    chat_id: &str,
    messages: &[PersistedAIMessage],
) -> Result<()> {
    let mut tx = begin(st).await?;
    sqlx::query("DELETE FROM ai_messages WHERE chat_id = ?")
        .bind(chat_id)
        .execute(&mut *tx)
        .await?;
    let insert = insert_sql("ai_messages", &MESSAGE_COLUMNS);
    for m in messages {
        sqlx::query(&insert)
            .bind(&m.id)
            .bind(&m.chat_id)
            .bind(&m.role)
            .bind(&m.content)
            .bind(&m.timestamp)
            .bind(&m.query)
            .bind(&m.dashboard_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
