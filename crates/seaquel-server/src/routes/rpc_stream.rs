//! `GET /rpc/stream`: one WebSocket per browser session, for the user in
//! `X-Seaquel-User`. It carries that user's query streams, several at once,
//! and their connection events, the editor's runs (`db.run`, `db.page`),
//! the data tab's pages (`db.tablePage`) and the assistant's turns
//! (`ai.chat`, phase 6), each of which counts as one stream.
//!
//! Only Node's `/api/rpc/stream` upgrade reaches it. Node sets the header
//! from the session and drops any copy the browser sent, and passes frames
//! through untouched; ownership is checked here, by Core. Node also sets
//! `X-Seaquel-Origin` from the page's `?origin=` when it is well-formed;
//! it's checked again here, and a run's history event carries it.
//!
//! # Client frames (Text, JSON)
//!
//! ```json
//! {"op":"start","streamId":"s1","request":{"method":"db","params":{"method":"queryStream","params":{"connectionId":"c","streamId":"s1","sql":"SELECT 1"}}}}
//! {"op":"cancel","streamId":"s1"}
//! ```
//!
//! - `start` runs `request`, a `CoreRequest` that must be `db.queryStream`,
//!   `db.run`, `db.page`, `db.tablePage` or `ai.chat` with the same
//!   `streamId` (`Request::stream_kind`), on the user's workspace
//!   (`dispatch_stream`).
//!   `request` is an inline JSON object. It goes to `parse_request` as the
//!   exact bytes it has in the frame (a borrowed `RawValue`, never a
//!   `serde_json::Value`), so the `method`-before-`params` rule holds.
//! - `cancel` cancels a stream, run or turn this socket started
//!   (`Workspace::cancel`). Nothing more arrives for it. An id this socket
//!   isn't running is ignored. A turn's approvals and client tools are
//!   answered over `/rpc` (`ai.respond`), not here.
//!
//! # Server frames (Text, `CoreEvent` JSON)
//!
//! - `{"type":"stream","streamId":…,"event":…}`: a stream's batches, then
//!   one `done` or `error`. A stream that ends without either (another
//!   request cancelled it, or the workspace was evicted) gets a final
//!   `error` with code [`CANCELLED`]; one this socket cancelled gets nothing.
//! - `{"type":"run","streamId":…,"event":…}`: a run's, page's or table
//!   page's events (per statement a `statementStart`, `batch`es and
//!   `statementDone` or `statementError`), then one `done` or `error`. The
//!   `CANCELLED` rule is the same, as a run `error`.
//! - `{"type":"ai","streamId":…,"event":…}` (phase 6): a turn's `started`,
//!   `text`, tool calls, `approvalRequired` and `clientTool`, then one
//!   `done` or `error`. The `CANCELLED` rule is the same, as an `ai`
//!   `error`.
//! - `{"type":"connectionClosed",…}`: one of the user's connections went
//!   away, from any of the user's workspaces: `WORKSPACE_EVICTED`,
//!   `WINDOW_CLOSED` (a tab whose sockets stayed closed past
//!   [`crate::workspaces::WINDOW_GRACE`]; its connections were closed) or
//!   `CONNECTION_REPLACED` (a tab connected the same saved connection
//!   again). A page ignores one for a connection it doesn't hold.
//!
//! # Windows
//!
//! The socket's origin (`X-Seaquel-Origin`, the tab's window id) is the
//! window its connections belong to. While any socket of a window is open
//! the window is kept; when its last one closes and none returns within
//! `WINDOW_GRACE` (2 minutes; a reload comes back well within it), the
//! window's connections are closed (`Workspace::close_owned_by`). A socket
//! without an origin keeps nothing.
//! - `{"type":"storageChanged","kind",…,"seq"}` (phase 5d): a write to the
//!   user's metadata committed, from any of their tabs (or Core itself).
//!   Every socket of that user gets it, the writer's too, which skips it by
//!   its `origin`; no other user's socket does. It names kinds and ids,
//!   never a value. Events sent while a socket was closed are lost: a tab
//!   reloads what it shows after reconnecting.
//!
//! A frame that can't be served gets an `error` event, never a closed
//! socket: `INVALID_ARGUMENT` (not JSON, binary, a bad `op`, a missing or
//! mismatched `streamId`, one longer than [`MAX_STREAM_ID_LEN`] or outside
//! `[A-Za-z0-9_.:-]`, a bad `request`, a stream id already running on this
//! socket) or [`TOO_MANY_STREAMS`]. It is a `run` event when the frame's
//! request names `db.run`, `db.page` or `db.tablePage`, an `ai` event when
//! it names `ai.chat`, else a `stream` event. Its
//! `streamId` is the frame's when it has a valid string one, else `""`.
//!
//! # Limits
//!
//! - At most [`MAX_STREAMS`] streams run per socket, a run, page or table
//!   page being one however many statements it has and a turn one however
//!   many rounds and tool calls it makes, and a user has at most
//!   [`MAX_LISTENERS_PER_USER`] sockets; one more is closed at once with
//!   code 1013 and a [`TOO_MANY_SOCKETS`] reason.
//! - A batch (a stream's or a run's) whose frame would pass
//!   [`MAX_BATCH_FRAME_BYTES`] is split by rows into several `batch`
//!   events, in order.
//! - A frame (and a message) is at most [`MAX_FRAME_BYTES`]; a larger one
//!   closes the socket (WebSocket close code 1009).
//! - Workspace events wait in a bounded queue per socket
//!   (`LISTENER_EVENT_BOUND`, 1,024 events, and `LISTENER_EVENT_BYTE_BOUND`,
//!   8 MiB). A client that falls that far behind is closed with 1013 and an [`EVENTS_LAGGED`] reason; it reconnects and
//!   reloads what it shows. The close cancels the socket's running queries,
//!   as any close does. While events are backed up the socket still reads
//!   client frames, so a `cancel` gets through.
//! - At most [`MAX_PENDING_REFUSALS`] refusals of client frames wait to be
//!   sent; a client that sends refused frames without reading passes it
//!   and is closed with 1008 and a [`TOO_MANY_PENDING`] reason.
//! - Closing the socket, or losing it, cancels every stream it started.
//!   **A turn is cancelled, never aborted** (phase 6, Task 4's contract):
//!   its task keeps polling it, so it stores the reply with what streamed,
//!   and is aborted only if it still runs after
//!   [`seaquel_rpc::TURN_STOP_WAIT`] (45 s). An `ai.respond` for it then
//!   answers `NOT_FOUND`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use axum::{
    extract::{
        ws::{close_code, CloseFrame, Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::HeaderMap,
    response::Response,
};
use futures::{SinkExt, StreamExt};
use seaquel_core::domain::run::RunEvent;
use seaquel_core::StreamEvent;
use seaquel_rpc::{
    dispatch_stream, parse_request, CoreEvent, Request, RpcError, StreamKind, WriteOrigin,
    INVALID_ARGUMENT,
};
use seaquel_types::StreamBatch;
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use super::rpc::{error_response, open_workspace, redact, user_id, write_origin};
use crate::workspaces::{
    ListenError, Listener, OpenWorkspace, EVENTS_LAGGED, MAX_LISTENERS_PER_USER,
};
use crate::AppState;

/// The most streams one socket runs at once.
pub const MAX_STREAMS: usize = 16;

/// The largest frame or message the socket takes. Node's proxy uses the
/// same limit (`shared/rpc-stream-proxy.js`).
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// A batch frame larger than this is split by rows (see `encode_split`),
/// so one wide result never makes a huge frame.
pub const MAX_BATCH_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// A socket beyond [`MAX_LISTENERS_PER_USER`] is accepted and closed at
/// once with WebSocket close code 1013 ("try again later") and a reason
/// starting with this code.
pub const TOO_MANY_SOCKETS: &str = "TOO_MANY_SOCKETS";

/// A `start` beyond [`MAX_STREAMS`].
pub const TOO_MANY_STREAMS: &str = "TOO_MANY_STREAMS";

/// A stream that ended without `done` or `error`, and not because this
/// socket cancelled it.
pub const CANCELLED: &str = "CANCELLED";

/// Frames queued for the client before senders wait. Streams wait for a
/// slow client instead of piling rows up in memory.
const OUTBOX: usize = 64;

/// How many refusals of client frames may wait for room in the outbox at
/// once, per socket. A client that sends refused frames without reading
/// the answers passes it and is closed with 1008 and [`TOO_MANY_PENDING`].
pub const MAX_PENDING_REFUSALS: usize = 64;

/// The close reason of a socket past [`MAX_PENDING_REFUSALS`].
pub const TOO_MANY_PENDING: &str = "TOO_MANY_PENDING";

/// How long a socket closing itself gives its writer to deliver the close
/// frame.
const LAGGED_CLOSE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let user = match user_id(&headers) {
        Ok(user) => user.to_string(),
        Err(e) => return error_response(e),
    };
    // The page's write origin (Node took it from `?origin=` and checked it;
    // checked again here): a run's history event carries it.
    let origin = write_origin(&headers);
    // Subscribed before the upgrade, so an eviction from here on reaches
    // this socket.
    let listener = match state.workspaces.listen(&user) {
        Ok(listener) => listener,
        Err(ListenError::InvalidUser(e)) => {
            return error_response(RpcError::invalid_argument(e.message()))
        }
        // Accepted and closed with a reason, so the client can tell it
        // apart from a lost connection (Node pipes the close through).
        Err(ListenError::TooMany) => {
            return ws.on_upgrade(|mut socket| async move {
                let _ = socket
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::AGAIN,
                        reason: format!(
                            "{TOO_MANY_SOCKETS}: at most {MAX_LISTENERS_PER_USER} open at once"
                        )
                        .into(),
                    })))
                    .await;
            })
        }
    };
    // While this socket is open its window counts as open; its last socket
    // closing starts the window's grace period (phase 6 probe F4).
    let window = origin
        .as_deref()
        .map(|o| state.workspaces.hold_window(&state.core, &user, o));
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            Session::new(state, user, origin)
                .run(socket, listener)
                .await;
            drop(window);
        })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClientFrame<'a> {
    op: String,
    #[serde(default)]
    stream_id: Option<String>,
    #[serde(borrow, default)]
    request: Option<&'a RawValue>,
}

