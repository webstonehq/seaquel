//! The chat's history as a turn sends it (Q7, Decisions 13, 23 and 29):
//! the stored rows, newest first within 512 KB, with a reply's stored tool
//! calls rendered back as the rounds they were.
//!
//! A reply's `parts` is an ordered list of text and tool items, each with
//! its 0-based `round`. Each round goes back as the assistant's text and
//! calls, then their results ([`Message::ToolResults`]: the wire marks a
//! failed one); a round without calls is a plain assistant message. A
//! result stored cut (past 16 KB) is sent with a note saying so.
//!
//! **Empty replies.** A reply stored with no text and no calls (Stop or an
//! error before anything streamed) is left out: Anthropic refuses an
//! assistant message with empty content. The questions on either side of
//! it are joined into one user message (`\n\n` between them, as
//! [`push_user`] joins the turn's own message to a history that ends with a
//! question), rather than the earlier one dropped: the budget already paid
//! for both rows, and the model sees what was asked.
//!
//! **The budget** (Decision 29) counts each row's UTF-8 bytes: a plain
//! row's content; for a row with `parts`, each text and each call's input
//! JSON plus its result as sent back. Turns are kept whole, newest first,
//! while they fit. In the first turn that doesn't, the user message is kept
//! with the reply's latest whole rounds that fit, and nothing older; if no
//! round fits, that turn goes too. A round is never split, so a tool call
//! never goes without its result.

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use crate::limits::PART_RESULT_BYTES;
use crate::tools::ToolOutput;
use crate::wire::{Message, ToolCall, ToolResult};

/// One item of a stored reply's `parts`.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Part {
    Text {
        round: u32,
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Tool {
        round: u32,
        call_id: String,
        name: String,
        input: Json,
        ok: bool,
        /// The rendered result (an error as `CODE: message`), cut at 16 KB.
        result: String,
        /// The full result's length, when `result` was cut.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result_bytes: Option<usize>,
    },
}

impl std::fmt::Debug for Part {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Part::Text { round, text } => f
                .debug_struct("Text")
                .field("round", round)
                .field("bytes", &text.len())
                .finish(),
            Part::Tool {
                round, name, ok, ..
            } => f
                .debug_struct("Tool")
                .field("round", round)
                .field("name", name)
                .field("ok", ok)
                .finish_non_exhaustive(),
        }
    }
}

impl Part {
    pub fn round(&self) -> u32 {
        match self {
            Part::Text { round, .. } | Part::Tool { round, .. } => *round,
        }
    }

    /// A tool call's item, its result cut at
    /// [`crate::limits::PART_RESULT_BYTES`] on a character boundary.
    pub fn tool(round: u32, call_id: &str, name: &str, input: Json, output: &ToolOutput) -> Part {
        let full = output.text.len();
        let cut = full > PART_RESULT_BYTES;
        let result = if cut {
            let mut end = PART_RESULT_BYTES;
            while !output.text.is_char_boundary(end) {
                end -= 1;
            }
            output.text[..end].to_string()
        } else {
            output.text.clone()
        };
        Part::Tool {
            round,
            call_id: call_id.to_string(),
            name: name.to_string(),
            input,
            ok: !output.is_error,
            result,
            result_bytes: cut.then_some(full),
        }
    }

    /// What this item costs against the history budget: a text's bytes, or
    /// a call's input JSON and its result as sent back.
    fn cost(&self) -> usize {
        match self {
            Part::Text { text, .. } => text.len(),
            Part::Tool { input, .. } => input.to_string().len() + self.result_text().len(),
        }
    }

    /// A call's stored result as history sends it back: with a note when it
    /// was cut. `""` for a text item.
    fn result_text(&self) -> String {
        match self {
            Part::Tool {
                result,
                result_bytes: Some(full),
                ..
            } => format!("{result}\n(cut at 16 KB of {full} bytes)"),
            Part::Tool { result, .. } => result.clone(),
            Part::Text { .. } => String::new(),
        }
    }
}

/// Who wrote a stored row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

/// A stored chat row, as history reads it.
#[derive(Clone, PartialEq)]
pub struct HistoryRow {
    pub id: String,
    pub role: Role,
    pub content: String,
    /// A reply's tool calls (`None`: none, as an older release wrote it).
    pub parts: Option<Vec<Part>>,
    pub dashboard_id: Option<String>,
}

impl std::fmt::Debug for HistoryRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HistoryRow")
            .field("id", &self.id)
            .field("role", &self.role)
            .field("bytes", &self.content.len())
            .field("parts", &self.parts.as_ref().map(Vec::len))
            .finish()
    }
}

/// A row the budget kept; `rounds` lists a reply's kept rounds when not all
/// of it fit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub id: String,
    pub rounds: Option<Vec<u32>>,
}

/// History as a turn sends it.
#[derive(Clone, Debug, PartialEq)]
pub struct History {
    /// The rows kept, oldest first.
    pub kept: Vec<Kept>,
    /// What they cost against the budget.
    pub bytes: usize,
    /// The rows as messages.
    pub messages: Vec<Message>,
}

