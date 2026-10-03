//! The assistant (phase 6): Core runs a turn, the inline prompt, the model
//! list and the provider test. What the model is sent, which tools it may
//! call, how a result is cut and what is stored all come from
//! `seaquel-ai`, re-exported here (`http`, `limits`, `prompt`, `sharing`,
//! `sse`, `wire`, and the registry as [`tools`]); this module is Core's
//! side of it:
//!
//! - [`Workspace::ai_chat`]: one turn (`turn.rs`), its tools run on the
//!   paths Core already has (`tools.rs`), its key from the call or the
//!   keychain (`keys.rs`), its two chat writes (`chats.rs`), and what it
//!   waits for (`waiters.rs`, answered by [`Workspace::ai_respond`]);
//! - [`Workspace::ai_generate`], [`Workspace::ai_models`],
//!   [`Workspace::ai_test`].
//!
//! Two builder policies with no default: an `HttpClient`
//! ([`crate::CoreBuilder::ai_http`]) and an [`AiEgress`]
//! ([`crate::CoreBuilder::ai_egress`]); without either every model call is
//! `NOT_SUPPORTED`. `ai-native` adds the native client as [`native`].

mod chats;
mod keys;
pub mod tools;
mod turn;
mod waiters;

use std::time::Duration;

pub use seaquel_ai::{http, limits, prompt, sharing, sse, wire, AiLimits, Sharing};
pub use seaquel_workspace::ai::{
    AiDecision, AiEvent, AiStop, Approval, ApprovalDecision, ChatParams, ChatUserMessage,
    ClientResult, GenerateParams, SuppliedSecret, REPLY_CUT_NOTE,
};

pub use chats::CHAT_FULL;
pub use turn::TOOL_LIMIT;
pub use waiters::{NOT_FOUND, TURN_IN_PROGRESS};

/// The native HTTP client (`seaquel-http`): reqwest with rustls, the OS
/// and webpki roots, the egress guard (Decision 9) and Decision 8's
/// timeouts. Build it with the same egress as [`AiEgress`].
#[cfg(feature = "ai-native")]
pub mod native {
    pub use seaquel_http::{Egress, NativeHttp, NativeHttpOptions};

    impl From<super::AiEgress> for Egress {
        fn from(e: super::AiEgress) -> Self {
            match e {
                super::AiEgress::Off => Egress::Off,
                super::AiEgress::Public => Egress::Public,
                super::AiEgress::Any => Egress::Any,
            }
        }
    }

    /// `SEAQUEL_AI_EGRESS` as Core's policy (`Egress::parse` reads it).
    impl From<Egress> for super::AiEgress {
        fn from(e: Egress) -> Self {
            match e {
                Egress::Off => super::AiEgress::Off,
                Egress::Public => super::AiEgress::Public,
                Egress::Any => super::AiEgress::Any,
            }
        }
    }
}

use futures::future::{select, Either};
use futures::{Future, StreamExt};
use log::{debug, info, warn};
use seaquel_ai::http::{HttpClient, HttpError, HttpErrorKind, HttpRequest, HttpResponse};
use seaquel_ai::wire::{Provider, ProviderKind, Role, WireError};
use seaquel_engine::{BoxStream, CancellationToken};
use seaquel_runtime::Executor;
use seaquel_sql::SqlEngine;
use seaquel_storage::{app_state, connections, projects};
use seaquel_types::storage::PersistedConnection;
use seaquel_workspace::state::{read_ai_settings, AI_PROVIDER_NOT_FOUND};

use crate::changes::WriteOrigin;
use crate::{Core, CoreError, Workspace};

/// Where model calls may go (Decision 9). There is no default: a Core
/// built without [`crate::CoreBuilder::ai_egress`] answers
/// `NOT_SUPPORTED` to every model call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiEgress {
    /// Refuse every model call with `AI_EGRESS_BLOCKED` (an air-gapped
    /// web install).
    Off,
    /// Public addresses only, `https:` only (the web's default).
    Public,
    /// Anywhere, `http:` included (the desktop, the CLI, a web install
    /// next to its own model server).
    Any,
}

// ── Codes (Decision 15) ──

