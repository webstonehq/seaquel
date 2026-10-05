//! A turn's chat writes and reads: the user's
//! message before the first round, the reply at the end (on `done`, an
//! error or a cancel), two writes per turn, each one `WriteTx` with its
//! `chatMessages` event and change number; the reply's also announces the
//! chat (its `updated_at` moved). Nothing here holds the write lock
//! while a model round runs: each write opens, writes and commits.
//!
//! History is read back from storage, a reply's `parts` rendered as the
//! rounds they were (`seaquel_ai::prompt::history`).

use seaquel_ai::prompt::history::{HistoryRow, Part, Role};
use seaquel_storage::ai_chats;
use seaquel_types::storage::{PersistedAIChat, PersistedAIMessage};
use seaquel_workspace::library::{ChangeSeq, LibraryError};
use seaquel_workspace::state::{self as st, ChatMessageDraft, CHAT_NOT_FOUND};

use crate::changes::WriteOrigin;
use crate::{Core, CoreError, StoredKind, Workspace};

/// `CHAT_FULL`: the chat can't take a turn here (the web's budget),
/// checked before any model call.
pub const CHAT_FULL: &str = "CHAT_FULL";

pub(crate) fn chat_not_found() -> CoreError {
    CoreError::new(CHAT_NOT_FOUND, "Chat not found.")
}

fn chat_full() -> CoreError {
    CoreError::new(
        CHAT_FULL,
        "This chat is full. Start a new chat to continue.",
    )
}

/// A row's own checks, as `chatMessagesPut` runs them: its id, role and
/// content within the limits.
pub(crate) fn check_row(core: &Core, row: &PersistedAIMessage) -> Result<(), CoreError> {
    let draft = ChatMessageDraft {
        id: row.id.clone(),
        role: row.role.clone(),
        content: row.content.clone(),
        timestamp: row.timestamp.clone(),
        query: row.query.clone(),
        dashboard_id: row.dashboard_id.clone(),
        parts: row.parts.clone(),
    };
    st::check_messages(&[draft], &core.library_limits(), &core.state_limits())
        .map_err(CoreError::from)
}

/// Whether a chat holding `stored` content bytes in `held` messages can
/// take a turn: the user's message of `user_bytes` and a reply of the
/// largest size allowed.
fn has_room(core: &Core, stored: u64, held: u64, user_bytes: u64) -> bool {
    let limits = core.state_limits();
    let count_ok = limits
        .max_messages_per_chat
        .is_none_or(|max| held.saturating_add(2) <= max as u64);
    let bytes_ok = limits.max_chat_bytes.is_none_or(|max| {
        let reply = limits.max_message_bytes.unwrap_or(0) as u64;
        stored.saturating_add(user_bytes).saturating_add(reply) <= max
    });
    count_ok && bytes_ok
}

/// The chat, or `CHAT_NOT_FOUND`.
pub(crate) async fn chat(ws: &Workspace, chat_id: &str) -> Result<PersistedAIChat, CoreError> {
    ai_chats::get(ws.storage(), chat_id)
        .await?
        .ok_or_else(chat_not_found)
}

/// `CHAT_FULL` unless the chat can take the turn (a read; the user's write
/// checks again under the lock).
pub(crate) async fn check_room(
    ws: &Workspace,
    core: &Core,
    chat_id: &str,
    user_bytes: u64,
) -> Result<(), CoreError> {
    let limits = core.state_limits();
    if limits.max_messages_per_chat.is_none() && limits.max_chat_bytes.is_none() {
        return Ok(());
    }
    let stored = ai_chats::content_bytes(ws.storage(), chat_id).await?;
    let held = ai_chats::message_count(ws.storage(), chat_id).await?;
    if has_room(core, stored, held, user_bytes) {
        Ok(())
    } else {
        Err(chat_full())
    }
}

/// Which of a turn's two writes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Write {
    /// The user's message, before the first round: the chat must have room
    /// for the turn.
    User,
    /// The reply: the chat's `updated_at` moves to now.
    Reply,
}

/// Stores `row` in `chat_id` and announces it: one `WriteTx`, one
/// `chatMessages` event with the caller's origin. Answers the write's
/// change number.
pub(crate) async fn write(
    ws: &Workspace,
    core: &Core,
    origin: &WriteOrigin,
    chat_id: &str,
    row: &PersistedAIMessage,
    which: Write,
    now: &str,
) -> Result<ChangeSeq, CoreError> {
    check_row(core, row)?;
    let mut tx = ws.storage().write().await?;
    let ticket = ws.take_seq();
    let Some(chat) = ai_chats::get(&mut tx, chat_id).await? else {
        return Err(chat_not_found());
    };
    let limits = core.state_limits();
    let budgeted = limits.max_chat_bytes.is_some() || limits.max_messages_per_chat.is_some();
    if which == Write::User && budgeted {
        let stored = ai_chats::content_bytes(&mut tx, chat_id).await?;
        let held = ai_chats::message_count(&mut tx, chat_id).await?;
        if !has_room(core, stored, held, row.content.len() as u64) {
            return Err(chat_full());
        }
    }
    // The reply, its tool calls included, within what the chat may hold
    // (the user's write left room for a reply of `max_message_bytes`;
    // parts can take it past that).
    if let (Write::Reply, Some(max)) = (which, limits.max_chat_bytes) {
        let stored = ai_chats::content_bytes(&mut tx, chat_id).await?;
        let replaced =
            ai_chats::content_bytes_of(&mut tx, chat_id, std::slice::from_ref(&row.id)).await?;
        let added = row.content.len() + row.parts.as_ref().map_or(0, |p| p.to_string().len());
        if stored.saturating_sub(replaced).saturating_add(added as u64) > max {
            return Err(chat_full());
        }
    }
    let touched = (which == Write::Reply).then_some(now);
    match ai_chats::append_turn_in(&mut tx, chat_id, std::slice::from_ref(row), touched).await? {
        ai_chats::PutMessages::Done => {}
        ai_chats::PutMessages::OtherChat { .. } => {
            return Err(LibraryError::invalid("A message id belongs to another chat.").into())
        }
    }
    tx.commit().await?;
    let seq = ws.announce(
        ticket,
        StoredKind::ChatMessages,
        Some(chat_id.to_string()),
        Some(vec![row.id.clone()]),
        origin,
    );
    // The reply moved the chat's `updated_at`: its list reorders (review
    // flag 3). The number is taken after the commit, as the storage
    // group's writes take theirs.
    if which == Write::Reply {
        ws.record_storage_write(
            origin,
            StoredKind::Chat,
            Some(chat.connection_id),
            Some(vec![chat_id.to_string()]),
        );
    }
    Ok(seq)
}

/// The chat's stored rows as history reads them, leaving out `skip` (the
/// turn's own rows, when a retry reuses their ids). A row whose `parts`
/// doesn't read as a list of text and tool parts goes as its text.
pub(crate) async fn history_rows(
    ws: &Workspace,
    chat_id: &str,
    skip: &[&str],
) -> Result<Vec<HistoryRow>, CoreError> {
    let rows = ai_chats::load_messages(ws.storage(), chat_id).await?;
    Ok(rows
        .into_iter()
        .filter(|m| !skip.contains(&m.id.as_str()))
        .map(|m| HistoryRow {
            role: if m.role == "user" {
                Role::User
            } else {
                Role::Assistant
            },
            parts: m
                .parts
                .and_then(|p| serde_json::from_value::<Vec<Part>>(p).ok()),
            id: m.id,
            content: m.content,
            dashboard_id: m.dashboard_id,
        })
        .collect())
}
