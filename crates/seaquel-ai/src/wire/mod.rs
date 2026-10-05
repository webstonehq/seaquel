//! The providers' wire: what a round, the inline prompt and the model list
//! send, and how the answers decode.
//!
//! Request bodies are the TypeScript client's (`services/ai/providers.ts`
//! before phase 6), field for field: `model`, `max_tokens` (Anthropic only:
//! 4,096 for a round, 2,048 for the inline prompt), `system` (Anthropic) or
//! a first `system` message (OpenAI-compatible), `messages`, `tools`
//! (only when there are some) and `stream`. What differs on purpose:
//! every tool call of a round is sent back with its result,
//! a tool error is marked (`is_error` on Anthropic, `Error: ` on
//! OpenAI-compatible), and the browser client adds Anthropic's
//! direct-access header.
//!
//! Decoding: every tool call of a round comes out, in order;
//! an `error` event, a malformed event or a stream that ends before the
//! model finished is a [`WireError`] (`PROVIDER_ERROR`); `max_tokens` is
//! [`StopReason::MaxTokens`]; a 429 is `RATE_LIMITED`.

mod anthropic;
mod openai;

pub use openai::TOOL_ERROR_PREFIX;

use std::fmt;

use serde_json::Value;

use crate::http::{HttpRequest, Method, Redacted};
use crate::sse::{SseEvent, SseParser};

/// `anthropic-version`, as the TypeScript client sent it.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Anthropic's API, when the provider names no base URL.
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
/// An OpenAI-compatible provider's base URL when it names none.
pub const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";
/// `max_tokens` of a round (Anthropic; OpenAI-compatible requests send none).
pub const ROUND_MAX_TOKENS: u32 = 4096;
/// `max_tokens` of the inline prompt's request (Anthropic).
pub const GENERATE_MAX_TOKENS: u32 = 2048;
/// How much of a provider's error message is kept for the user (never for
/// a log): 1 KB, cut on a character boundary.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 1024;
/// How much of a non-2xx body, or a non-streaming answer, is read.
pub const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;
/// How much of a `/models` answer is read: OpenRouter's list is about a
/// megabyte.
pub const MAX_MODELS_BODY_BYTES: usize = 8 * 1024 * 1024;
/// A round's budget, so a provider can't grow the decoder without bound
/// (the web server decodes for every user): tool calls or content blocks
/// open at once,
pub const MAX_OPEN_CALLS: usize = 64;
/// tool arguments across the round,
pub const MAX_TOOL_ARGUMENT_BYTES: usize = 16 * 1024 * 1024;
/// and text across the round. Past any of them the round is `Malformed`.
pub const MAX_ROUND_TEXT_BYTES: usize = 16 * 1024 * 1024;
/// A tool call's name and id: real ones are short identifiers.
pub const MAX_TOOL_NAME_BYTES: usize = 256;
/// Tool calls in one round in all (a closed Anthropic block frees its open
/// slot, not this). Above Core's 20 per turn, so Core's `TOOL_LIMIT` is
/// reached first as the events stream.
pub const MAX_ROUND_CALLS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    Anthropic,
    OpenAiCompatible,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::OpenAiCompatible => "openai-compatible",
        }
    }
}

/// Which provider a request goes to. `base_url` is the OpenAI-compatible
/// one (`https://api.openai.com/v1` when `None`; one trailing `/` is
/// dropped, as the TypeScript did); for Anthropic it replaces
/// `https://api.anthropic.com`, which only tests do.
#[derive(Clone, PartialEq, Eq)]
pub struct Provider {
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub model: String,
}

impl fmt::Debug for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provider")
            .field("kind", &self.kind)
            .field("host", &self.base_url.as_deref().map(crate::http::host_of))
            .field("model", &self.model)
            .finish()
    }
}

/// A tool the model may call: its name, description and JSON schema.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// One tool call the model asked for.
#[derive(Clone, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    /// The arguments as the model sent them (a JSON object; `{}` when it
    /// sent none).
    pub input: Value,
}

impl fmt::Debug for ToolCall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolCall")
            .field("id", &self.id)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

/// A tool call's result as the model gets it.
#[derive(Clone, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    /// A refusal or failure: `is_error: true` on Anthropic,
    /// the content prefixed `Error: ` on OpenAI-compatible.
    pub is_error: bool,
}

impl fmt::Debug for ToolResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolResult")
            .field("call_id", &self.call_id)
            .field("is_error", &self.is_error)
            .field("content_bytes", &self.content.len())
            .finish()
    }
}