pub const NO_PROVIDER: &str = "NO_PROVIDER";
pub const NO_MODEL: &str = "NO_MODEL";
pub const NO_API_KEY: &str = "NO_API_KEY";
pub const AI_DISABLED: &str = "AI_DISABLED";
pub const AI_EGRESS_BLOCKED: &str = "AI_EGRESS_BLOCKED";
pub const PROVIDER_ERROR: &str = "PROVIDER_ERROR";
pub const RATE_LIMITED: &str = "RATE_LIMITED";
pub const CONNECTION_MISMATCH: &str = "CONNECTION_MISMATCH";
/// A supplied key is for another provider than the connection's now
/// (Task 7 review I1): refused before any request.
pub const AI_PROVIDER_CHANGED: &str = "AI_PROVIDER_CHANGED";
pub const TIMEOUT: &str = "TIMEOUT";
/// A user's message (or the inline prompt's request or editor text) past
/// [`AiLimits::max_message_bytes`] (the web's 1 MiB; F1/F2/F5 review P1),
/// so the page can say so in words.
pub const MESSAGE_TOO_LONG: &str = "MESSAGE_TOO_LONG";
/// Past [`AiLimits::max_turns_in_flight`].
pub const TOO_MANY_REQUESTS: &str = "TOO_MANY_REQUESTS";
const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
const NOT_SUPPORTED: &str = "NOT_SUPPORTED";

/// Whether model calls leave from a web page (the demo's module): the
/// wire then adds Anthropic's direct-access header, without which it
/// refuses a browser's request (bug 1, Decision 10). Native clients don't.
pub(crate) const FROM_BROWSER: bool = cfg!(feature = "browser");

/// No byte from the provider for this long ends the call (Decision 8).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// One round (or one non-streaming call) may take this long in all.
pub const ROUND_TIMEOUT: Duration = Duration::from_secs(600);

/// A workspace's turns and their waiters.
#[derive(Default)]
pub(crate) struct WorkspaceAi {
    pub(crate) waiters: waiters::Waiters,
}

/// A refusal or failure with its code and the message the page shows.
/// Never a key, a URL's path or the provider's text beyond Decision 10's
/// 1 KiB.
#[derive(Clone, Debug)]
pub(crate) struct Fail {
    pub(crate) code: String,
    pub(crate) message: String,
}

impl Fail {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub(crate) fn not_supported() -> Self {
        Self::new(NOT_SUPPORTED, "The AI assistant isn't available here.")
    }

    pub(crate) fn disabled() -> Self {
        Self::new(
            AI_DISABLED,
            "The AI assistant is turned off in Settings > AI.",
        )
    }

    pub(crate) fn timeout() -> Self {
        Self::new(TIMEOUT, "The provider didn't answer in time.")
    }

    pub(crate) fn event(self) -> AiEvent {
        AiEvent::error(self.code, self.message)
    }
}

impl From<CoreError> for Fail {
    fn from(e: CoreError) -> Self {
        Self::new(e.code, e.message)
    }
}

impl From<seaquel_storage::StorageError> for Fail {
    fn from(e: seaquel_storage::StorageError) -> Self {
        CoreError::from(e).into()
    }
}

impl From<Fail> for CoreError {
    fn from(f: Fail) -> Self {
        CoreError::new(f.code, f.message)
    }
}

/// What a model call needs, read from storage and the call.
pub(crate) struct Resolved {
    /// The saved connection whose provider, model and sharing apply.
    pub(crate) row: PersistedConnection,
    pub(crate) project_name: String,
    pub(crate) provider: Provider,
    pub(crate) key: Option<keys::ApiKey>,
    pub(crate) sharing: Sharing,
    /// The SQL rules: the open connection's (a turn), else the row's type.
    pub(crate) engine: SqlEngine,
}

/// The policies a model call needs: the client and egress
/// (`NOT_SUPPORTED`), and not `Off` (`AI_EGRESS_BLOCKED`).
fn ready(core: &Core) -> Result<&dyn HttpClient, Fail> {
    let (Some(http), Some(egress)) = (core.ai_http.as_deref(), core.ai_egress) else {
        return Err(Fail::not_supported());
    };
    if egress == AiEgress::Off {
        return Err(egress_off());
    }
    Ok(http)
}

fn egress_off() -> Fail {
    Fail::new(
        AI_EGRESS_BLOCKED,
        "Model calls are turned off on this server.",
    )
}

