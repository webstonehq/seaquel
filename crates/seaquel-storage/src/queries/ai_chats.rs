//! `aiChatsRepo`: `ai_chats` and `ai_messages`.
//!
//! From phase 5d-2 (Decision 24) Core writes chats one at a time and
//! upserts messages by id ([`put_messages`]) instead of replacing a chat's
//! list; [`replace_all_messages`] stays for its frozen fixtures.

use seaquel_types::storage::{PersistedAIChat, PersistedAIMessage};
use sqlx::sqlite::SqliteRow;

use super::codec::{begin, insert_sql, opt_text, text, upsert_sql, Result};
use crate::{Reader, Storage, WriteTx};

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

fn map_chat(row: &SqliteRow) -> Result<PersistedAIChat> {
    Ok(PersistedAIChat {
        id: text(row, "id")?,
        connection_id: text(row, "connection_id")?,
        title: text(row, "title")?,
        created_at: text(row, "created_at")?,
        updated_at: text(row, "updated_at")?,
    })
}

/// A connection's chats, most recently updated first.
pub async fn load_by_connection(st: &Storage, connection_id: &str) -> Result<Vec<PersistedAIChat>> {
    list(st, connection_id).await
}

/// [`load_by_connection`] on the pool or inside a write. A row with a value
/// that doesn't decode (text that isn't UTF-8) is skipped rather than
/// failing the list.
pub async fn list(r: impl Into<Reader<'_>>, connection_id: &str) -> Result<Vec<PersistedAIChat>> {
    let mut conn = r.into().conn().await?;
    let rows =
        sqlx::query("SELECT * FROM ai_chats WHERE connection_id = ? ORDER BY updated_at DESC")
            .bind(connection_id)
            .fetch_all(&mut *conn)
            .await?;
    Ok(rows.iter().filter_map(|row| map_chat(row).ok()).collect())
}

