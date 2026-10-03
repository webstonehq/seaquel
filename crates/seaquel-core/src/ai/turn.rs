//! One turn of the assistant (`ai.chat`, Decisions 4–6, 10–13, 22–31).
//!
//! **Order.** Core's refusals come first and store nothing: the transport
//! and egress, the turns-in-flight cap, the message's size and ids, the
//! chat, then `NO_PROVIDER`, `AI_DISABLED` (and an `http:` provider under
//! public egress), `NO_API_KEY`, `NO_MODEL`, `CHAT_FULL` and
//! `CONNECTION_MISMATCH`: a turned-off assistant reads no key. Then the user's message is
//! stored (one write), the rounds run, and the reply is stored (a second
//! write) at `done`, on an error and on a cancel, with what streamed.
//!
//! **Rounds.** Each round's request is built fresh: the history (read once
//! from storage, within 512 KiB), the user's message with its `@mentions`
//! (`history::push_user`), this turn's earlier rounds, and the dashboard
//! line on the user's message. The system prompt and the tool list are the
//! turn's start's. Every tool call of a round runs, in order, whatever the
//! stop reason, and each gets its result; calls are counted as they stream
//! and the 21st ends the turn with `TOOL_LIMIT` before it runs.
//!
//! **Tools.** Before each call the connection's row and the AI settings are
//! read again, so sharing switched off between two calls refuses the
//! second (Decision 22). A query tool waits for `respond` unless the turn
//! allows all; a client tool waits for the page's answer. A wait ends only
//! with the answer or the turn's cancel (its stream id's, or the
//! workspace's `close_all`).
//!
//! **One turn per chat and per stream id**: a second is refused
//! (`TURN_IN_PROGRESS`, `INVALID_ARGUMENT`), and nothing else can take a
//! running turn's stream id, so its cancel always reaches it.
//!
//! **Time** comes from the executor: 120 s without a byte from the
//! provider, 10 minutes per round, 60 s per tool call; `text` events are
//! coalesced to one per 50 ms or 4 KiB, and the rest goes out before any
//! other event.
//!
//! **No storage lock across a round**: each write opens, writes and
//! commits on its own. **No text in logs**: activity, ids, the provider's
//! kind, the model, statuses, codes, counts, durations and token counts.

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc;
use futures::future::{select, Either};
use futures::StreamExt;
use log::{debug, info, warn};
use seaquel_ai::http::{HttpClient, HttpResponse};
use seaquel_ai::limits::{HISTORY_BYTES, MAX_TOOL_CALLS_PER_TURN, SCHEMA_CONTEXT_BYTES};
use seaquel_ai::prompt::history::{
    history, last_dashboard_id, push_user, with_dashboard_context, Part,
};
use seaquel_ai::prompt::{self, MentionDashboard, MentionQuery, MentionWidget};
use seaquel_ai::wire::{
    self, Decoder, Message, Round, RoundEvent, StopReason, ToolCall, ToolResult, ToolSpec,
};
use seaquel_engine::{BoxStream, CancellationToken};
use seaquel_runtime::Executor;
use seaquel_storage::{dashboards, saved_queries};
use seaquel_types::storage::PersistedAIMessage;
use seaquel_workspace::ai::{
    AiDecision, AiEvent, AiStop, Approval, ApprovalDecision, ChatParams, REPLY_CUT_NOTE,
};
use seaquel_workspace::library::ChangeSeq;
use seaquel_workspace::run::iso_timestamp;
use serde_json::Value as Json;

use super::chats::{self, Write};
use super::tools::{
    self, Args, Call, Gate, Planned, Profile, Tool, ToolContext, ToolError, ToolOutput,
};
use super::waiters::Kind;
use super::{Fail, Resolved, Stop};
use crate::changes::WriteOrigin;
use crate::{Core, Workspace};

/// `text` events: at most one per this long,
const COALESCE: Duration = Duration::from_millis(50);
/// or per this many bytes.
const COALESCE_BYTES: usize = 4 * 1024;

/// `TOOL_LIMIT`: the model asked for a 21st tool call.
pub const TOOL_LIMIT: &str = "TOOL_LIMIT";