/// A message in a round, provider-neutral.
#[derive(Clone, PartialEq)]
pub enum Message {
    /// The user's text (or, in history, any stored user message).
    User(String),
    /// The model's text and the tool calls it made after it. With no
    /// calls the content goes as a plain string, as stored history did.
    Assistant {
        text: String,
        tool_calls: Vec<ToolCall>,
    },
    /// The results of the previous assistant message's calls, in its
    /// order: one user message of `tool_result` blocks (Anthropic), one
    /// `tool` message each (OpenAI-compatible).
    ToolResults(Vec<ToolResult>),
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Message::User(t) => write!(f, "User({} bytes)", t.len()),
            Message::Assistant { text, tool_calls } => f
                .debug_struct("Assistant")
                .field("text_bytes", &text.len())
                .field("tool_calls", tool_calls)
                .finish(),
            Message::ToolResults(r) => f.debug_tuple("ToolResults").field(r).finish(),
        }
    }
}

/// What one round sends.
#[derive(Clone, PartialEq)]
pub struct Round {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
}

impl fmt::Debug for Round {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Round")
            .field("system_bytes", &self.system.len())
            .field("messages", &self.messages)
            .field(
                "tools",
                &self
                    .tools
                    .iter()
                    .map(|t| t.name.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Why the model stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    /// It finished (`end_turn`, `stop_sequence`, `stop`, and anything
    /// else that isn't one of the two below).
    End,
    /// It wants its tool calls run (`tool_use`, `tool_calls`).
    ToolUse,
    /// It ran out of tokens (`max_tokens`, `length`).
    MaxTokens,
}

/// Token counts from `usage`, when the provider sent them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

/// What a round's stream decodes to, in order. A stream that decodes
/// cleanly ends with exactly one `Stop`, and every `ToolCall` comes before
/// it.
#[derive(Clone, PartialEq)]
pub enum RoundEvent {
    Text(String),
    ToolCall(ToolCall),
    Usage(Usage),
    Stop(StopReason),
}

impl fmt::Debug for RoundEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoundEvent::Text(t) => write!(f, "Text({} bytes)", t.len()),
            RoundEvent::ToolCall(c) => f.debug_tuple("ToolCall").field(c).finish(),
            RoundEvent::Usage(u) => f.debug_tuple("Usage").field(u).finish(),
            RoundEvent::Stop(s) => f.debug_tuple("Stop").field(s).finish(),
        }
    }
}

/// A provider's answer that isn't a result. `message` is the provider's
/// own text, cut at [`MAX_ERROR_MESSAGE_BYTES`], for the user only:
/// `Debug` and `Display` leave it out, so a log line never carries it.
#[derive(Clone, PartialEq, Eq)]
pub enum WireError {
    /// A non-2xx status (a 3xx too: redirects aren't followed).
    Status {
        status: u16,
        error_type: Option<String>,
        message: Option<String>,
    },
    /// An `error` event in the stream.
    Stream {
        error_type: Option<String>,
        message: Option<String>,
    },
    /// Something the wire doesn't allow: bad JSON, a delta for no block, a
    /// tool call without a name, an event past the size cap. The `&str`
    /// says which, never what was sent.
    Malformed(&'static str),
    /// The stream ended before the model finished.
    Incomplete,
}

impl WireError {
    /// `RATE_LIMITED` for a 429, `PROVIDER_ERROR` otherwise.
    pub fn code(&self) -> &'static str {
        match self {
            WireError::Status { status: 429, .. } => "RATE_LIMITED",
            _ => "PROVIDER_ERROR",
        }
    }

    /// The HTTP status, if the error is one.
    pub fn status(&self) -> Option<u16> {
        match self {
            WireError::Status { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// The provider's error type (`overloaded_error`, `invalid_request_error`,
    /// …), only when it is a short identifier. Safe to log.
    pub fn error_type(&self) -> Option<&str> {
        match self {
            WireError::Status { error_type, .. } | WireError::Stream { error_type, .. } => {
                error_type.as_deref()
            }
            _ => None,
        }
    }

    /// The provider's message, cut at 1 KB. For the chat, never a log.
    pub fn provider_message(&self) -> Option<&str> {
        match self {
            WireError::Status { message, .. } | WireError::Stream { message, .. } => {
                message.as_deref()
            }
            _ => None,
        }
    }
}

impl fmt::Debug for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WireError({self})")
    }
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::Status { status, .. } => match self.error_type() {
                Some(t) => write!(f, "the provider answered HTTP {status} ({t})"),
                None => write!(f, "the provider answered HTTP {status}"),
            },
            WireError::Stream { .. } => match self.error_type() {
                Some(t) => write!(f, "the provider sent an error ({t})"),
                None => f.write_str("the provider sent an error"),
            },
            WireError::Malformed(what) => {
                write!(f, "the provider sent a malformed answer ({what})")
            }
            WireError::Incomplete => {
                f.write_str("the provider's answer ended before the model finished")
            }
        }
    }
}