/// A stream this socket started.
struct Running {
    open: Arc<OpenWorkspace>,
    /// Set when this socket cancels it: then it ends with nothing more.
    cancelled: Arc<AtomicBool>,
    task: JoinHandle<()>,
    /// Tells this run's end apart from a later one with the same id.
    generation: u64,
    /// A turn (`ai.chat`) is never aborted: see [`stop`].
    kind: StreamKind,
}

struct Session {
    state: AppState,
    user: String,
    origin: WriteOrigin,
    running: HashMap<String, Running>,
    next_generation: u64,
}

/// A stream `error` event, for a frame that isn't a run's.
fn error_event(stream_id: &str, code: &str, message: impl Into<String>) -> CoreEvent {
    CoreEvent::error(stream_id, StreamKind::Stream, code, message)
}

/// Which stream kind a start frame's `request` names, read leniently (it
/// may not parse as a request at all), so that even its refusal is an
/// event of that kind: a `run` event for `db.run`, `db.page` and
/// `db.tablePage`, an `ai` event for `ai.chat`, else a `stream` event.
fn named_kind(request: &RawValue) -> StreamKind {
    #[derive(Deserialize)]
    struct Outer {
        method: Option<String>,
        params: Option<Inner>,
    }
    #[derive(Deserialize)]
    struct Inner {
        method: Option<String>,
    }
    serde_json::from_str::<Outer>(request.get())
        .ok()
        .and_then(|o| {
            let group = o.method?;
            let method = o.params?.method?;
            StreamKind::of(&group, &method)
        })
        .unwrap_or(StreamKind::Stream)
}