/// `http:` goes only under [`AiEgress::Any`] (Decision 9); the client
/// checks the rest of the rule.
fn check_scheme(core: &Core, provider: &Provider) -> Result<(), Fail> {
    let plain = provider
        .base_url
        .as_deref()
        .is_some_and(|u| u.trim_start().to_ascii_lowercase().starts_with("http:"));
    if plain && core.ai_egress == Some(AiEgress::Public) {
        return Err(Fail::new(
            AI_EGRESS_BLOCKED,
            "This server allows model calls over https only.",
        ));
    }
    Ok(())
}

fn provider_kind(ty: &str) -> ProviderKind {
    match ty {
        "openai-compatible" => ProviderKind::OpenAiCompatible,
        _ => ProviderKind::Anthropic,
    }
}

/// A provider's wire settings: Anthropic always goes to its API (as the
/// TypeScript did; a stored base URL is an OpenAI-compatible one's).
fn wire_provider(info: &seaquel_workspace::state::ProviderInfo, model: String) -> Provider {
    let kind = provider_kind(&info.ty);
    Provider {
        kind,
        base_url: match kind {
            ProviderKind::OpenAiCompatible => info.base_url.clone(),
            ProviderKind::Anthropic => None,
        },
        model,
    }
}

/// The engine a saved connection's type names.
fn engine_of_type(ty: &str) -> Result<SqlEngine, Fail> {
    ty.parse()
        .map_err(|_| Fail::new(NOT_SUPPORTED, "This connection's engine has no assistant."))
}

/// The provider, key and model of saved connection `saved_id`, in order:
/// `NO_PROVIDER`, `AI_DISABLED` and an `http:` provider under public
/// egress (both before the keychain is read, so a turned-off assistant
/// reads no key), `NO_API_KEY`, `NO_MODEL`.
pub(crate) async fn resolve(
    ws: &Workspace,
    core: &Core,
    saved_id: &str,
    supplied: Option<SuppliedSecret>,
    supplied_for: Option<&str>,
) -> Result<Resolved, Fail> {
    ready(core)?;
    let row = connections::get(ws.storage(), saved_id)
        .await?
        .ok_or_else(|| {
            Fail::new(
                crate::SAVED_CONNECTION_NOT_FOUND,
                "The chat's connection no longer exists.",
            )
        })?;
    let raw = app_state::get(ws.storage(), seaquel_workspace::state::AI_SETTINGS_KEY).await?;
    let settings = read_ai_settings(raw.as_deref());
    let info = match row.active_ai_provider_id.as_deref() {
        Some(id) => settings
            .provider(id)
            .ok_or_else(|| Fail::new(NO_PROVIDER, "The chat's AI provider no longer exists."))?,
        None if settings.provider_count() == 0 => {
            return Err(Fail::new(NO_PROVIDER, "No AI provider is configured."))
        }
        // Providers exist and none is chosen: choosing a model chooses one.
        None => return Err(no_model()),
    };
    let provider_id = row.active_ai_provider_id.clone().unwrap_or_default();
    check_supplied_for(supplied.is_some(), supplied_for, &provider_id)?;
    if !settings.enabled() {
        return Err(Fail::disabled());
    }
    check_scheme(core, &wire_provider(&info, String::new()))?;
    let key = keys::api_key(ws, &provider_id, supplied).await?;
    let kind = provider_kind(&info.ty);
    if key.is_none() && kind == ProviderKind::Anthropic {
        return Err(no_api_key());
    }
    let model = row
        .active_ai_model
        .clone()
        .filter(|m| !m.is_empty())
        .ok_or_else(no_model)?;
    let provider = wire_provider(&info, model);
    let global = sharing::global_sharing_from(raw.as_deref());
    let project_name = projects::get(ws.storage(), &row.project_id)
        .await?
        .map(|p| p.name)
        .unwrap_or_default();
    Ok(Resolved {
        sharing: sharing::sharing(&row, global),
        engine: engine_of_type(&row.ty)?,
        project_name,
        provider,
        key,
        row,
    })
}

/// A supplied key goes only to the provider it was read for (review I1):
/// the page names it, and a key for another provider than the one Core
/// resolved (the connection's changed meanwhile) is `AI_PROVIDER_CHANGED`.
fn check_supplied_for(
    supplied: bool,
    supplied_for: Option<&str>,
    provider_id: &str,
) -> Result<(), Fail> {
    if !supplied {
        return Ok(());
    }
    match supplied_for {
        None => Err(Fail::new(
            INVALID_ARGUMENT,
            "An apiKey needs the providerId it belongs to.",
        )),
        Some(id) if id != provider_id => Err(Fail::new(
            AI_PROVIDER_CHANGED,
            "The connection's AI provider changed.",
        )),
        Some(_) => Ok(()),
    }
}