/// The rows (oldest first) within `max_bytes`, as messages.
pub fn history(rows: &[HistoryRow], max_bytes: usize) -> History {
    // Turns: a user row and the rows after it (a leading reply is a turn of
    // its own).
    let mut turns: Vec<&[HistoryRow]> = Vec::new();
    let mut start = 0;
    for (i, row) in rows.iter().enumerate() {
        if row.role == Role::User && i > start {
            turns.push(&rows[start..i]);
            start = i;
        }
    }
    if start < rows.len() {
        turns.push(&rows[start..]);
    }

    let mut total = 0;
    // Newest turn first.
    let mut kept: Vec<Vec<KeptRow>> = Vec::new();
    for turn in turns.iter().rev() {
        let cost: usize = turn.iter().map(row_cost).sum();
        if total + cost <= max_bytes {
            kept.push(turn.iter().map(|r| (r, None)).collect());
            total += cost;
            continue;
        }
        // The first turn that doesn't fit: its question and the reply's
        // latest whole rounds that do, then nothing older.
        if let [user, reply] = turn {
            if let (Role::User, Some(parts)) = (user.role, &reply.parts) {
                let mut used = total + row_cost(user);
                let mut rounds_kept = Vec::new();
                for (round, items) in rounds(parts).into_iter().rev() {
                    let c: usize = items.iter().map(|p| p.cost()).sum();
                    if used + c > max_bytes {
                        break;
                    }
                    used += c;
                    rounds_kept.insert(0, round);
                }
                if !rounds_kept.is_empty() {
                    kept.push(vec![(user, None), (reply, Some(rounds_kept))]);
                    total = used;
                }
            }
        }
        break;
    }
    kept.reverse();

    let mut out = History {
        kept: Vec::new(),
        bytes: total,
        messages: Vec::new(),
    };
    for (row, only) in kept.into_iter().flatten() {
        out.kept.push(Kept {
            id: row.id.clone(),
            rounds: only.clone(),
        });
        render_row(row, only.as_deref(), &mut out.messages);
    }
    out
}

/// A row the budget kept, and its kept rounds when not all of them.
type KeptRow<'a> = (&'a HistoryRow, Option<Vec<u32>>);

fn row_cost(row: &HistoryRow) -> usize {
    match &row.parts {
        Some(parts) => parts.iter().map(Part::cost).sum(),
        None => row.content.len(),
    }
}

/// A reply's parts by round, in round order.
fn rounds(parts: &[Part]) -> Vec<(u32, Vec<&Part>)> {
    let mut out: Vec<(u32, Vec<&Part>)> = Vec::new();
    for p in parts {
        match out.iter_mut().find(|(r, _)| *r == p.round()) {
            Some((_, items)) => items.push(p),
            None => out.push((p.round(), vec![p])),
        }
    }
    out.sort_by_key(|(r, _)| *r);
    out
}

/// A stored row as messages: a plain row as its text; a reply with parts
/// as each (kept) round's text and calls, then their results.
fn render_row(row: &HistoryRow, only: Option<&[u32]>, out: &mut Vec<Message>) {
    let Some(parts) = &row.parts else {
        match row.role {
            Role::User => push_user(out, row.content.clone()),
            // A reply stored empty (Stop or an error before any text):
            // Anthropic refuses an assistant message with no content.
            Role::Assistant if row.content.is_empty() => {}
            Role::Assistant => out.push(Message::Assistant {
                text: row.content.clone(),
                tool_calls: Vec::new(),
            }),
        }
        return;
    };
    for (round, items) in rounds(parts) {
        if only.is_some_and(|keep| !keep.contains(&round)) {
            continue;
        }
        let text: String = items
            .iter()
            .filter_map(|p| match p {
                Part::Text { text, .. } => Some(text.as_str()),
                Part::Tool { .. } => None,
            })
            .collect();
        let mut calls = Vec::new();
        let mut results = Vec::new();
        for p in &items {
            if let Part::Tool {
                call_id,
                name,
                input,
                ok,
                ..
            } = p
            {
                calls.push(ToolCall {
                    id: call_id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
                results.push(ToolResult {
                    call_id: call_id.clone(),
                    content: p.result_text(),
                    is_error: !ok,
                });
            }
        }
        if calls.is_empty() {
            if !text.is_empty() {
                out.push(Message::Assistant {
                    text,
                    tool_calls: Vec::new(),
                });
            }
            continue;
        }
        out.push(Message::Assistant {
            text,
            tool_calls: calls,
        });
        out.push(Message::ToolResults(results));
    }
}

/// Append the user's message, joined to a user message the list already
/// ends with (see [`history`]).
pub fn push_user(messages: &mut Vec<Message>, text: String) {
    if let Some(Message::User(last)) = messages.last_mut() {
        last.push_str("\n\n");
        last.push_str(&text);
    } else {
        messages.push(Message::User(text));
    }
}

/// The most recent dashboard id an earlier reply carried.
pub fn last_dashboard_id(rows: &[HistoryRow]) -> Option<&str> {
    rows.iter().rev().find_map(|r| r.dashboard_id.as_deref())
}

/// Add today's `[Context: The active dashboard ID is "…" …]` line to the
/// last user message: the one the user typed, never a round's tool results
/// (`Message::ToolResults`), whatever comes after it.
pub fn with_dashboard_context(messages: &mut [Message], dashboard_id: &str) {
    let line = format!(
        "\n\n[Context: The active dashboard ID is \"{dashboard_id}\". Use this ID for any dashboard tool calls.]"
    );
    if let Some(Message::User(text)) = messages
        .iter_mut()
        .rev()
        .find(|m| matches!(m, Message::User(_)))
    {
        // Once per message, however often a round calls this.
        if !text.ends_with(&line) {
            text.push_str(&line);
        }
    }
}