/// `event` as JSON text. A batch carries driver data, so this can fail;
/// then it's the stream's error instead.
fn encode(event: &CoreEvent) -> Result<String, String> {
    serde_json::to_string(event).map_err(|e| {
        log::warn!("failed to serialize a stream event: {e}");
        format!("failed to serialize result: {e}")
    })
}

impl Session {
    fn new(state: AppState, user: String, origin: WriteOrigin) -> Self {
        Self {
            state,
            user,
            origin,
            running: HashMap::new(),
            next_generation: 0,
        }
    }

    async fn run(mut self, socket: WebSocket, mut listener: Listener) {
        let (mut sink, mut source) = socket.split();
        let (outbox, mut outgoing) = mpsc::channel::<String>(OUTBOX);
        // A close frame to send instead of what's still queued (a lagging
        // listener's `EVENTS_LAGGED`).
        let (close_tx, mut close_rx) = oneshot::channel::<CloseFrame>();
        let writer = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    close = &mut close_rx => {
                        if let Ok(frame) = close {
                            let _ = sink.send(Message::Close(Some(frame))).await;
                        }
                        break;
                    }
                    text = outgoing.recv() => match text {
                        Some(text) => {
                            if sink.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    },
                }
            }
        });
        let mut close_tx = Some(close_tx);
        // Each stream task reports its end, so its slot frees up.
        let (ended_tx, mut ended) = mpsc::unbounded_channel::<(String, u64)>();
        // One workspace event taken from the listener and waiting for room
        // in the outbox. While it waits, the loop still reads client frames
        // (a `cancel` must get through while events are backed up), and the
        // listener's own bounded channel fills; past its bound the hub
        // drops it and the socket closes as lagging.
        let mut pending: Option<String> = None;
        // Why this socket closes itself, if it does: a lagging listener, or
        // a client that sends refused frames without reading the refusals.
        let mut close: Option<CloseFrame> = None;
        let mut lag = listener.lag_signal();
        // Refusals spawned by `send_later` and not in the outbox yet.
        let refusals = Arc::new(AtomicUsize::new(0));

        loop {
            tokio::select! {
                frame = source.next() => match frame {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(event) = self.frame(text.as_str(), &outbox, &ended_tx).await {
                            if !send_later(&outbox, &refusals, event) {
                                close = Some(too_many_pending());
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        let event = error_event("", INVALID_ARGUMENT, "frames must be JSON text");
                        if !send_later(&outbox, &refusals, event) {
                            close = Some(too_many_pending());
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    // Closed, lost, or a frame over the limit.
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                },
                permit = outbox.reserve(), if pending.is_some() => match permit {
                    Ok(permit) => permit.send(pending.take().unwrap_or_default()),
                    Err(_) => break,
                },
                event = listener.recv(), if pending.is_none() => match event {
                    Some(event) => pending = encode(&event).ok(),
                    None => {
                        close = Some(events_lagged());
                        break;
                    }
                },
                () = lag.fired() => {
                    close = Some(events_lagged());
                    break;
                }
                Some((stream_id, generation)) = ended.recv() => {
                    if self.running.get(&stream_id).is_some_and(|r| r.generation == generation) {
                        self.running.remove(&stream_id);
                    }
                }
            }
        }

        // Cancel everything this socket started: through Core, and by
        // dropping each stream, except a turn, which ends on its own (see
        // `stop`). That includes a socket closing itself (lagging or too
        // many refusals waiting): its running queries and turns stop with it.
        for (stream_id, running) in self.running.drain() {
            stop(&self.state.core, &stream_id, running);
        }
        drop(outbox);
        if let Some(frame) = close {
            // Tell the client why (after `EVENTS_LAGGED` it reconnects and
            // reloads what it shows). A client too slow to take even the
            // close frame is cut off.
            log::warn!(activity = "rpc.stream", close_code = frame.code; "Closing a socket");
            if let Some(close_tx) = close_tx.take() {
                let _ = close_tx.send(frame);
            }
            let mut writer = writer;
            if tokio::time::timeout(LAGGED_CLOSE_WAIT, &mut writer)
                .await
                .is_err()
            {
                writer.abort();
                log::warn!(activity = "rpc.stream"; "The socket didn't take its close frame");
            }
        } else {
            writer.abort();
        }
    }

    /// Serve one text frame. Returns the event to send for a frame that
    /// can't be served.
    async fn frame(
        &mut self,
        text: &str,
        outbox: &mpsc::Sender<String>,
        ended: &mpsc::UnboundedSender<(String, u64)>,
    ) -> Option<CoreEvent> {
        let frame: ClientFrame<'_> = match serde_json::from_str(text) {
            Ok(frame) => frame,
            Err(e) => {
                return Some(error_event(
                    &lenient_stream_id(text),
                    INVALID_ARGUMENT,
                    format!("invalid frame: {e}"),
                ))
            }
        };
        let stream_id = match frame.stream_id {
            Some(id) if !id.is_empty() => id,
            _ => {
                return Some(error_event(
                    "",
                    INVALID_ARGUMENT,
                    "the frame needs a non-empty streamId",
                ))
            }
        };
        if !valid_stream_id(&stream_id) {
            // Not echoed: it may be anything the browser sent.
            return Some(error_event(
                "",
                INVALID_ARGUMENT,
                format!(
                    "a streamId is at most {MAX_STREAM_ID_LEN} characters of A-Z, a-z, 0-9, \
                     '_', '.', ':' and '-'"
                ),
            ));
        }
        match frame.op.as_str() {
            "start" => {
                let Some(request) = frame.request else {
                    return Some(error_event(
                        &stream_id,
                        INVALID_ARGUMENT,
                        "a start frame needs a request",
                    ));
                };
                let kind = named_kind(request);
                self.start(stream_id, request, outbox, ended)
                    .await
                    .err()
                    .map(|(id, e)| CoreEvent::error(&id, kind, &e.code, e.message))
            }
            "cancel" => {
                if let Some(running) = self.running.get(&stream_id) {
                    running.cancelled.store(true, Ordering::SeqCst);
                    running
                        .open
                        .workspace()
                        .cancel(&self.state.core, &stream_id);
                }
                None
            }
            other => Some(error_event(
                &stream_id,
                INVALID_ARGUMENT,
                format!("unknown op {other:?}; expected \"start\" or \"cancel\""),
            )),
        }
    }

    async fn start(
        &mut self,
        stream_id: String,
        request: &RawValue,
        outbox: &mpsc::Sender<String>,
        ended: &mpsc::UnboundedSender<(String, u64)>,
    ) -> Result<(), (String, RpcError)> {
        let fail = |e: RpcError| (stream_id.clone(), e);
        if self.running.contains_key(&stream_id) {
            return Err(fail(RpcError::invalid_argument(format!(
                "stream {stream_id:?} is already running on this socket"
            ))));
        }
        if self.running.len() >= MAX_STREAMS {
            return Err(fail(RpcError::new(
                TOO_MANY_STREAMS,
                format!("at most {MAX_STREAMS} streams run at once on one socket"),
            )));
        }
        let request = parse_request(request.get().as_bytes()).map_err(fail)?;
        let Some(kind) = request.stream_kind() else {
            return Err(fail(not_a_stream(&request)));
        };
        if request.stream_id() != Some(stream_id.as_str()) {
            return Err(fail(RpcError::invalid_argument(
                "the request's streamId must be the frame's",
            )));
        }
        let open = open_workspace(&self.state, &self.user).await.map_err(|e| {
            log::warn!(activity = "rpc.stream.error", code = e.code.as_str(); "/rpc/stream failed");
            fail(redact(e, self.state.workspaces.root()))
        })?;

        let generation = self.next_generation;
        self.next_generation += 1;
        let cancelled = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(run_stream(
            Arc::clone(&self.state.core),
            Arc::clone(&open),
            request,
            self.origin.clone(),
            stream_id.clone(),
            kind,
            Arc::clone(&cancelled),
            outbox.clone(),
            ended.clone(),
            generation,
        ));
        self.running.insert(
            stream_id,
            Running {
                open,
                cancelled,
                task,
                generation,
                kind,
            },
        );
        Ok(())
    }
}