impl std::error::Error for WireError {}

/// The base URL requests go to, with one trailing `/` dropped (the
/// TypeScript's `replace(/\/$/, "")`); the provider's default when that
/// leaves nothing.
fn base_url(p: &Provider) -> String {
    let given = p.base_url.as_deref().unwrap_or("");
    let given = given.strip_suffix('/').unwrap_or(given);
    if !given.is_empty() {
        return given.to_string();
    }
    match p.kind {
        ProviderKind::Anthropic => ANTHROPIC_BASE_URL.to_string(),
        ProviderKind::OpenAiCompatible => OPENAI_BASE_URL.to_string(),
    }
}

/// Where a request goes: Anthropic's paths sit under `/v1`, an
/// OpenAI-compatible base URL already ends with its version.
fn url(p: &Provider, path: &str) -> String {
    match p.kind {
        ProviderKind::Anthropic => format!("{}/v1/{path}", base_url(p)),
        ProviderKind::OpenAiCompatible => format!("{}/{path}", base_url(p)),
    }
}

/// The headers, in the TypeScript's order. `json`: the request has a body.
fn headers(
    p: &Provider,
    key: Option<&str>,
    json: bool,
    browser: bool,
) -> Vec<(String, Redacted<String>)> {
    let key = key.filter(|k| !k.is_empty());
    let mut h = Vec::new();
    if json {
        h.push(header("content-type", "application/json"));
    }
    match p.kind {
        ProviderKind::Anthropic => {
            if let Some(k) = key {
                h.push(header("x-api-key", k));
            }
            h.push(header("anthropic-version", ANTHROPIC_VERSION));
            if browser {
                // Anthropic refuses a browser's (CORS) request without it.
                h.push(header("anthropic-dangerous-direct-browser-access", "true"));
            }
        }
        ProviderKind::OpenAiCompatible => {
            if let Some(k) = key {
                h.push(header("authorization", &format!("Bearer {k}")));
            }
        }
    }
    h
}

/// The request for one round of a turn (streaming).
pub fn round_request(p: &Provider, key: Option<&str>, round: &Round, browser: bool) -> HttpRequest {
    let (path, body) = match p.kind {
        ProviderKind::Anthropic => ("messages", anthropic::round_body(&p.model, round)),
        ProviderKind::OpenAiCompatible => ("chat/completions", openai::round_body(&p.model, round)),
    };
    request(
        Method::Post,
        url(p, path),
        headers(p, key, true, browser),
        &body,
    )
}

/// The inline prompt's request (not streaming): `system` and plain-text
/// messages, `max_tokens` 2,048 on Anthropic.
pub fn generate_request(
    p: &Provider,
    key: Option<&str>,
    system: &str,
    messages: &[(Role, String)],
    browser: bool,
) -> HttpRequest {
    let (path, body) = match p.kind {
        ProviderKind::Anthropic => (
            "messages",
            anthropic::generate_body(&p.model, system, messages),
        ),
        ProviderKind::OpenAiCompatible => (
            "chat/completions",
            openai::generate_body(&p.model, system, messages),
        ),
    };
    request(
        Method::Post,
        url(p, path),
        headers(p, key, true, browser),
        &body,
    )
}

/// `GET …/models`: the model list, and the provider test.
pub fn models_request(p: &Provider, key: Option<&str>, browser: bool) -> HttpRequest {
    request(
        Method::Get,
        url(p, "models"),
        headers(p, key, false, browser),
        &Value::Null,
    )
}

/// A plain message's role, for [`generate_request`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

/// `Ok` for a 2xx; otherwise the error, with the provider's error type and
/// message read from `body` (both providers' error shapes).
pub fn check_status(status: u16, body: &[u8]) -> Result<(), WireError> {
    check_status_with(status, body, None)
}