pub(super) fn chat<'a>(
    ws: &'a Workspace,
    core: &'a Core,
    params: ChatParams,
    origin: WriteOrigin,
) -> BoxStream<'a, AiEvent> {
    // Registered now, not on first poll, so a cancel that arrives first
    // still counts. No connection: a reconnect fails the next tool call
    // rather than the turn.
    let (token, _closed, guard) =
        match core.register_stream((Some(ws.id()), params.stream_id.clone()), None) {
            Ok(registered) => registered,
            Err(e) => {
                return Box::pin(futures::stream::once(std::future::ready(AiEvent::error(
                    e.code, e.message,
                ))))
            }
        };
    let (tx, mut rx) = mpsc::unbounded::<AiEvent>();
    let mut turn = Box::pin(run(ws, core, params, origin, token, tx));
    Box::pin(async_stream::stream! {
        let _guard = guard;
        loop {
            match select(rx.next(), turn.as_mut()).await {
                Either::Left((Some(event), _)) => yield event,
                Either::Left((None, _)) => break,
                Either::Right(((), _)) => {
                    while let Ok(event) = rx.try_recv() {
                        yield event;
                    }
                    break;
                }
            }
        }
    })
}

/// Sends a turn's events. Once the turn is cancelled nothing more goes
/// out: text it holds is dropped.
struct Out<'a> {
    tx: mpsc::UnboundedSender<AiEvent>,
    executor: &'a dyn Executor,
    stop: &'a Stop<'a>,
    pending: String,
    last: Option<Duration>,
}

impl Out<'_> {
    fn send(&mut self, event: AiEvent) {
        self.flush();
        let _ = self.tx.unbounded_send(event);
    }

    fn text(&mut self, delta: &str) {
        self.pending.push_str(delta);
        let now = self.executor.monotonic();
        let due = self
            .last
            .is_none_or(|last| now.saturating_sub(last) >= COALESCE);
        if due || self.pending.len() >= COALESCE_BYTES {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.stop.is_cancelled() {
            self.pending.clear();
        }
        if self.pending.is_empty() {
            return;
        }
        let delta = std::mem::take(&mut self.pending);
        self.last = Some(self.executor.monotonic());
        let _ = self.tx.unbounded_send(AiEvent::Text { delta });
    }

    /// How long until held text is due, if any is held.
    fn due_in(&self) -> Option<Duration> {
        if self.pending.is_empty() {
            return None;
        }
        let since = self.last.map_or(COALESCE, |last| {
            self.executor.monotonic().saturating_sub(last)
        });
        Some(COALESCE.saturating_sub(since))
    }
}

/// How a turn ended.
enum End {
    Done(AiStop),
    Failed(String, String),
    Cancelled,
}

impl From<Fail> for End {
    fn from(f: Fail) -> Self {
        End::Failed(f.code, f.message)
    }
}

/// What a round streamed.
struct RoundResult {
    text: String,
    calls: Vec<ToolCall>,
    stop: StopReason,
}

/// The reply as it builds up.
#[derive(Default)]
struct Reply {
    text: String,
    parts: Vec<Part>,
    dashboard_id: Option<String>,
}

impl Reply {
    fn has_tool(&self) -> bool {
        self.parts.iter().any(|p| matches!(p, Part::Tool { .. }))
    }

    fn row(&self, id: &str, chat_id: &str, now: &str) -> PersistedAIMessage {
        PersistedAIMessage {
            id: id.to_string(),
            chat_id: chat_id.to_string(),
            role: "assistant".into(),
            content: self.text.clone(),
            timestamp: now.to_string(),
            query: None,
            dashboard_id: self.dashboard_id.clone(),
            parts: self
                .has_tool()
                .then(|| serde_json::to_value(&self.parts).unwrap_or(Json::Null)),
        }
    }
}