fn not_a_stream(request: &Request) -> RpcError {
    RpcError::invalid_argument(format!(
        "{}.{} isn't a stream; only db.queryStream, db.run, db.page, db.tablePage and \
         ai.chat are",
        request.group(),
        request.method()
    ))
}

/// Stops a stream this socket started, as the socket closes: cancels it
/// through Core, and aborts its task, **except a turn's** (phase 6, Task
/// 4's contract). A turn, once cancelled, stores its reply with what
/// streamed and ends; aborting it would drop that write. So its task runs
/// on (sending into an outbox nobody reads, see `run_stream`), and only one
/// still running after [`seaquel_rpc::TURN_STOP_WAIT`] is aborted.
fn stop(core: &seaquel_core::Core, stream_id: &str, running: Running) {
    running.cancelled.store(true, Ordering::SeqCst);
    running.open.workspace().cancel(core, stream_id);
    if running.kind != StreamKind::Ai {
        running.task.abort();
        return;
    }
    let mut task = running.task;
    tokio::spawn(async move {
        if tokio::time::timeout(seaquel_rpc::TURN_STOP_WAIT, &mut task)
            .await
            .is_err()
        {
            task.abort();
            log::warn!(activity = "ai.stop"; "A stopped turn didn't end in time; dropping it");
        }
    });
}