fn no_model() -> Fail {
    Fail::new(NO_MODEL, "No model is chosen for this connection.")
}

fn no_api_key() -> Fail {
    Fail::new(NO_API_KEY, "No API key is set for this provider.")
}

/// A connection's sharing as stored now, and its name (Decision 22): read
/// before every tool call. A row that's gone shares nothing.
pub(crate) async fn sharing_now(ws: &Workspace, saved_id: &str) -> (Sharing, String) {
    let row = connections::get(ws.storage(), saved_id)
        .await
        .ok()
        .flatten();
    let raw = app_state::get(ws.storage(), seaquel_workspace::state::AI_SETTINGS_KEY)
        .await
        .ok()
        .flatten();
    match row {
        Some(row) => (
            sharing::sharing(&row, sharing::global_sharing_from(raw.as_deref())),
            row.name,
        ),
        None => (
            Sharing {
                schema: false,
                data: false,
            },
            String::new(),
        ),
    }
}

// ── Sending, on the executor's clock ──

/// What ends a call: its own token (a turn's stream id), and the
/// workspace's `close_all` (review I1: so an eviction always reaches a
/// turn, whatever happened to its stream entry).
pub(crate) struct Stop<'a> {
    token: &'a CancellationToken,
    closing: Option<&'a CancellationToken>,
}

impl<'a> Stop<'a> {
    pub(crate) fn new(
        token: &'a CancellationToken,
        closing: Option<&'a CancellationToken>,
    ) -> Self {
        Self { token, closing }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.token.is_cancelled() || self.closing.is_some_and(CancellationToken::is_cancelled)
    }

    /// Resolves once either token is cancelled.
    pub(crate) async fn cancelled(&self) {
        match self.closing {
            Some(closing) => {
                select(
                    Box::pin(self.token.cancelled()),
                    Box::pin(closing.cancelled()),
                )
                .await;
            }
            None => self.token.cancelled().await,
        }
    }
}

/// What a race ended with.
pub(crate) enum Next<T> {
    Item(T),
    Timer,
    Cancelled,
}

/// `fut`, unless `wait` passes on the executor's clock or `stop` fires
/// first (then `fut` is dropped). Already stopped: `fut` isn't polled.
pub(crate) async fn race<T>(
    fut: impl Future<Output = T> + Unpin,
    executor: &dyn Executor,
    wait: Duration,
    stop: &Stop<'_>,
) -> Next<T> {
    if stop.is_cancelled() {
        return Next::Cancelled;
    }
    let timer = executor.sleep(wait);
    let cancelled = Box::pin(stop.cancelled());
    match select(fut, select(timer, cancelled)).await {
        Either::Left((item, _)) => Next::Item(item),
        Either::Right((Either::Left(_), _)) => Next::Timer,
        Either::Right((Either::Right(_), _)) => Next::Cancelled,
    }
}

/// Sends `req` and waits for the head, within the idle timeout and
/// `deadline`. `Ok(None)`: cancelled.
pub(crate) async fn send(
    http: &dyn HttpClient,
    executor: &dyn Executor,
    req: HttpRequest,
    stop: &Stop<'_>,
    deadline: Duration,
) -> Result<Option<HttpResponse>, Fail> {
    let wait = IDLE_TIMEOUT.min(deadline.saturating_sub(executor.monotonic()));
    match race(http.send(req), executor, wait, stop).await {
        Next::Item(Ok(response)) => Ok(Some(response)),
        Next::Item(Err(e)) => Err(http_fail(&e)),
        Next::Timer => Err(Fail::timeout()),
        Next::Cancelled => Ok(None),
    }
}

/// A body, at most `max` bytes, within the idle timeout and `deadline`.
/// `Ok(None)`: cancelled.
pub(crate) async fn read_all(
    body: &mut BoxStream<'static, Result<Vec<u8>, HttpError>>,
    executor: &dyn Executor,
    stop: &Stop<'_>,
    deadline: Duration,
    max: usize,
) -> Result<Option<Vec<u8>>, Fail> {
    let mut out = Vec::new();
    loop {
        let wait = IDLE_TIMEOUT.min(deadline.saturating_sub(executor.monotonic()));
        match race(body.next(), executor, wait, stop).await {
            Next::Item(Some(Ok(chunk))) => {
                let room = max.saturating_sub(out.len());
                out.extend_from_slice(&chunk[..chunk.len().min(room)]);
                if out.len() >= max {
                    return Ok(Some(out));
                }
            }
            Next::Item(Some(Err(e))) => return Err(http_fail(&e)),
            Next::Item(None) => return Ok(Some(out)),
            Next::Timer => return Err(Fail::timeout()),
            Next::Cancelled => return Ok(None),
        }
    }
}

