//! The `ai` group of the workspace RPC (phase 6): the assistant's turn
//! (`chat`, a stream), the page's answers to a waiting turn (`respond`),
//! the inline prompt (`generate`), the model list (`models`) and the
//! provider test (`test`). Core runs them all (`seaquel_core::ai`).
//!
//! Wire shape, like the other groups:
//!
//! ```json
//! {"method":"ai","params":{"method":"respond","params":{"streamId":"s","callId":"c","decision":"allow"}}}
//! {"method":"ai","result":{"method":"respond","result":null}}
//! ```
//!
//! **`chat` is a stream.** [`crate::dispatch_stream`] serves it, its events
//! are [`crate::CoreEvent::Ai`]s, and [`crate::dispatch_workspace`] refuses
//! it with `INVALID_ARGUMENT`, as it refuses `db.run`. A transport stops a
//! turn with `db.cancel` (Core's `Workspace::cancel`) and keeps polling the
//! stream to its end, so the reply is stored with what streamed; it never
//! drops a running turn (Task 4's contract).
//!
//! A key the page sends (web, demo) is `apiKey`: never serialized back and
//! never in `Debug`. On the desktop the page sends none; Core reads the
//! keychain. Every params type refuses unknown fields.
//!
//! Without the crate's `ai` feature every method answers `NOT_SUPPORTED`;
//! with it, Core's builder policies decide (no client or no egress policy
//! is `NOT_SUPPORTED` too, `Off` is `AI_EGRESS_BLOCKED`).

use std::fmt;

use seaquel_core::domain::ai::{AiDecision, ChatParams, GenerateParams, SuppliedSecret};
use seaquel_core::{Core, Workspace, WriteOrigin};
use serde::{Deserialize, Serialize};

use seaquel_engine::BoxStream;

use crate::workspace::RpcError;
use crate::CoreEvent;

/// How long a transport keeps polling a turn it cancelled because its
/// receiver went away (a closed socket, a reloaded or closed webview), so
/// the reply's write can finish: a full storage write turn
/// (`WRITE_WAIT`, which that write may queue behind) and 15 s more. Past
/// it the transport drops the turn. The turn needs `storage`, so this
/// exists with the `ai` feature.
#[cfg(feature = "ai")]
pub const TURN_STOP_WAIT: std::time::Duration =
    seaquel_core::storage::WRITE_WAIT.saturating_add(std::time::Duration::from_secs(15));

/// An `ai` call. `Debug` shows ids and sizes, never a key, a message, SQL
/// or a client tool's answer.
#[allow(clippy::large_enum_variant)] // See `Request`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "method",
    content = "params",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum AiRequest {
    /// One turn. Only through [`crate::dispatch_stream`].
    Chat(ChatParams),
    /// Answers a waiting turn of this workspace: an approval (`"allow"`,
    /// `"deny"`, `"allowAll"`) or a client tool's `{result, isError}`.
    /// `NOT_FOUND` for a stream or call the workspace isn't waiting on; a
    /// second answer to a call is ignored.
    Respond {
        stream_id: String,
        call_id: String,
        decision: AiDecision,
    },
    /// The inline prompt: the SQL to insert (Decision 18).
    Generate(GenerateParams),
    /// The provider's model ids (Decision 28).
    Models(ProviderParams),
    /// Whether the provider takes the key (Decision 28).
    Test(ProviderParams),
}

impl AiRequest {
    /// The method's wire name.
    pub fn method(&self) -> &'static str {
        match self {
            AiRequest::Chat(_) => "chat",
            AiRequest::Respond { .. } => "respond",
            AiRequest::Generate(_) => "generate",
            AiRequest::Models(_) => "models",
            AiRequest::Test(_) => "test",
        }
    }

    /// The turn's `streamId`, for `chat`.
    pub fn stream_id(&self) -> Option<&str> {
        match self {
            AiRequest::Chat(params) => Some(&params.stream_id),
            _ => None,
        }
    }
}

/// `models`' and `test`'s params: the provider, and its key for this call
/// (web, demo).
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProviderParams {
    pub provider_id: String,
    #[serde(default, skip_serializing)]
    #[cfg_attr(feature = "ts", ts(as = "Option<String>", optional))]
    pub api_key: Option<SuppliedSecret>,
}

impl fmt::Debug for ProviderParams {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderParams")
            .field("provider_id", &self.provider_id)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// An `ai` call's result, as `{"method": …, "result": …}`.
#[derive(Debug, Serialize)]
#[serde(tag = "method", content = "result", rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum AiResponse {
    Respond(()),
    Generate(Generated),
    Models(Vec<String>),
    Test(()),
}

/// `generate`'s answer: the SQL to insert (the reply's first fenced block).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Generated {
    pub sql: String,
}

impl fmt::Debug for Generated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Generated")
            .field("sql_bytes", &self.sql.len())
            .finish()
    }
}

/// Serve an `ai` call that isn't a stream through `ws`. `chat` is
/// `INVALID_ARGUMENT`: [`crate::dispatch_stream`] serves it.
#[cfg(feature = "ai")]
pub(crate) async fn ai(
    core: &Core,
    ws: &Workspace,
    req: AiRequest,
    _origin: &WriteOrigin,
) -> Result<AiResponse, RpcError> {
    Ok(match req {
        AiRequest::Chat(_) => return Err(chat_is_a_stream()),
        AiRequest::Respond {
            stream_id,
            call_id,
            decision,
        } => AiResponse::Respond(ws.ai_respond(&stream_id, &call_id, decision)?),
        AiRequest::Generate(params) => AiResponse::Generate(Generated {
            sql: ws.ai_generate(core, params).await?,
        }),
        AiRequest::Models(p) => {
            AiResponse::Models(ws.ai_models(core, &p.provider_id, p.api_key).await?)
        }
        AiRequest::Test(p) => AiResponse::Test(ws.ai_test(core, &p.provider_id, p.api_key).await?),
    })
}

#[cfg(not(feature = "ai"))]
pub(crate) async fn ai(
    _: &Core,
    _: &Workspace,
    req: AiRequest,
    _: &WriteOrigin,
) -> Result<AiResponse, RpcError> {
    match req {
        AiRequest::Chat(_) => Err(chat_is_a_stream()),
        _ => Err(RpcError::not_supported("The AI assistant")),
    }
}

fn chat_is_a_stream() -> RpcError {
    RpcError::invalid_argument(
        "ai.chat is served by the stream transport (core_stream, /rpc/stream), not as a \
         single call",
    )
}

/// `ai.chat` on `ws`: the turn's events as [`CoreEvent::Ai`]s.
#[cfg(feature = "ai")]
pub(crate) fn chat<'a>(
    core: &'a Core,
    ws: &'a Workspace,
    params: ChatParams,
    origin: WriteOrigin,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    use futures::StreamExt;

    let stream_id = params.stream_id.clone();
    Ok(Box::pin(ws.ai_chat(core, params, origin).map(
        move |event| CoreEvent::Ai {
            stream_id: stream_id.clone(),
            event,
        },
    )))
}

#[cfg(not(feature = "ai"))]
pub(crate) fn chat<'a>(
    _: &'a Core,
    _: &'a Workspace,
    _: ChatParams,
    _: WriteOrigin,
) -> Result<BoxStream<'a, CoreEvent>, RpcError> {
    Err(RpcError::not_supported("The AI assistant"))
}