/// Run one stream (a query stream, a run, a page, a table page or a turn)
/// to its end, sending its events to the client. `kind`: the shape of its
/// events, and of the error this sends when it can't be served or ends
/// without a terminal event.
///
/// A turn is polled to its end even once the client is gone (its events
/// are dropped): stopping it is Core's cancel, never dropping the stream.
#[allow(clippy::too_many_arguments)]
async fn run_stream(
    core: Arc<seaquel_core::Core>,
    open: Arc<OpenWorkspace>,
    request: Request,
    origin: WriteOrigin,
    stream_id: String,
    kind: StreamKind,
    cancelled: Arc<AtomicBool>,
    outbox: mpsc::Sender<String>,
    ended: mpsc::UnboundedSender<(String, u64)>,
    generation: u64,
) {
    let error = |code: &str, message: String| CoreEvent::error(&stream_id, kind, code, message);
    let turn = kind == StreamKind::Ai;
    match dispatch_stream(&core, open.workspace(), request, origin) {
        Err(e) => {
            let _ = send(&outbox, &error(&e.code, e.message)).await;
        }
        Ok(mut events) => {
            let mut finished = false;
            // The client is gone (a turn only: anything else stops here).
            let mut gone = false;
            while let Some(event) = events.next().await {
                finished = event.is_terminal();
                if gone {
                    continue;
                }
                let mut frames = Vec::new();
                if let Err(message) = encode_split(event, &mut frames) {
                    if turn {
                        // A turn's events hold no driver data; this can't
                        // happen, and stopping the turn here would lose its
                        // reply. Its terminal event still comes.
                        continue;
                    }
                    // Stop the query (or run) and report it.
                    let _ = send(&outbox, &error("QUERY_ERROR", message)).await;
                    finished = true;
                    break;
                }
                for text in frames {
                    if outbox.send(text).await.is_err() {
                        gone = true;
                        break;
                    }
                }
                if gone && !turn {
                    // The client is gone.
                    finished = true;
                    break;
                }
                if gone && !cancelled.load(Ordering::SeqCst) {
                    // A turn whose socket went: stop it through Core, then
                    // keep polling it so it stores its reply.
                    cancelled.store(true, Ordering::SeqCst);
                    open.workspace().cancel(&core, &stream_id);
                }
            }
            drop(events);
            if !finished && !cancelled.load(Ordering::SeqCst) {
                let _ = send(&outbox, &error(CANCELLED, "The query was stopped.".into())).await;
            }
        }
    }
    drop(open);
    let _ = ended.send((stream_id, generation));
}