/// A failure below the wire, worded for the page; the log gets its kind.
pub(crate) fn http_fail(e: &HttpError) -> Fail {
    warn!(activity = "ai.http", code = e.code(); "A model call failed");
    match e.kind {
        HttpErrorKind::Connect | HttpErrorKind::Other => {
            Fail::new(PROVIDER_ERROR, "Could not reach the provider.")
        }
        HttpErrorKind::Body => Fail::new(
            PROVIDER_ERROR,
            "The provider's stream ended before the reply did.",
        ),
        HttpErrorKind::Timeout => Fail::timeout(),
        HttpErrorKind::EgressBlocked => Fail::new(
            AI_EGRESS_BLOCKED,
            "This server doesn't allow model calls to that address.",
        ),
        HttpErrorKind::InvalidUrl => Fail::new(INVALID_ARGUMENT, "The provider's URL isn't valid."),
    }
}

/// The provider's answer that isn't a result, worded for the page: the
/// provider's own message (cut at 1 KiB) when it gave one, else Core's.
/// The log gets the status, the provider's error type and the code.
pub(crate) fn wire_fail(e: &WireError) -> Fail {
    warn!(activity = "ai.wire", status = e.status(), error_type = e.error_type(), code = e.code(); "The provider answered with an error");
    let message = match e {
        WireError::Status { status, .. } => e
            .provider_message()
            .map(String::from)
            .unwrap_or_else(|| format!("The provider answered HTTP {status}.")),
        WireError::Stream { .. } => e
            .provider_message()
            .unwrap_or("The provider sent an error.")
            .to_string(),
        WireError::Malformed(what) => match *what {
            "an event that isn't JSON" => "The provider sent an event that isn't valid JSON.",
            "an answer that isn't JSON" => "The provider's answer isn't valid JSON.",
            "tool arguments that aren't a JSON object" => {
                "The provider sent a tool call whose input isn't a JSON object."
            }
            "a model list without `data`" => "The provider's answer isn't a model list.",
            _ => "The provider sent an answer Seaquel can't read.",
        }
        .to_string(),
        WireError::Incomplete => "The provider's stream ended before the reply did.".to_string(),
    };
    Fail::new(e.code(), message)
}

/// The first fenced block of `content` (```` ``` ```` or ```` ```sql ````
/// then a newline, up to the next ```` ``` ````), trimmed, else all of it
/// trimmed: the TypeScript's `/```(?:sql)?\n([\s\S]*?)```/`.
pub fn extract_sql(content: &str) -> String {
    let trim = |s: &str| {
        s.trim_matches(seaquel_ai::tools::saved::is_js_space)
            .to_string()
    };
    let mut from = 0;
    while let Some(at) = content[from..].find("```") {
        let open = from + at + 3;
        let rest = &content[open..];
        let body = rest
            .strip_prefix("sql\n")
            .or_else(|| rest.strip_prefix('\n'));
        if let Some(body) = body {
            if let Some(close) = body.find("```") {
                return trim(&body[..close]);
            }
        }
        from = from + at + 1;
    }
    trim(content)
}

