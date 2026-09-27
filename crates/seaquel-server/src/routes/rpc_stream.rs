//! `GET /rpc/stream`: one WebSocket per browser session, for the user in
//! `X-Seaquel-User`. It carries that user's query streams, several at once,
//! and their connection events.
//!
//! Only Node's `/api/rpc/stream` upgrade reaches it. Node sets the header
//! from the session and drops any copy the browser sent, and passes frames
//! through untouched; ownership is checked here, by Core.
//!
//! # Client frames (Text, JSON)
//!
//! ```json
//! {"op":"start","streamId":"s1","request":{"method":"db","params":{"method":"queryStream","params":{"connectionId":"c","streamId":"s1","sql":"SELECT 1"}}}}
//! {"op":"cancel","streamId":"s1"}
//! ```
//!
//! - `start` runs `request`, a `CoreRequest` that must be `db.queryStream`
//!   with the same `streamId`, on the user's workspace (`dispatch_stream`).
//!   `request` is an inline JSON object. It goes to `parse_request` as the
//!   exact bytes it has in the frame (a borrowed `RawValue`, never a
//!   `serde_json::Value`), so the `method`-before-`params` rule holds.
//! - `cancel` cancels a stream this socket started (`Workspace::cancel`).
//!   Nothing more arrives for it. An id this socket isn't running is
//!   ignored.
//!
//! # Server frames (Text, `CoreEvent` JSON)
//!
//! - `{"type":"stream","streamId":…,"event":…}`: a stream's batches, then
//!   one `done` or `error`. A stream that ends without either (another
//!   request cancelled it, or the workspace was evicted) gets a final
//!   `error` with code [`CANCELLED`]; one this socket cancelled gets nothing.
//! - `{"type":"connectionClosed",…}`: one of the user's connections went
//!   away (`WORKSPACE_EVICTED`), from any of the user's workspaces.
//!
//! A frame that can't be served gets a stream `error` event, never a
//! closed socket: `INVALID_ARGUMENT` (not JSON, binary, a bad `op`, a
//! missing or mismatched `streamId`, a bad `request`, a stream id already
//! running on this socket) or [`TOO_MANY_STREAMS`]. Its `streamId` is the
//! frame's when it has a string one, else `""`.
//!
//! # Limits
//!
//! - At most [`MAX_STREAMS`] streams run per socket, and a user has at
//!   most [`MAX_LISTENERS_PER_USER`] sockets; one more is closed at once
//!   with code 1013 and a [`TOO_MANY_SOCKETS`] reason.
//! - A batch whose frame would pass [`MAX_BATCH_FRAME_BYTES`] is split by
//!   rows into several `batch` events.
//! - A frame (and a message) is at most [`MAX_FRAME_BYTES`]; a larger one
//!   closes the socket (WebSocket close code 1009).
//! - Closing the socket, or losing it, cancels every stream it started.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
use seaquel_core::StreamEvent;
use seaquel_rpc::{
    dispatch_stream, parse_request, CoreEvent, DbRequest, Request, RpcError, INVALID_ARGUMENT,
};
use seaquel_types::StreamBatch;
use serde::Deserialize;
use serde_json::value::RawValue;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::rpc::{error_response, open_workspace, redact, user_id};
use crate::workspaces::{ListenError, Listener, OpenWorkspace, MAX_LISTENERS_PER_USER};
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

pub async fn stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let user = match user_id(&headers) {
        Ok(user) => user.to_string(),
        Err(e) => return error_response(e),
    };
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
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| Session::new(state, user).run(socket, listener))
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
}

struct Session {
    state: AppState,
    user: String,
    running: HashMap<String, Running>,
    next_generation: u64,
}