/// [`check_status`], with every occurrence of `secret` (the key the request
/// was sent with) in the provider's message replaced by `<redacted>` before
/// it is cut: some providers echo the key they refused.
pub fn check_status_with(status: u16, body: &[u8], secret: Option<&str>) -> Result<(), WireError> {
    if (200..300).contains(&status) {
        return Ok(());
    }
    let (error_type, message) = match serde_json::from_slice::<Value>(body) {
        Ok(v) => error_fields(&v, secret),
        Err(_) => {
            let text = String::from_utf8_lossy(body);
            (None, cut_message(Some(text.trim()), secret))
        }
    };
    Err(WireError::Status {
        status,
        error_type,
        message,
    })
}

/// The type and message of an error body or event, from either shape:
/// `{"error": {"type", "message"}}` (both providers), `{"error": "…"}` and
/// `{"message": "…"}` (some OpenAI-compatible servers).
pub(crate) fn error_fields(v: &Value, secret: Option<&str>) -> (Option<String>, Option<String>) {
    match &v["error"] {
        Value::Object(e) => (
            safe_error_type(e.get("type").and_then(Value::as_str)),
            cut_message(e.get("message").and_then(Value::as_str), secret),
        ),
        Value::String(m) => (None, cut_message(Some(m), secret)),
        _ => (None, cut_message(v["message"].as_str(), secret)),
    }
}

/// The text of a non-streaming answer (the inline prompt): Anthropic's
/// first `text` block, OpenAI's `choices[0].message.content`; `""` when
/// there is none, as the TypeScript gave.
pub fn decode_generate(kind: ProviderKind, body: &[u8]) -> Result<String, WireError> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|_| WireError::Malformed("an answer that isn't JSON"))?;
    let text = match kind {
        ProviderKind::Anthropic => v["content"]
            .as_array()
            .and_then(|blocks| blocks.iter().find(|b| b["type"] == "text"))
            .and_then(|b| b["text"].as_str()),
        ProviderKind::OpenAiCompatible => v["choices"][0]["message"]["content"].as_str(),
    };
    Ok(text.unwrap_or("").to_string())
}

/// The model ids of a `/models` answer (`data[].id`, both providers).
pub fn decode_models(body: &[u8]) -> Result<Vec<String>, WireError> {
    let v: Value = serde_json::from_slice(body)
        .map_err(|_| WireError::Malformed("an answer that isn't JSON"))?;
    let data = v["data"]
        .as_array()
        .ok_or(WireError::Malformed("a model list without `data`"))?;
    Ok(data
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect())
}

/// Turns a round's streamed body into [`RoundEvent`]s. Feed it bytes as
/// they arrive, then call [`Decoder::finish`] at the end of the body. After
/// an error the decoder is spent: feed nothing more.
///
/// Deliberately not `Debug`: it may hold the request's key (to redact it
/// from an error event).
pub struct Decoder {
    sse: SseParser,
    events: Vec<SseEvent>,
    inner: Inner,
    secret: Option<String>,
}

enum Inner {
    Anthropic(anthropic::State),
    OpenAi(openai::State),
}

impl Decoder {
    pub fn new(kind: ProviderKind) -> Self {
        Self::with_secret(kind, None)
    }

    /// A decoder that replaces `secret` (the request's key) in an error
    /// event's message with `<redacted>`, as [`check_status_with`] does.
    pub fn with_secret(kind: ProviderKind, secret: Option<&str>) -> Self {
        Self {
            sse: SseParser::new(),
            events: Vec::new(),
            inner: match kind {
                ProviderKind::Anthropic => Inner::Anthropic(anthropic::State::default()),
                ProviderKind::OpenAiCompatible => Inner::OpenAi(openai::State::default()),
            },
            secret: secret.filter(|s| !s.is_empty()).map(String::from),
        }
    }

    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<RoundEvent>) -> Result<(), WireError> {
        self.sse
            .feed(bytes, &mut self.events)
            .map_err(|_| TOO_LARGE)?;
        self.handle(out)
    }

    pub fn finish(&mut self, out: &mut Vec<RoundEvent>) -> Result<(), WireError> {
        self.sse.finish(&mut self.events).map_err(|_| TOO_LARGE)?;
        self.handle(out)?;
        match &mut self.inner {
            Inner::Anthropic(s) => s.finish(out),
            Inner::OpenAi(s) => s.finish(out),
        }
    }

    fn handle(&mut self, out: &mut Vec<RoundEvent>) -> Result<(), WireError> {
        let secret = self.secret.as_deref();
        for event in std::mem::take(&mut self.events) {
            match &mut self.inner {
                Inner::Anthropic(s) => s.handle(&event, out, secret)?,
                Inner::OpenAi(s) => s.handle(&event, out, secret)?,
            }
        }
        Ok(())
    }
}