impl Workspace {
    /// One turn of the assistant (`ai.chat`): see `turn.rs`. The stream
    /// ends with exactly one `done` or `error`, unless the turn is
    /// cancelled ([`Workspace::cancel`] with its stream id, or
    /// [`Workspace::close_all`]): then the reply is stored with what
    /// streamed and nothing follows. Dropping the stream drops the turn
    /// where it is (the user's message stays stored; the reply isn't).
    pub fn ai_chat<'a>(
        &'a self,
        core: &'a Core,
        params: ChatParams,
        origin: WriteOrigin,
    ) -> BoxStream<'a, AiEvent> {
        turn::chat(self, core, params, origin)
    }

    /// Answers a waiting turn of this workspace (`ai.respond`): an
    /// approval, or a client tool's result. `NOT_FOUND` for a stream or
    /// call this workspace isn't waiting on (another workspace's
    /// included); a second answer to the same call is ignored.
    pub fn ai_respond(
        &self,
        stream_id: &str,
        call_id: &str,
        decision: AiDecision,
    ) -> Result<(), CoreError> {
        self.ai.waiters.respond(stream_id, call_id, decision)
    }

    /// The inline prompt (`ai.generate`, Decision 18): the SQL the model
    /// writes for `request` on saved connection `connection_id`, its first
    /// fenced block. No tools; the schema context when the connection
    /// shares its schema and this workspace has it open.
    pub async fn ai_generate(
        &self,
        core: &Core,
        params: GenerateParams,
    ) -> Result<String, CoreError> {
        // The web's cap on a message applies to what the user typed and the
        // editor's text alike (review M8).
        if let Some(max) = core.ai_limits.max_message_bytes {
            if params.request.len() > max || params.existing_query.len() > max {
                return Err(Fail::new(
                    MESSAGE_TOO_LONG,
                    format!(
                        "The request is longer than allowed here (max_message_bytes: {max} bytes)."
                    ),
                )
                .into());
            }
        }
        let r = resolve(
            self,
            core,
            &params.connection_id,
            params.api_key,
            params.provider_id.as_deref(),
        )
        .await?;
        let executor = core.executor.clone().ok_or_else(Fail::not_supported)?;
        let http = ready(core)?;
        info!(activity = "ai.generate", connection_id = params.connection_id.as_str(), provider = r.provider.kind.as_str(), model = r.provider.model.as_str(); "Generating SQL");
        let tables = if r.sharing.schema {
            self.open_schema(core, &r.row.id).await
        } else {
            Vec::new()
        };
        let context = prompt::schema_context(&tables, limits::SCHEMA_CONTEXT_BYTES);
        let system = prompt::system(r.engine, Some(&context), r.sharing, false, false);
        let existing = &params.existing_query;
        let message = if existing
            .trim_matches(seaquel_ai::tools::saved::is_js_space)
            .is_empty()
        {
            params.request.clone()
        } else {
            format!(
                "{}\n\nExisting query for context:\n```sql\n{existing}\n```",
                params.request
            )
        };
        let req = wire::generate_request(
            &r.provider,
            r.key.as_ref().map(|k| k.expose()),
            &system,
            &[(Role::User, message)],
            FROM_BROWSER,
        );
        let secret = r.key.as_ref().map(|k| k.expose());
        let body = call_once(http, &*executor, req, wire::MAX_ERROR_BODY_BYTES, secret).await?;
        let content = wire::decode_generate(r.provider.kind, &body).map_err(|e| wire_fail(&e))?;
        Ok(extract_sql(&content))
    }

    /// The provider's model ids (`ai.models`, Decision 28).
    pub async fn ai_models(
        &self,
        core: &Core,
        provider_id: &str,
        api_key: Option<SuppliedSecret>,
    ) -> Result<Vec<String>, CoreError> {
        let (http, executor, req, key) = self.models_call(core, provider_id, api_key).await?;
        let secret = key.as_ref().map(|k| k.expose());
        let body = call_once(http, &*executor, req, wire::MAX_MODELS_BODY_BYTES, secret).await?;
        wire::decode_models(&body).map_err(|e| wire_fail(&e).into())
    }

    /// Whether the provider takes the key (`ai.test`, Decision 28): any
    /// 2xx to its model list passes, whatever the body.
    pub async fn ai_test(
        &self,
        core: &Core,
        provider_id: &str,
        api_key: Option<SuppliedSecret>,
    ) -> Result<(), CoreError> {
        let (http, executor, req, key) = self.models_call(core, provider_id, api_key).await?;
        let token = CancellationToken::new();
        let stop = Stop::new(&token, None);
        let deadline = executor.monotonic() + ROUND_TIMEOUT;
        let Some(response) = send(http, &*executor, req, &stop, deadline).await? else {
            return Ok(());
        };
        if (200..300).contains(&response.status) {
            return Ok(());
        }
        let mut body = response.body;
        let bytes = read_all(
            &mut body,
            &*executor,
            &stop,
            deadline,
            wire::MAX_ERROR_BODY_BYTES,
        )
        .await?
        .unwrap_or_default();
        let secret = key.as_ref().map(|k| k.expose());
        let e = wire::check_status_with(response.status, &bytes, secret).expect_err("not a 2xx");
        Err(wire_fail(&e).into())
    }

    async fn models_call<'a>(
        &self,
        core: &'a Core,
        provider_id: &str,
        api_key: Option<SuppliedSecret>,
    ) -> Result<
        (
            &'a dyn HttpClient,
            std::sync::Arc<dyn Executor>,
            HttpRequest,
            Option<keys::ApiKey>,
        ),
        CoreError,
    > {
        let http = ready(core)?;
        let executor = core.executor.clone().ok_or_else(Fail::not_supported)?;
        let raw = app_state::get(self.storage(), seaquel_workspace::state::AI_SETTINGS_KEY).await?;
        let settings = read_ai_settings(raw.as_deref());
        let info = settings
            .provider(provider_id)
            .ok_or_else(|| CoreError::new(AI_PROVIDER_NOT_FOUND, "AI provider not found."))?;
        let key = keys::api_key(self, provider_id, api_key).await?;
        let provider = wire_provider(&info, String::new());
        if key.is_none() && provider.kind == ProviderKind::Anthropic {
            return Err(no_api_key().into());
        }
        check_scheme(core, &provider)?;
        debug!(activity = "ai.models", provider = provider.kind.as_str(); "Listing models");
        let req = wire::models_request(&provider, key.as_ref().map(|k| k.expose()), FROM_BROWSER);
        Ok((http, executor, req, key))
    }

    /// The schema of an open connection of this workspace opened for saved
    /// connection `saved_id`, or nothing.
    async fn open_schema(&self, core: &Core, saved_id: &str) -> Vec<seaquel_types::SchemaTable> {
        let open = core.connections_of(self.id()).into_iter().find(|id| {
            core.connection_as(id, Some(self.id()))
                .is_ok_and(|c| c.saved_connection_id.as_deref() == Some(saved_id))
        });
        let Some(id) = open else {
            return Vec::new();
        };
        let tables = match self.engine(core, &id) {
            Ok(h) => h.schema_tables().await,
            Err(e) => Err(e),
        };
        tables.unwrap_or_else(|e| {
            debug!(activity = "ai.generate", code = e.code.as_str(); "Reading the schema failed");
            Vec::new()
        })
    }

    /// Calls this workspace's turns wait on (tests).
    #[doc(hidden)]
    pub fn ai_waiter_count(&self) -> usize {
        self.ai.waiters.pending()
    }

    /// Turns running in this workspace.
    pub fn ai_turn_count(&self) -> usize {
        self.ai.waiters.turns()
    }
}