/// One turn, start to end. Its events go out through `tx`.
async fn run(
    ws: &Workspace,
    core: &Core,
    p: ChatParams,
    origin: WriteOrigin,
    token: CancellationToken,
    tx: mpsc::UnboundedSender<AiEvent>,
) {
    // After `close_all` a turn says so rather than ending silently (or on
    // the closed connection).
    if ws.closing().is_cancelled() {
        let _ = tx.unbounded_send(AiEvent::error(
            crate::WORKSPACE_CLOSED,
            "This workspace was closed; reload to use the assistant again.",
        ));
        return;
    }
    if token.is_cancelled() {
        return;
    }
    let Some(executor) = core.executor.clone() else {
        let _ = tx.unbounded_send(Fail::not_supported().event());
        return;
    };
    // The stream id's cancel, and `close_all`'s.
    let stop = Stop::new(&token, Some(ws.closing()));
    let mut out = Out {
        tx,
        executor: &*executor,
        stop: &stop,
        pending: String::new(),
        last: None,
    };
    let _turn =
        match ws
            .ai
            .waiters
            .begin(&p.stream_id, &p.chat_id, core.ai_limits.max_turns_in_flight)
        {
            Ok(guard) => guard,
            Err(e) => {
                warn!(activity = "ai.chat", code = e.code.as_str(); "Refused a turn");
                out.send(AiEvent::error(e.code, e.message));
                return;
            }
        };
    let started = executor.monotonic();
    let resolved = match resolve(ws, core, &p).await {
        Ok(r) => r,
        Err(f) => {
            info!(activity = "ai.chat", chat_id = p.chat_id.as_str(), code = f.code.as_str(); "Refused a turn");
            out.send(f.event());
            return;
        }
    };
    if stop.is_cancelled() {
        return;
    }
    info!(activity = "ai.chat", chat_id = p.chat_id.as_str(), connection_id = p.connection_id.as_str(), provider = resolved.provider.kind.as_str(), model = resolved.provider.model.as_str(); "Turn started");

    // The user's message, before the first round.
    let user_row = PersistedAIMessage {
        id: p.user_message.id.clone(),
        chat_id: p.chat_id.clone(),
        role: "user".into(),
        content: p.user_message.content.clone(),
        timestamp: iso_timestamp(executor.unix_time()),
        query: None,
        dashboard_id: None,
        parts: None,
    };
    let user_seq = match chats::write(
        ws,
        core,
        &origin,
        &p.chat_id,
        &user_row,
        Write::User,
        &user_row.timestamp,
    )
    .await
    {
        Ok(seq) => seq,
        Err(e) => {
            warn!(activity = "ai.chat", code = e.code.as_str(); "Storing the user's message failed");
            out.send(AiEvent::error(e.code, e.message));
            return;
        }
    };
    out.send(AiEvent::Started {
        provider_kind: resolved.provider.kind.as_str().to_string(),
        model: resolved.provider.model.clone(),
    });

    let reply_cap = reply_cap(core);
    let mut t = Turn {
        ws,
        core,
        p: &p,
        r: &resolved,
        // Room for the note, unless the limit is too small for it to
        // leave much text (then the text is cut at the limit, no note).
        text_cap: if reply_cap >= 2 * REPLY_CUT_NOTE.len() {
            reply_cap - REPLY_CUT_NOTE.len()
        } else {
            reply_cap
        },
        http: core.ai_http.clone().expect("checked by resolve"),
        executor: executor.clone(),
        stop: &stop,
        reply: Reply::default(),
        calls: 0,
        rounds: 0,
        allow_all: p.approval == Approval::AllowAll,
    };
    let end = t.rounds(&mut out).await;
    // Held text goes out before the ending, unless the turn was cancelled
    // (then `flush` drops it).
    out.flush();
    // A reply cut for its size says so where it is stored (probe F2): the
    // page shows its own wording for the note, and later turns' history
    // tells the model.
    if matches!(end, End::Done(AiStop::TooLong))
        && t.reply.text.len() + REPLY_CUT_NOTE.len() <= reply_cap
    {
        t.reply.text.push_str(REPLY_CUT_NOTE);
    }

    // The reply, however the turn ended.
    let now = iso_timestamp(executor.unix_time());
    let reply_row = t.reply.row(&p.assistant_message_id, &p.chat_id, &now);
    let stored = store_reply(ws, core, &origin, &p.chat_id, reply_row, &now).await;
    let elapsed_ms = executor.monotonic().saturating_sub(started).as_millis() as u64;
    // Only stored rows go in an ending (review I4): the page has the
    // streamed text from the `text` events.
    let (messages, seq, stored_err) = match stored {
        Ok((row, seq)) => (vec![user_row, row], seq, None),
        Err(e) => {
            warn!(activity = "ai.chat", chat_id = p.chat_id.as_str(), code = e.code.as_str(); "Storing the reply failed");
            (vec![user_row], user_seq, Some(e))
        }
    };
    match (end, stored_err) {
        (End::Cancelled, _) => {
            info!(activity = "ai.chat", chat_id = p.chat_id.as_str(), outcome = "cancelled", rounds = t.rounds, tool_calls = t.calls, duration_ms = elapsed_ms; "Turn stopped");
        }
        (End::Done(stop), None) => {
            info!(activity = "ai.chat", chat_id = p.chat_id.as_str(), outcome = "done", stop = stop_name(stop), rounds = t.rounds, tool_calls = t.calls, duration_ms = elapsed_ms; "Turn done");
            out.send(AiEvent::Done {
                messages,
                seq,
                stop,
            });
        }
        (End::Done(_), Some(e)) => {
            out.send(failed(e.code, e.message, messages, seq));
        }
        (End::Failed(code, message), _) => {
            info!(activity = "ai.chat", chat_id = p.chat_id.as_str(), outcome = "error", code = code.as_str(), rounds = t.rounds, tool_calls = t.calls, duration_ms = elapsed_ms; "Turn failed");
            out.send(failed(code, message, messages, seq));
        }
    }
}