/// Keeps a provider's error type only when it is a short identifier
/// (letters, digits, `_`, `.`, `-`; at most 64), so it is safe to log.
pub(crate) fn safe_error_type(v: Option<&str>) -> Option<String> {
    let v = v?;
    let ok = (1..=64).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    ok.then(|| v.to_string())
}

/// What replaces the request's key in a provider's message.
pub const REDACTED: &str = "<redacted>";

/// `message` with every occurrence of `secret` (when it's not empty)
/// replaced by [`REDACTED`], then cut at [`MAX_ERROR_MESSAGE_BYTES`] on a
/// character boundary. Redacting first means a key straddling the cut
/// can't leave its first bytes.
pub(crate) fn cut_message(v: Option<&str>, secret: Option<&str>) -> Option<String> {
    let v = v.filter(|m| !m.is_empty())?;
    let redacted;
    let v = match secret.filter(|s| !s.is_empty()) {
        Some(secret) if v.contains(secret) => {
            redacted = v.replace(secret, REDACTED);
            redacted.as_str()
        }
        _ => v,
    };
    let mut end = v.len().min(MAX_ERROR_MESSAGE_BYTES);
    while !v.is_char_boundary(end) {
        end -= 1;
    }
    Some(v[..end].to_string())
}

/// What a round has used of its budget (`MAX_*` above).
#[derive(Default)]
pub(crate) struct Budget {
    args: usize,
    text: usize,
    calls: usize,
}

impl Budget {
    pub(crate) fn text(&mut self, bytes: usize) -> Result<(), WireError> {
        self.text += bytes;
        if self.text > MAX_ROUND_TEXT_BYTES {
            return Err(WireError::Malformed("text past the round's 16 MiB"));
        }
        Ok(())
    }

    pub(crate) fn args(&mut self, bytes: usize) -> Result<(), WireError> {
        self.args += bytes;
        if self.args > MAX_TOOL_ARGUMENT_BYTES {
            return Err(WireError::Malformed(
                "tool arguments past the round's 16 MiB",
            ));
        }
        Ok(())
    }

    /// One more tool call in the round.
    pub(crate) fn call(&mut self) -> Result<(), WireError> {
        self.calls += 1;
        if self.calls > MAX_ROUND_CALLS {
            return Err(WireError::Malformed("more than 64 tool calls in a round"));
        }
        Ok(())
    }

    /// A tool call's name or id, at most [`MAX_TOOL_NAME_BYTES`].
    pub(crate) fn name(v: &str) -> Result<&str, WireError> {
        if v.len() > MAX_TOOL_NAME_BYTES {
            return Err(WireError::Malformed(
                "a tool call name or id past 256 bytes",
            ));
        }
        Ok(v)
    }

    /// Before opening one more call or block when `open` are open.
    pub(crate) fn open(open: usize) -> Result<(), WireError> {
        if open >= MAX_OPEN_CALLS {
            return Err(WireError::Malformed(
                "more than 64 tool calls or blocks open",
            ));
        }
        Ok(())
    }
}

const TOO_LARGE: WireError = WireError::Malformed("an event past 16 MiB");

/// A tool call's accumulated arguments as a JSON object: `{}` for none,
/// [`WireError::Malformed`] for anything that isn't an object.
pub(crate) fn tool_input(raw: &str) -> Result<Value, WireError> {
    if raw.trim().is_empty() {
        return Ok(Value::Object(Default::default()));
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(v @ Value::Object(_)) => Ok(v),
        _ => Err(WireError::Malformed(
            "tool arguments that aren't a JSON object",
        )),
    }
}

/// An SSE event's data as JSON.
pub(crate) fn event_json(data: &str) -> Result<Value, WireError> {
    serde_json::from_str(data).map_err(|_| WireError::Malformed("an event that isn't JSON"))
}

fn header(name: &str, value: &str) -> (String, Redacted<String>) {
    (name.to_string(), Redacted::new(value.to_string()))
}

fn request(
    method: Method,
    url: String,
    headers: Vec<(String, Redacted<String>)>,
    body: &Value,
) -> HttpRequest {
    HttpRequest {
        method,
        url,
        headers,
        body: match method {
            Method::Get => Vec::new(),
            Method::Post => serde_json::to_vec(body).unwrap_or_default(),
        },
    }
}