/// `event` as JSON frames, a batch (a stream's or a run's) split by rows
/// into frames of at most about [`MAX_BATCH_FRAME_BYTES`], in order: the
/// first piece keeps `columns`, the last keeps `is_final` and `truncated`.
/// A single row larger than that is one frame of its own.
fn encode_split(event: CoreEvent, out: &mut Vec<String>) -> Result<(), String> {
    let text = encode(&event)?;
    let splittable =
        |batch: &StreamBatch| text.len() > MAX_BATCH_FRAME_BYTES && batch.rows.len() > 1;
    let (stream_id, batch, run) = match event {
        CoreEvent::Stream {
            stream_id,
            event: StreamEvent::Batch(batch),
        } if splittable(&batch) => (stream_id, batch, false),
        CoreEvent::Run {
            stream_id,
            event: RunEvent::Batch(batch),
        } if splittable(&batch) => (stream_id, batch, true),
        _ => {
            out.push(text);
            return Ok(());
        }
    };
    drop(text);
    let StreamBatch {
        columns,
        mut rows,
        is_final,
        truncated,
    } = batch;
    let tail = rows.split_off(rows.len() / 2);
    let first = StreamBatch {
        columns,
        rows,
        is_final: false,
        truncated: false,
    };
    let second = StreamBatch {
        columns: None,
        rows: tail,
        is_final,
        truncated,
    };
    for batch in [first, second] {
        let stream_id = stream_id.clone();
        let event = if run {
            CoreEvent::Run {
                stream_id,
                event: RunEvent::Batch(batch),
            }
        } else {
            CoreEvent::Stream {
                stream_id,
                event: StreamEvent::Batch(batch),
            }
        };
        encode_split(event, out)?;
    }
    Ok(())
}