fn error_event(stream_id: &str, code: &str, message: impl Into<String>) -> CoreEvent {
    CoreEvent::Stream {
        stream_id: stream_id.to_string(),
        event: StreamEvent::Error {
            message: message.into(),
            code: code.to_string(),
        },
    }
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
    fn new(state: AppState, user: String) -> Self {
        Self {
            state,
            user,
            running: HashMap::new(),
            next_generation: 0,
        }
    }

    async fn run(mut self, socket: WebSocket, mut listener: Listener) {
        let (mut sink, mut source) = socket.split();
        let (outbox, mut outgoing) = mpsc::channel::<String>(OUTBOX);
        let writer = tokio::spawn(async move {
            while let Some(text) = outgoing.recv().await {
                if sink.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        });
        // Each stream task reports its end, so its slot frees up.
        let (ended_tx, mut ended) = mpsc::unbounded_channel::<(String, u64)>();

        loop {
            tokio::select! {
                frame = source.next() => match frame {
                    Some(Ok(Message::Text(text))) => {
                        if let Some(event) = self.frame(text.as_str(), &outbox, &ended_tx).await {
                            if send(&outbox, &event).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(_))) => {
                        let event = error_event("", INVALID_ARGUMENT, "frames must be JSON text");
                        if send(&outbox, &event).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                    // Closed, lost, or a frame over the limit.
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                },
                Some(event) = listener.recv() => {
                    if send(&outbox, &event).await.is_err() {
                        break;
                    }
                }
                Some((stream_id, generation)) = ended.recv() => {
                    if self.running.get(&stream_id).is_some_and(|r| r.generation == generation) {
                        self.running.remove(&stream_id);
                    }
                }
            }
        }

        // Cancel everything this socket started: through Core, and by
        // dropping each stream.
        for (stream_id, running) in self.running.drain() {
            running.cancelled.store(true, Ordering::SeqCst);
            running
                .open
                .workspace()
                .cancel(&self.state.core, &stream_id);
            running.task.abort();
        }
        drop(outbox);
        writer.abort();
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
        match frame.op.as_str() {
            "start" => {
                let Some(request) = frame.request else {
                    return Some(error_event(
                        &stream_id,
                        INVALID_ARGUMENT,
                        "a start frame needs a request",
                    ));
                };
                self.start(stream_id, request, outbox, ended)
                    .await
                    .err()
                    .map(|(id, e)| error_event(&id, &e.code, e.message))
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
        match &request {
            Request::Db(DbRequest::QueryStream(params)) if params.stream_id != stream_id => {
                return Err(fail(RpcError::invalid_argument(
                    "the request's streamId must be the frame's",
                )));
            }
            Request::Db(DbRequest::QueryStream(_)) => {}
            _ => {
                return Err(fail(RpcError::invalid_argument(format!(
                    "{}.{} isn't a stream; only db.queryStream is",
                    request.group(),
                    request.method()
                ))));
            }
        }
        let open = open_workspace(&self.state, &self.user)
            .await
            .map_err(|e| {
                log::warn!(activity = "rpc.stream.error", code = e.code.as_str(); "/rpc/stream failed: {}: {}", e.code, e.message);
                fail(redact(e, self.state.workspaces.root()))
            })?;

        let generation = self.next_generation;
        self.next_generation += 1;
        let cancelled = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(run_stream(
            Arc::clone(&self.state.core),
            Arc::clone(&open),
            request,
            stream_id.clone(),
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
            },
        );
        Ok(())
    }
}

/// Run one stream to its end, sending its events to the client.
#[allow(clippy::too_many_arguments)]
async fn run_stream(
    core: Arc<seaquel_core::Core>,
    open: Arc<OpenWorkspace>,
    request: Request,
    stream_id: String,
    cancelled: Arc<AtomicBool>,
    outbox: mpsc::Sender<String>,
    ended: mpsc::UnboundedSender<(String, u64)>,
    generation: u64,
) {
    match dispatch_stream(&core, open.workspace(), request) {
        Err(e) => {
            let _ = send(&outbox, &error_event(&stream_id, &e.code, e.message)).await;
        }
        Ok(mut events) => {
            let mut finished = false;
            while let Some(event) = events.next().await {
                if let CoreEvent::Stream { event: e, .. } = &event {
                    finished = matches!(e, StreamEvent::Done | StreamEvent::Error { .. });
                }
                let mut frames = Vec::new();
                if let Err(message) = encode_split(event, &mut frames) {
                    // Stop the query and report it.
                    let _ = send(&outbox, &error_event(&stream_id, "QUERY_ERROR", message)).await;
                    finished = true;
                    break;
                }
                let mut gone = false;
                for text in frames {
                    if outbox.send(text).await.is_err() {
                        gone = true;
                        break;
                    }
                }
                if gone {
                    // The client is gone.
                    finished = true;
                    break;
                }
            }
            drop(events);
            if !finished && !cancelled.load(Ordering::SeqCst) {
                let _ = send(
                    &outbox,
                    &error_event(&stream_id, CANCELLED, "The query was stopped."),
                )
                .await;
            }
        }
    }
    drop(open);
    let _ = ended.send((stream_id, generation));
}

/// `event` as JSON frames, a batch split by rows into frames of at most
/// about [`MAX_BATCH_FRAME_BYTES`]: the first piece keeps `columns`, the
/// last keeps `is_final` and `truncated`. A single row larger than that is
/// one frame of its own.
fn encode_split(event: CoreEvent, out: &mut Vec<String>) -> Result<(), String> {
    let text = encode(&event)?;
    let (stream_id, batch) = match event {
        CoreEvent::Stream {
            stream_id,
            event: StreamEvent::Batch(batch),
        } if text.len() > MAX_BATCH_FRAME_BYTES && batch.rows.len() > 1 => (stream_id, batch),
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
        encode_split(
            CoreEvent::Stream {
                stream_id: stream_id.clone(),
                event: StreamEvent::Batch(batch),
            },
            out,
        )?;
    }
    Ok(())
}

async fn send(outbox: &mpsc::Sender<String>, event: &CoreEvent) -> Result<(), ()> {
    let text = match encode(event) {
        Ok(text) => text,
        Err(_) => return Ok(()), // only batches can fail, and they don't come here
    };
    outbox.send(text).await.map_err(|_| ())
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