/// The most a reply's text may hold, its cut note included (probe F2): the
/// message limit a stored reply is checked against (`StateLimits`'
/// `max_message_bytes`, the web's 1 MiB) or Core's own ceiling
/// (`AiLimits::max_reply_bytes`, 1 MiB by default), whichever is lower.
fn reply_cap(core: &Core) -> usize {
    let stored = core.state_limits().max_message_bytes.unwrap_or(usize::MAX);
    core.ai_limits.max_reply_bytes.min(stored)
}

/// The longest prefix of `text` within `max` bytes that ends on a
/// character boundary.
fn prefix_within(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// The reply's write (review I4): refused for its size (`CHAT_FULL`, or
/// `INVALID_ARGUMENT` past `max_message_bytes`) with tool calls, it is
/// tried once more without them, keeping its text. Answers the row as
/// stored and its write's sequence.
async fn store_reply(
    ws: &Workspace,
    core: &Core,
    origin: &WriteOrigin,
    chat_id: &str,
    mut row: PersistedAIMessage,
    now: &str,
) -> Result<(PersistedAIMessage, ChangeSeq), crate::CoreError> {
    match chats::write(ws, core, origin, chat_id, &row, Write::Reply, now).await {
        Ok(seq) => Ok((row, seq)),
        Err(e)
            if row.parts.is_some()
                && (e.code == chats::CHAT_FULL || e.code == super::INVALID_ARGUMENT) =>
        {
            debug!(activity = "ai.chat", code = e.code.as_str(); "Storing the reply again without its tool calls");
            row.parts = None;
            let seq = chats::write(ws, core, origin, chat_id, &row, Write::Reply, now).await?;
            Ok((row, seq))
        }
        Err(e) => Err(e),
    }
}

fn stop_name(stop: AiStop) -> &'static str {
    match stop {
        AiStop::End => "end",
        AiStop::MaxTokens => "maxTokens",
        AiStop::TooLong => "tooLong",
    }
}

fn failed(
    code: String,
    message: String,
    messages: Vec<PersistedAIMessage>,
    seq: ChangeSeq,
) -> AiEvent {
    AiEvent::Error {
        code,
        message,
        messages: Some(messages),
        seq: Some(seq),
    }
}

/// Core's refusals for a turn, in order, before anything is stored.
async fn resolve(ws: &Workspace, core: &Core, p: &ChatParams) -> Result<Resolved, Fail> {
    if let Some(max) = core.ai_limits.max_message_bytes {
        if p.user_message.content.len() > max {
            return Err(Fail::new(
                super::MESSAGE_TOO_LONG,
                format!(
                    "The message is longer than allowed here (max_message_bytes: {max} bytes)."
                ),
            ));
        }
    }
    for id in [&p.user_message.id, &p.assistant_message_id] {
        if !seaquel_workspace::state::is_client_id(id) {
            return Err(Fail::new(
                super::INVALID_ARGUMENT,
                "A message id is 1 to 64 letters, digits, '-' or '_'.",
            ));
        }
    }
    if p.user_message.id == p.assistant_message_id {
        return Err(Fail::new(
            super::INVALID_ARGUMENT,
            "The message and the reply need ids of their own.",
        ));
    }
    let chat = chats::chat(ws, &p.chat_id).await?;
    let mut r = super::resolve(
        ws,
        core,
        &chat.connection_id,
        p.api_key.clone(),
        p.provider_id.as_deref(),
    )
    .await?;
    chats::check_room(ws, core, &p.chat_id, p.user_message.content.len() as u64).await?;
    let connection = core
        .connection_as(&p.connection_id, Some(ws.id()))
        .map_err(|e| Fail::new(e.code, e.message))?;
    if connection.saved_connection_id.as_deref() != Some(chat.connection_id.as_str()) {
        return Err(Fail::new(
            super::CONNECTION_MISMATCH,
            "This connection wasn't opened for the chat's connection.",
        ));
    }
    if let Some(engine) = connection.sql_engine {
        r.engine = engine;
    }
    Ok(r)
}