/// One chat, or `None` (also for a row with a value that doesn't decode,
/// which no list shows either).
pub async fn get(r: impl Into<Reader<'_>>, id: &str) -> Result<Option<PersistedAIChat>> {
    let mut conn = r.into().conn().await?;
    let row = sqlx::query("SELECT * FROM ai_chats WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    Ok(row.as_ref().and_then(|row| map_chat(row).ok()))
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

/// Inserts a new chat. An id that exists fails rather than overwriting.
pub async fn insert(tx: &mut WriteTx, chat: &PersistedAIChat) -> Result<()> {
    sqlx::query(&insert_sql("ai_chats", &CHAT_COLUMNS))
        .bind(&chat.id)
        .bind(&chat.connection_id)
        .bind(&chat.title)
        .bind(&chat.created_at)
        .bind(&chat.updated_at)
        .execute(tx.conn())
        .await?;
    Ok(())
}

/// Writes an existing chat's `title` and `updated_at`; a chat never moves
/// to another connection and keeps when it was made. `false` when there's
/// no chat with that id.
pub async fn update(tx: &mut WriteTx, chat: &PersistedAIChat) -> Result<bool> {
    let done = sqlx::query("UPDATE ai_chats SET title = ?, updated_at = ? WHERE id = ?")
        .bind(&chat.title)
        .bind(&chat.updated_at)
        .bind(&chat.id)
        .execute(tx.conn())
        .await?;
    Ok(done.rows_affected() > 0)
}

/// Deletes one chat and its messages. The messages are deleted explicitly,
/// not left to the foreign key's cascade. `false` when there was no such
/// chat.
pub async fn delete(tx: &mut WriteTx, id: &str) -> Result<bool> {
    let conn = tx.conn();
    sqlx::query("DELETE FROM ai_messages WHERE chat_id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    let done = sqlx::query("DELETE FROM ai_chats WHERE id = ?")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// How many chats the file holds, for every connection.
pub async fn count(r: impl Into<Reader<'_>>) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_chats")
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
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

/// The read behind [`load_messages`]: `idx_ai_messages_chat_time`
/// (migration `0002`) gives the rows in order, with no sort (`?1` the chat).
pub const LOAD_MESSAGES: &str =
    "SELECT * FROM ai_messages WHERE chat_id = ?1 ORDER BY timestamp ASC, rowid ASC";

/// A chat's messages, oldest first. Messages with one timestamp (a user
/// message and the assistant's placeholder made in one millisecond) come
/// in the order they were first stored (`rowid`), which an upsert keeps. A
/// message with a value that doesn't decode (text that isn't UTF-8) is
/// skipped rather than failing the chat.
pub async fn load_messages(
    r: impl Into<Reader<'_>>,
    chat_id: &str,
) -> Result<Vec<PersistedAIMessage>> {
    let mut conn = r.into().conn().await?;
    let rows = sqlx::query(LOAD_MESSAGES)
        .bind(chat_id)
        .fetch_all(&mut *conn)
        .await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let message = || -> Result<PersistedAIMessage> {
                Ok(PersistedAIMessage {
                    id: text(row, "id")?,
                    chat_id: text(row, "chat_id")?,
                    role: text(row, "role")?,
                    content: text(row, "content")?,
                    timestamp: text(row, "timestamp")?,
                    query: opt_text(row, "query")?,
                    dashboard_id: opt_text(row, "dashboard_id")?,
                })
            };
            message().ok()
        })
        .collect())
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

/// What [`put_messages`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PutMessages {
    /// Every message was stored.
    Done,
    /// `id` is a message of another chat (`ai_messages.id` is global), so
    /// nothing was written.
    OtherChat { id: String },
}

/// The chat each of `ids` belongs to, for those that exist (primary-key
/// lookups), as `(id, chat_id)` in the order given.
pub async fn message_chat_ids(
    r: impl Into<Reader<'_>>,
    ids: &[String],
) -> Result<Vec<(String, String)>> {
    let mut conn = r.into().conn().await?;
    let mut out = Vec::new();
    for id in ids {
        let chat: Option<(Option<Vec<u8>>,)> =
            sqlx::query_as("SELECT CAST(chat_id AS BLOB) FROM ai_messages WHERE id = ?")
                .bind(id)
                .fetch_optional(&mut *conn)
                .await?;
        if let Some((chat,)) = chat {
            let chat = chat
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            out.push((id.clone(), chat));
        }
    }
    Ok(out)
}

/// Upserts `messages` into chat `chat_id` by id (each message's own
/// `chat_id` is ignored). A message that exists keeps its place (`ON
/// CONFLICT DO UPDATE` keeps its rowid) and gets every other field; new
/// ones are added in list order; messages not listed stay. When an id
/// belongs to another chat, nothing is written and that id is returned.
pub async fn put_messages(
    tx: &mut WriteTx,
    chat_id: &str,
    messages: &[PersistedAIMessage],
) -> Result<PutMessages> {
    let ids: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();
    for (id, owner) in message_chat_ids(&mut *tx, &ids).await? {
        if owner != chat_id {
            return Ok(PutMessages::OtherChat { id });
        }
    }
    let upsert = upsert_sql("ai_messages", &MESSAGE_COLUMNS, "id");
    let conn = tx.conn();
    for m in messages {
        sqlx::query(&upsert)
            .bind(&m.id)
            .bind(chat_id)
            .bind(&m.role)
            .bind(&m.content)
            .bind(&m.timestamp)
            .bind(&m.query)
            .bind(&m.dashboard_id)
            .execute(&mut *conn)
            .await?;
    }
    Ok(PutMessages::Done)
}

/// Deletes those of `ids` that are messages of `chat_id` (another chat's
/// are left alone), and returns how many it deleted.
pub async fn delete_messages(tx: &mut WriteTx, chat_id: &str, ids: &[String]) -> Result<u64> {
    let conn = tx.conn();
    let mut deleted = 0;
    for id in ids {
        deleted += sqlx::query("DELETE FROM ai_messages WHERE chat_id = ? AND id = ?")
            .bind(chat_id)
            .bind(id)
            .execute(&mut *conn)
            .await?
            .rows_affected();
    }
    Ok(deleted)
}

/// The count behind [`message_count`], on `idx_ai_messages_chat` (`?1` the
/// chat).
pub const MESSAGE_COUNT: &str = "SELECT COUNT(*) FROM ai_messages WHERE chat_id = ?1";

/// How many messages a chat holds.
pub async fn message_count(r: impl Into<Reader<'_>>, chat_id: &str) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = sqlx::query_scalar(MESSAGE_COUNT)
        .bind(chat_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
}

/// The sum behind [`content_bytes`], on `idx_ai_messages_chat` (`?1` the
/// chat). `octet_length` reads each value's size from its record header,
/// so the text itself (often in overflow pages) isn't loaded.
pub const CONTENT_BYTES: &str =
    "SELECT COALESCE(SUM(octet_length(content)), 0) FROM ai_messages WHERE chat_id = ?1";

/// The UTF-8 bytes of a chat's stored message content (the web budget,
/// `max_chat_bytes`, Decision 24).
pub async fn content_bytes(r: impl Into<Reader<'_>>, chat_id: &str) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let n: i64 = sqlx::query_scalar(CONTENT_BYTES)
        .bind(chat_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok(n.max(0) as u64)
}

/// The content bytes of those of `ids` that are messages of `chat_id`: what
/// a put replaces, taken off [`content_bytes`] before adding the new ones.
pub async fn content_bytes_of(
    r: impl Into<Reader<'_>>,
    chat_id: &str,
    ids: &[String],
) -> Result<u64> {
    let mut conn = r.into().conn().await?;
    let mut total = 0u64;
    for id in ids {
        let n: Option<i64> = sqlx::query_scalar(
            "SELECT octet_length(content) FROM ai_messages WHERE id = ? AND chat_id = ?",
        )
        .bind(id)
        .bind(chat_id)
        .fetch_optional(&mut *conn)
        .await?
        .flatten();
        total = total.saturating_add(n.unwrap_or(0).max(0) as u64);
    }
    Ok(total)
}
