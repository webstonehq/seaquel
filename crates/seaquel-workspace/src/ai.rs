//! The assistant's wire types (phase 6): what a turn is asked to do
//! ([`ChatParams`]), how the page answers a waiting turn ([`AiDecision`]),
//! what the turn tells it ([`AiEvent`]), and the inline prompt's request
//! ([`GenerateParams`]). Core runs the turn (`seaquel_core::ai`); the `ai`
//! RPC group carries these.
//!
//! A key the page sends (web, demo) is in `api_key`: never serialized back,
//! and every type here has a `Debug` that leaves out keys, messages, SQL,
//! tool input and results.

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

use seaquel_types::storage::PersistedAIMessage;

use crate::library::ChangeSeq;

/// A provider's API key sent with one call (web, demo):
/// never serialized, never shown by `Debug`, dropped with the call.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct SuppliedSecret(String);

impl SuppliedSecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The key, for the one place that sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SuppliedSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SuppliedSecret(<redacted>)")
    }
}

impl From<&str> for SuppliedSecret {
    fn from(v: &str) -> Self {
        Self::new(v)
    }
}

impl From<String> for SuppliedSecret {
    fn from(v: String) -> Self {
        Self(v)
    }
}

/// Whether the turn asks before a query tool runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Approval {
    /// Each `run_query`, `explain_query` and `run_saved_query` waits for
    /// `respond`.
    #[default]
    Ask,
    /// The page's "Allow all" for this connection: none waits.
    AllowAll,
}

/// The user's message, with the id the page made for it (5d-2).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChatUserMessage {
    pub id: String,
    pub content: String,
}

impl fmt::Debug for ChatUserMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatUserMessage")
            .field("id", &self.id)
            .field("bytes", &self.content.len())
            .finish()
    }
}

/// One turn (`ai.chat`, stream only).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChatParams {
    pub stream_id: String,
    pub chat_id: String,
    /// The open connection (Core's id) the tools run on; it must have been
    /// connected for the chat's saved connection (`CONNECTION_MISMATCH`).
    pub connection_id: String,
    pub user_message: ChatUserMessage,
    /// The id the page made for the reply's row.
    pub assistant_message_id: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<Approval>", optional))]
    pub approval: Approval,
    /// The page runs the dashboard tools (`clientTool`).
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub client_tools: bool,
    /// The provider's key for this call only (web, demo). The desktop
    /// sends none: Core reads the keychain.
    #[serde(default, skip_serializing)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub api_key: Option<SuppliedSecret>,
    /// The provider `api_key` is for, required with it: a key for any other
    /// provider than the one Core resolves is refused before a request
    /// (`AI_PROVIDER_CHANGED`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub provider_id: Option<String>,
}

impl fmt::Debug for ChatParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatParams")
            .field("stream_id", &self.stream_id)
            .field("chat_id", &self.chat_id)
            .field("connection_id", &self.connection_id)
            .field("user_message", &self.user_message)
            .field("assistant_message_id", &self.assistant_message_id)
            .field("approval", &self.approval)
            .field("client_tools", &self.client_tools)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("provider_id", &self.provider_id)
            .finish()
    }
}

/// An approval's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum ApprovalDecision {
    Allow,
    Deny,
    /// Allow this one and every later query of the turn.
    AllowAll,
}

/// A client tool's answer: the page's text, and whether it is an error.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ClientResult {
    pub result: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<bool>", optional))]
    pub is_error: bool,
}

impl fmt::Debug for ClientResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientResult")
            .field("bytes", &self.result.len())
            .field("is_error", &self.is_error)
            .finish()
    }
}

/// `ai.respond`'s answer to a waiting turn: `"allow" | "deny" |
/// "allowAll"` for an approval, `{result, isError}` for a client tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum AiDecision {
    Approval(ApprovalDecision),
    Client(ClientResult),
}

/// How a turn that finished stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum AiStop {
    /// The model finished.
    End,
    /// The model ran out of tokens (bug 5): the reply is cut.
    MaxTokens,
    /// The reply reached the most a stored reply may hold (the message limit, `max_message_bytes`, or Core's 1 MiB ceiling):
    /// Core stopped reading the provider and stored the reply cut, ending
    /// with [`REPLY_CUT_NOTE`].
    TooLong,
}

/// What a reply Core cut for its size ends with, as stored: the
/// model reads it in later turns' history, and the page recognises it and
/// shows its own wording instead.
pub const REPLY_CUT_NOTE: &str =
    "\n\n[Seaquel cut this reply here: it was longer than a stored reply may be.]";