/// The state of a turn between its two writes.
struct Turn<'a> {
    ws: &'a Workspace,
    core: &'a Core,
    p: &'a ChatParams,
    r: &'a Resolved,
    http: Arc<dyn HttpClient>,
    executor: Arc<dyn Executor>,
    stop: &'a Stop<'a>,
    reply: Reply,
    /// The most text the reply may stream; past it the turn stops reading
    /// the provider and ends `tooLong` (probe F2).
    text_cap: usize,
    /// Tool calls so far, across rounds.
    calls: usize,
    rounds: u32,
    allow_all: bool,
}

impl Turn<'_> {
    async fn rounds(&mut self, out: &mut Out<'_>) -> End {
        let r = self.r;
        // What every round starts from.
        let tables = if r.sharing.schema {
            self.schema().await
        } else {
            Vec::new()
        };
        let context = prompt::schema_context(&tables, SCHEMA_CONTEXT_BYTES);
        let (queries, boards) = self.mention_sources().await;
        let user_text = prompt::mentions(
            &self.p.user_message.content,
            r.sharing.schema,
            &tables,
            &queries,
            &boards,
        );
        let rows = match chats::history_rows(
            self.ws,
            &self.p.chat_id,
            &[&self.p.user_message.id, &self.p.assistant_message_id],
        )
        .await
        {
            Ok(rows) => rows,
            Err(e) => return End::Failed(e.code, e.message),
        };
        let past = history(&rows, HISTORY_BYTES);
        let dashboard_id = last_dashboard_id(&rows).map(str::to_string);
        let system = prompt::system(
            r.engine,
            Some(&context),
            r.sharing,
            true,
            self.p.client_tools,
        );
        let specs: Vec<ToolSpec> =
            tools::definitions(Profile::Assistant, r.sharing, self.p.client_tools)
                .iter()
                .map(|d| d.spec())
                .collect();
        debug!(activity = "ai.chat", history_rows = past.kept.len(), history_bytes = past.bytes, tables = context.kept, tables_left = context.left, tools = specs.len(); "Turn context");

        let mut this_turn: Vec<Message> = Vec::new();
        loop {
            if self.stop.is_cancelled() {
                return End::Cancelled;
            }
            let mut messages = past.messages.clone();
            push_user(&mut messages, user_text.clone());
            messages.extend(this_turn.iter().cloned());
            if let Some(id) = &dashboard_id {
                with_dashboard_context(&mut messages, id);
            }
            let round = Round {
                system: system.clone(),
                messages,
                tools: specs.clone(),
            };
            let index = self.rounds;
            self.rounds += 1;
            let result = self.stream_round(&round, index, out).await;
            let streamed = match result {
                Ok(streamed) => streamed,
                Err(end) => return end,
            };
            if streamed.calls.is_empty() {
                return End::Done(match streamed.stop {
                    StopReason::MaxTokens => AiStop::MaxTokens,
                    _ => AiStop::End,
                });
            }
            let mut results = Vec::new();
            for call in &streamed.calls {
                let output = match self.tool_call(call, out).await {
                    Ok(output) => output,
                    Err(end) => return end,
                };
                self.reply.parts.push(Part::tool(
                    index,
                    &call.id,
                    &call.name,
                    call.input.clone(),
                    &output,
                ));
                results.push(ToolResult {
                    call_id: call.id.clone(),
                    content: output.text,
                    is_error: output.is_error,
                });
            }
            this_turn.push(Message::Assistant {
                text: streamed.text,
                tool_calls: streamed.calls,
            });
            this_turn.push(Message::ToolResults(results));
        }
    }

    /// The schema for the context and mentions; nothing when it can't be
    /// read (the turn goes on without it).
    async fn schema(&self) -> Vec<seaquel_types::SchemaTable> {
        let tables = match self.ws.engine(self.core, &self.p.connection_id) {
            Ok(h) => h.schema_tables().await,
            Err(e) => Err(e),
        };
        tables.unwrap_or_else(|e| {
            debug!(activity = "ai.chat", code = e.code.as_str(); "Reading the schema failed");
            Vec::new()
        })
    }

    /// The project's saved queries and dashboards, for `@mentions`.
    async fn mention_sources(&self) -> (Vec<MentionQuery>, Vec<MentionDashboard>) {
        let project = self.r.row.project_id.as_str();
        let queries = saved_queries::list(self.ws.storage(), project)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|q| MentionQuery {
                name: q.name,
                query: q.query,
            })
            .collect();
        let boards = dashboards::list(self.ws.storage(), project)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|d| MentionDashboard {
                widgets: widgets_of(&d.widgets),
                id: d.id,
                name: d.name,
            })
            .collect();
        (queries, boards)
    }

    /// Streams one round: its text out as it comes, its calls collected
    /// and counted. `Err` ends the turn.
    async fn stream_round(
        &mut self,
        round: &Round,
        index: u32,
        out: &mut Out<'_>,
    ) -> Result<RoundResult, End> {
        let r = self.r;
        let req = wire::round_request(
            &r.provider,
            r.key.as_ref().map(|k| k.expose()),
            round,
            super::FROM_BROWSER,
        );
        let begun = self.executor.monotonic();
        let deadline = begun + super::ROUND_TIMEOUT;
        let response =
            match super::send(&*self.http, &*self.executor, req, self.stop, deadline).await {
                Ok(Some(response)) => response,
                Ok(None) => return Err(End::Cancelled),
                Err(f) => return Err(f.into()),
            };
        debug!(activity = "ai.round", round = index, status = response.status; "Round answered");
        let HttpResponse { status, mut body } = response;
        if !(200..300).contains(&status) {
            let bytes = match super::read_all(
                &mut body,
                &*self.executor,
                self.stop,
                deadline,
                wire::MAX_ERROR_BODY_BYTES,
            )
            .await
            {
                Ok(Some(bytes)) => bytes,
                Ok(None) => return Err(End::Cancelled),
                Err(f) => return Err(f.into()),
            };
            let secret = r.key.as_ref().map(|k| k.expose());
            let e = wire::check_status_with(status, &bytes, secret).expect_err("not a 2xx");
            return Err(super::wire_fail(&e).into());
        }

        let mut decoder = Decoder::with_secret(r.provider.kind, r.key.as_ref().map(|k| k.expose()));
        let mut result = RoundResult {
            text: String::new(),
            calls: Vec::new(),
            stop: StopReason::End,
        };
        let mut events = Vec::new();
        let mut idle_from = self.executor.monotonic();
        loop {
            let now = self.executor.monotonic();
            let idle_left = super::IDLE_TIMEOUT.saturating_sub(now.saturating_sub(idle_from));
            let round_left = deadline.saturating_sub(now);
            let wait = [Some(idle_left), Some(round_left), out.due_in()]
                .into_iter()
                .flatten()
                .min()
                .unwrap_or(idle_left);
            let next = super::race(body.next(), &*self.executor, wait, self.stop).await;
            let chunk = match next {
                super::Next::Cancelled => {
                    self.keep_round_text(index, &result.text);
                    return Err(End::Cancelled);
                }
                super::Next::Timer => {
                    let now = self.executor.monotonic();
                    if now >= deadline || now.saturating_sub(idle_from) >= super::IDLE_TIMEOUT {
                        self.keep_round_text(index, &result.text);
                        return Err(Fail::timeout().into());
                    }
                    out.flush();
                    continue;
                }
                super::Next::Item(item) => item,
            };
            idle_from = self.executor.monotonic();
            let (fed, ended) = match chunk {
                Some(Ok(bytes)) => (decoder.feed(&bytes, &mut events), false),
                Some(Err(e)) => {
                    self.keep_round_text(index, &result.text);
                    return Err(super::http_fail(&e).into());
                }
                None => (decoder.finish(&mut events), true),
            };
            for event in events.drain(..) {
                match event {
                    RoundEvent::Text(t) => {
                        let room = self.text_cap.saturating_sub(self.reply.text.len());
                        let kept = prefix_within(&t, room);
                        out.text(kept);
                        result.text.push_str(kept);
                        self.reply.text.push_str(kept);
                        if kept.len() < t.len() {
                            // Dropping the body here stops the provider's
                            // stream; calls the round streamed don't run.
                            self.keep_round_text(index, &result.text);
                            return Err(End::Done(AiStop::TooLong));
                        }
                    }
                    RoundEvent::ToolCall(call) => {
                        self.calls += 1;
                        if self.calls > MAX_TOOL_CALLS_PER_TURN {
                            self.keep_round_text(index, &result.text);
                            return Err(End::Failed(
                                TOOL_LIMIT.into(),
                                format!(
                                    "The model asked for more than {MAX_TOOL_CALLS_PER_TURN} tool calls in this turn."
                                ),
                            ));
                        }
                        result.calls.push(call);
                    }
                    RoundEvent::Usage(u) => {
                        info!(activity = "ai.round", round = index, input_tokens = u.input_tokens, output_tokens = u.output_tokens; "Token usage");
                    }
                    RoundEvent::Stop(s) => result.stop = s,
                }
            }
            if let Err(e) = fed {
                self.keep_round_text(index, &result.text);
                return Err(super::wire_fail(&e).into());
            }
            if ended {
                break;
            }
        }
        self.keep_round_text(index, &result.text);
        Ok(result)
    }

    /// The round's text as a part of the reply (Decision 23), before its
    /// calls.
    fn keep_round_text(&mut self, round: u32, text: &str) {
        if !text.is_empty() {
            self.reply.parts.push(Part::Text {
                round,
                text: text.to_string(),
            });
        }
    }

    /// One call, start to result. `Err`: the turn ends (cancelled while it
    /// waited or ran, or its wait was lost).
    async fn tool_call(&mut self, call: &ToolCall, out: &mut Out<'_>) -> Result<ToolOutput, End> {
        let sql = match call.name.as_str() {
            "run_query" | "explain_query" => call
                .input
                .get("sql")
                .and_then(Json::as_str)
                .map(String::from),
            _ => None,
        };
        out.send(AiEvent::ToolCall {
            call_id: call.id.clone(),
            name: call.name.clone(),
            sql,
        });
        let (output, code) = match self.tool(call, out).await? {
            Ok(output) => (output, None),
            Err(e) => (ToolOutput::error(&e), Some(e.code)),
        };
        let (rows, truncated) = if output.is_error {
            (None, None)
        } else {
            let v: Json = serde_json::from_str(&output.text).unwrap_or(Json::Null);
            (
                v["rowCount"].as_u64(),
                v["truncated"]
                    .as_bool()
                    .filter(|_| v.get("rowCount").is_some()),
            )
        };
        // The registry's name only: an unknown one is the model's text.
        let tool = Tool::find(Profile::Assistant, &call.name).map_or("unknown", Tool::name);
        debug!(activity = "ai.tool", tool = tool, ok = !output.is_error, code = code.as_deref(); "Tool call");
        out.send(AiEvent::ToolDone {
            call_id: call.id.clone(),
            ok: !output.is_error,
            rows,
            truncated,
            code,
        });
        Ok(output)
    }

    /// The call's checks, approval or the page's answer, and its run.
    /// `Err`: the turn ends. A client tool's answer is `Ok` even when the
    /// page marked it an error (its output says so).
    async fn tool(
        &mut self,
        call: &ToolCall,
        out: &mut Out<'_>,
    ) -> Result<Result<ToolOutput, ToolError>, End> {
        // Sharing as it is now (Decision 22), and the names answers use.
        let (sharing, name) = super::sharing_now(self.ws, &self.r.row.id).await;
        let gate = Gate {
            sharing,
            client_tools: self.p.client_tools,
            connection_name: &name,
        };
        let parsed: Call = match tools::prepare(&call.name, &call.input, &gate) {
            Ok(parsed) => parsed,
            Err(e) => return Ok(Err(e)),
        };
        let engine = match tools::engine_of(self.core, self.ws, &self.p.connection_id) {
            Ok(engine) => engine,
            Err(_) => self.r.engine,
        };
        if let Err(e) = tools::read_only_check(&parsed, engine) {
            return Ok(Err(e));
        }
        if parsed.tool.is_client() {
            let Args::Client { input, .. } = &parsed.args else {
                return Ok(Err(ToolError::unknown_tool(&call.name)));
            };
            let rx = self
                .ws
                .ai
                .waiters
                .wait(&self.p.stream_id, &call.id, Kind::Client);
            out.send(AiEvent::ClientTool {
                call_id: call.id.clone(),
                name: call.name.clone(),
                input: input.clone(),
            });
            let AiDecision::Client(answer) = self.answer(rx).await? else {
                return Err(lost_wait());
            };
            let mut output =
                tools::render::client_result(parsed.tool, &answer.result, sharing.schema);
            output.is_error |= answer.is_error;
            if parsed.tool == Tool::CreateDashboard && !output.is_error {
                if let Some(id) = serde_json::from_str::<Json>(&answer.result)
                    .ok()
                    .and_then(|v| v["dashboard_id"].as_str().map(String::from))
                {
                    self.reply.dashboard_id = Some(id);
                }
            }
            return Ok(Ok(output));
        }
        let ctx = ToolContext {
            profile: Profile::Assistant,
            connection_id: &self.p.connection_id,
            connection_name: &name,
            project_id: &self.r.row.project_id,
            project_name: &self.r.project_name,
            timeout: tools_timeout(),
        };
        let planned: Planned = match tools::plan(self.core, self.ws, &ctx, &parsed).await {
            Ok(planned) => planned,
            Err(e) => return Ok(Err(e)),
        };
        if parsed.tool.asks_approval() && !self.allow_all {
            let rx = self
                .ws
                .ai
                .waiters
                .wait(&self.p.stream_id, &call.id, Kind::Approval);
            out.send(AiEvent::ApprovalRequired {
                call_id: call.id.clone(),
                sql: planned.sql().unwrap_or_default().to_string(),
            });
            match self.answer(rx).await? {
                AiDecision::Approval(ApprovalDecision::Allow) => {}
                AiDecision::Approval(ApprovalDecision::AllowAll) => self.allow_all = true,
                AiDecision::Approval(ApprovalDecision::Deny) => {
                    return Ok(Err(ToolError::denied()))
                }
                AiDecision::Client(_) => return Err(lost_wait()),
            }
        }
        if self.stop.is_cancelled() {
            return Err(End::Cancelled);
        }
        let run = Box::pin(tools::run(self.core, self.ws, &ctx, &parsed, planned));
        match super::race(run, &*self.executor, tools_timeout(), self.stop).await {
            super::Next::Item(result) => Ok(result.map(|v| ToolOutput::ok(&v))),
            super::Next::Timer => Ok(Err(tools::timed_out(tools_timeout()))),
            super::Next::Cancelled => Err(End::Cancelled),
        }
    }

    /// The answer to a waiting call. The turn's cancel ends it; a waiter
    /// lost without an answer (review M3) fails the turn rather than ending
    /// it silently.
    async fn answer(
        &self,
        rx: futures::channel::oneshot::Receiver<AiDecision>,
    ) -> Result<AiDecision, End> {
        if self.stop.is_cancelled() {
            return Err(End::Cancelled);
        }
        let cancelled = Box::pin(self.stop.cancelled());
        match select(rx, cancelled).await {
            Either::Left((Ok(decision), _)) => Ok(decision),
            Either::Left((Err(_), _)) => Err(lost_wait()),
            Either::Right(_) => Err(End::Cancelled),
        }
    }
}

/// A wait whose answer can't come any more.
fn lost_wait() -> End {
    End::Failed(
        "INTERNAL".into(),
        "The turn stopped waiting for an answer.".into(),
    )
}

fn tools_timeout() -> Duration {
    seaquel_ai::limits::CALL_TIMEOUT
}

/// A dashboard's widgets as mentions read them: each one that has an id,
/// a title and a type; a body that doesn't parse lists none.
fn widgets_of(raw: &str) -> Vec<MentionWidget> {
    serde_json::from_str::<Vec<Json>>(raw)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|w| serde_json::from_value(w).ok())
        .collect()
}