/// A non-streaming call: the status checked, the body read whole (at most
/// `max`), within the timeouts. `secret`, the request's key, is redacted
/// from a provider's error message.
async fn call_once(
    http: &dyn HttpClient,
    executor: &dyn Executor,
    req: HttpRequest,
    max: usize,
    secret: Option<&str>,
) -> Result<Vec<u8>, CoreError> {
    let token = CancellationToken::new();
    let stop = Stop::new(&token, None);
    let deadline = executor.monotonic() + ROUND_TIMEOUT;
    let Some(response) = send(http, executor, req, &stop, deadline).await? else {
        return Err(Fail::new("CANCELLED", "The call was cancelled.").into());
    };
    let mut body = response.body;
    let limit = if (200..300).contains(&response.status) {
        max
    } else {
        wire::MAX_ERROR_BODY_BYTES
    };
    let bytes = read_all(&mut body, executor, &stop, deadline, limit)
        .await?
        .unwrap_or_default();
    wire::check_status_with(response.status, &bytes, secret)
        .map_err(|e| CoreError::from(wire_fail(&e)))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::extract_sql;

    #[test]
    fn the_first_fenced_block_is_the_sql() {
        assert_eq!(extract_sql("```sql\nSELECT 1\n```"), "SELECT 1");
        assert_eq!(extract_sql("text\n```\nSELECT 2\n```\nmore"), "SELECT 2");
        assert_eq!(
            extract_sql("```sql\nSELECT 1\n```\nor\n```sql\nSELECT 2\n```"),
            "SELECT 1"
        );
        assert_eq!(extract_sql("  SELECT 3  \n"), "SELECT 3");
        assert_eq!(extract_sql("```sqlx\nnope```"), "```sqlx\nnope```");
        assert_eq!(
            extract_sql("```python x\n```sql\nSELECT 4\n```"),
            "SELECT 4"
        );
        assert_eq!(extract_sql(""), "");
    }
}