/// What a turn tells the page, in order: one `started`, then
/// `text` (coalesced), `toolCall`/`toolDone`, `approvalRequired` and
/// `clientTool` as they come, and exactly one `done` or `error`. A
/// cancelled turn ends with neither.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum AiEvent {
    Started {
        /// `anthropic` or `openai-compatible`.
        provider_kind: String,
        model: String,
    },
    Text {
        delta: String,
    },
    /// A tool call is about to run (or be refused): its SQL, for a query
    /// tool.
    ToolCall {
        call_id: String,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        sql: Option<String>,
    },
    /// A tool call's result reached the model: `rows` and `truncated` for
    /// a query's rows, `code` for a refusal or failure.
    ToolDone {
        call_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional, type = "number"))]
        rows: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        truncated: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        code: Option<String>,
    },
    /// The turn waits for `respond` with `allow`, `deny` or `allowAll`.
    ApprovalRequired {
        call_id: String,
        sql: String,
    },
    /// The turn waits for `respond` with the page's `{result, isError}`.
    ClientTool {
        call_id: String,
        name: String,
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        input: Json,
    },
    /// The rows Core stored for the turn (the user's message and the
    /// reply) and the sequence of the last write.
    Done {
        messages: Vec<PersistedAIMessage>,
        seq: ChangeSeq,
        stop: AiStop,
    },
    /// The turn failed. `messages` and `seq` are there once the user's
    /// message was stored: the stored rows, and the reply as it streamed
    /// (stored too, unless storing it is what failed; `seq` is then the
    /// user message's write).
    Error {
        code: String,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        messages: Option<Vec<PersistedAIMessage>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts", ts(optional))]
        seq: Option<ChangeSeq>,
    },
}

impl AiEvent {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        AiEvent::Error {
            code: code.into(),
            message: message.into(),
            messages: None,
            seq: None,
        }
    }

    /// `done` or `error`.
    pub fn is_terminal(&self) -> bool {
        matches!(self, AiEvent::Done { .. } | AiEvent::Error { .. })
    }
}

/// Ids, kinds, counts and codes only: never text, SQL, input or a row.
impl fmt::Debug for AiEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AiEvent::Started { provider_kind, .. } => f
                .debug_struct("Started")
                .field("provider_kind", provider_kind)
                .finish_non_exhaustive(),
            AiEvent::Text { delta } => write!(f, "Text({} bytes)", delta.len()),
            AiEvent::ToolCall { call_id, name, .. } => f
                .debug_struct("ToolCall")
                .field("call_id", call_id)
                .field("name", name)
                .finish_non_exhaustive(),
            AiEvent::ToolDone {
                call_id, ok, code, ..
            } => f
                .debug_struct("ToolDone")
                .field("call_id", call_id)
                .field("ok", ok)
                .field("code", code)
                .finish_non_exhaustive(),
            AiEvent::ApprovalRequired { call_id, .. } => f
                .debug_struct("ApprovalRequired")
                .field("call_id", call_id)
                .finish_non_exhaustive(),
            AiEvent::ClientTool { call_id, name, .. } => f
                .debug_struct("ClientTool")
                .field("call_id", call_id)
                .field("name", name)
                .finish_non_exhaustive(),
            AiEvent::Done {
                messages,
                seq,
                stop,
            } => f
                .debug_struct("Done")
                .field("messages", &messages.len())
                .field("seq", seq)
                .field("stop", stop)
                .finish(),
            AiEvent::Error {
                code,
                messages,
                seq,
                ..
            } => f
                .debug_struct("Error")
                .field("code", code)
                .field("messages", &messages.as_ref().map(Vec::len))
                .field("seq", seq)
                .finish_non_exhaustive(),
        }
    }
}

/// The inline prompt (`ai.generate`): the saved connection
/// whose provider, model and sharing apply, what the user asked and the
/// editor's text. Answers the SQL to insert.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GenerateParams {
    /// The saved connection's id.
    pub connection_id: String,
    pub request: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub existing_query: String,
    #[serde(default, skip_serializing)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub api_key: Option<SuppliedSecret>,
    /// The provider `api_key` is for (see [`ChatParams::provider_id`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub provider_id: Option<String>,
}

impl fmt::Debug for GenerateParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenerateParams")
            .field("connection_id", &self.connection_id)
            .field("request_bytes", &self.request.len())
            .field("existing_query_bytes", &self.existing_query.len())
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("provider_id", &self.provider_id)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions_read_both_shapes() {
        let d: AiDecision = serde_json::from_str(r#""allowAll""#).unwrap();
        assert_eq!(d, AiDecision::Approval(ApprovalDecision::AllowAll));
        let d: AiDecision = serde_json::from_str(r#"{"result":"{}","isError":true}"#).unwrap();
        assert_eq!(
            d,
            AiDecision::Client(ClientResult {
                result: "{}".into(),
                is_error: true
            })
        );
        assert!(serde_json::from_str::<AiDecision>(r#""maybe""#).is_err());
    }

    #[test]
    fn keys_never_leave_through_debug_or_serde() {
        let params: ChatParams = serde_json::from_value(serde_json::json!({
            "streamId": "s", "chatId": "c", "connectionId": "k",
            "userMessage": {"id": "u", "content": "MARKER_PROMPT"},
            "assistantMessageId": "a", "apiKey": "test-key-not-real"
        }))
        .unwrap();
        assert_eq!(
            params.api_key.as_ref().map(SuppliedSecret::expose),
            Some("test-key-not-real")
        );
        let shown = format!("{params:?} {params:#?}");
        assert!(!shown.contains("test-key-not-real") && !shown.contains("MARKER"));
        let back = serde_json::to_string(&params).unwrap();
        assert!(!back.contains("test-key-not-real"), "{back}");
        assert!(serde_json::from_value::<ChatParams>(serde_json::json!({
            "streamId": "s", "chatId": "c", "connectionId": "k",
            "userMessage": {"id": "u", "content": "x"}, "assistantMessageId": "a",
            "extra": 1
        }))
        .is_err());
    }
}