/// Queue `event` for the client without holding up the socket's loop: a
/// frame's refusal waits for room on its own, so the loop keeps reading
/// client frames while the outbox is full. At most [`MAX_PENDING_REFUSALS`]
/// wait at once (`waiting` counts them); past that it sends nothing and
/// returns `false`, and the socket closes with [`TOO_MANY_PENDING`].
fn send_later(outbox: &mpsc::Sender<String>, waiting: &Arc<AtomicUsize>, event: CoreEvent) -> bool {
    if waiting.fetch_add(1, Ordering::SeqCst) >= MAX_PENDING_REFUSALS {
        waiting.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    let outbox = outbox.clone();
    let waiting = Arc::clone(waiting);
    tokio::spawn(async move {
        let _ = send(&outbox, &event).await;
        waiting.fetch_sub(1, Ordering::SeqCst);
    });
    true
}

/// The close frame of a socket that fell behind on events.
fn events_lagged() -> CloseFrame {
    CloseFrame {
        code: close_code::AGAIN,
        reason: format!("{EVENTS_LAGGED}: the socket fell behind on events").into(),
    }
}

/// The close frame of a socket with [`MAX_PENDING_REFUSALS`] refusals
/// waiting: a client that sends refused frames and doesn't read.
fn too_many_pending() -> CloseFrame {
    CloseFrame {
        code: close_code::POLICY,
        reason: format!(
            "{TOO_MANY_PENDING}: {MAX_PENDING_REFUSALS} refusals are waiting to be read"
        )
        .into(),
    }
}

async fn send(outbox: &mpsc::Sender<String>, event: &CoreEvent) -> Result<(), ()> {
    let text = match encode(event) {
        Ok(text) => text,
        Err(_) => return Ok(()), // only batches can fail, and they don't come here
    };
    outbox.send(text).await.map_err(|_| ())
}

/// The longest `streamId` a frame may carry. The clients send UUIDs (36).
pub const MAX_STREAM_ID_LEN: usize = 128;

/// Whether `id` is a `streamId` this socket takes: at most
/// [`MAX_STREAM_ID_LEN`] of `[A-Za-z0-9_.:-]`. It reaches Core's logs and
/// every event, so nothing else is let through (phase 5b review, I1).
fn valid_stream_id(id: &str) -> bool {
    id.len() <= MAX_STREAM_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

/// The frame's `streamId`, if it is JSON with a string one, for the error
/// event of a frame that didn't parse as a [`ClientFrame`].
fn lenient_stream_id(text: &str) -> String {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Id {
        stream_id: Option<String>,
    }
    serde_json::from_str::<Id>(text)
        .ok()
        .and_then(|id| id.stream_id)
        .filter(|id| valid_stream_id(id))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_borrowed_as_its_exact_text() {
        let text = r#"{"op":"start","streamId":"s","request":{"method":"db","params":{"b":1.50,"a":1e+21}}}"#;
        let frame: ClientFrame<'_> = serde_json::from_str(text).unwrap();
        assert_eq!(
            frame.request.unwrap().get(),
            r#"{"method":"db","params":{"b":1.50,"a":1e+21}}"#
        );
    }

    #[test]
    fn a_bad_frames_stream_id_is_kept_when_there_is_one() {
        assert_eq!(lenient_stream_id(r#"{"op":7,"streamId":"s1"}"#), "s1");
        assert_eq!(lenient_stream_id(r#"{"streamId":5}"#), "");
        assert_eq!(lenient_stream_id("not json"), "");
    }
}
