//! The client's half of the wire:
//! one helper process, its frames and its calls.
//!
//! Three tasks run per connection, on the runtime that opened it, owned by
//! the driver's `JoinSet` (dropping the driver aborts them, which drops the
//! child and kills it, [`process::HelperChild`]):
//!
//! - **The writer** writes the client's control frames in the order they
//!   were queued. Its queue is unbounded and every request, `cancel` and
//!   `credit` is queued synchronously, under the state lock, so a cancel
//!   posted from a guard's `Drop` is on the wire before any later call's
//!   request. A request finding nothing queued is written by its caller
//!   instead, at once and without waiting (the hand-off to
//!   the writer task was about 4 µs of a `SELECT 1`); what the pipe doesn't
//!   take then is queued like any frame.
//! - **The reader** reads the helper's frames and hands each to its call's
//!   channel with `try_send`: it never waits on a call, so the helper's
//!   output is always drained. A call's channel holds what its credit lets
//!   the helper send ahead ([`CHANNEL`]); a helper that sends more broke the
//!   protocol. Frames for a call nobody waits for any more are dropped
//!   until its last control frame.
//! - **The exit watcher** owns the child. When the wire ends or the child
//!   exits, it waits for the other (bounded), then fails every waiting call
//!   and every later one with `CONNECTION_CLOSED`, naming the signal or
//!   exit code. A helper that ended its output after `close` and hasn't
//!   exited 2 s later is closing its database (a checkpoint can take
//!   seconds): it is let go, not killed, and exits on its own within its
//!   own bound.
//!
//! **Call ids.** A call's id is live from its request until both its last
//! control frame has arrived and its [`Call`] is dropped; ids are never
//! reused while live, so a late `cancel` or `credit` can't reach another
//! call.
//!
//! **Read-only slots**. The
//! helper runs at most [`MAX_READ_ONLY_CALLS`] read-only calls at once and
//! refuses more, so the client sends no more than that: a read-only call
//! waits for a permit ([`Conn::start_read_only`]), and the permit is held
//! in the call's slot until the call's last frame arrives, also when its
//! handle was dropped and its `cancel` posted. The helper counts a call off
//! before it sends that frame, so a freed permit always finds a free slot
//! there.
//!
//! **How the connection ended** ([`Conn::closed`]): the exit watcher says whether the helper was lost (it ended
//! without `close`: a signal, an exit of its own, a broken protocol) or
//! ended as asked. A dropped driver aborts the watcher, which counts as
//! asked.

use std::collections::HashMap;
use std::io;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use log::{info, warn};
use seaquel_engine::DbError;
use tokio::io::BufReader;
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio::time::{timeout, Instant};

use super::closing::{self, Claim};
use super::process::{self, HelperChild, Started, OPEN_CALL};
use crate::wire::{
    read_frame_async, write_frame, Frame, FrameKind, Reply, Request, MAX_PAYLOAD,
    MAX_READ_ONLY_CALLS, STREAM_CREDIT,
};

/// A call's channel: its schema frame, the batch frames its credit lets
/// the helper send ahead, and its last control frame. The client grants a
/// frame's credit back when it takes the frame out, so a helper that keeps
/// to the protocol never finds the channel full.
const CHANNEL: usize = STREAM_CREDIT as usize + 2;

/// How long the exit watcher waits, once the wire has ended, for the helper
/// to exit (and, once it has exited, for the wire to end) before it kills
/// it and fails the calls anyway.
const EXIT_WAIT: Duration = Duration::from_secs(2);

/// How long `close` waits for the helper to exit. Past it, a helper that
/// took `close` (its output ended) is let go to finish closing its
/// database, and one that didn't is killed.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// The code of every call on a connection whose helper is gone.
pub(super) const CONNECTION_CLOSED: &str = "CONNECTION_CLOSED";

/// What a call on a closed connection is told.
const CLOSED_MESSAGE: &str = "This DuckDB connection is closed.";

/// What arrives for a call.
pub(super) enum Incoming {
    Schema(Vec<u8>),
    Batch(Vec<u8>),
    Reply(Reply),
}

/// A call's state, as the reader and the call's handle see it.
enum Slot {
    /// Both are live: frames go to the handle.
    Waiting(mpsc::Sender<Incoming>),
    /// The handle is gone and a `cancel` was posted; frames are dropped
    /// until the last control frame.
    Discarding,
    /// The last control frame arrived; the handle still holds the id.
    Answered,
}

struct State {
    calls: HashMap<u32, Slot>,
    /// The read-only slots held by calls whose last frame hasn't arrived.
    permits: HashMap<u32, OwnedSemaphorePermit>,
    next: u32,
    /// Why every call fails now: the helper is gone or the connection was
    /// closed.
    dead: Option<String>,
}

impl State {
    /// Whether the helper may still send `id` rows: its last frame hasn't
    /// arrived and its handle is live. Credit for any other call is
    /// pointless.
    fn wants_credit(&self, id: u32) -> bool {
        matches!(self.calls.get(&id), Some(Slot::Waiting(_)))
    }

    /// Hands a frame from the helper to its call. `Err` is a broken
    /// protocol: a frame for no call, after its call's last frame, past its
    /// credit, or a control message that doesn't parse.
    fn deliver(&mut self, frame: Frame) -> Result<(), String> {
        let Some(slot) = self.calls.get_mut(&frame.call) else {
            return Err(format!(
                "a frame for call {} that isn't running",
                frame.call
            ));
        };
        let incoming = match frame.kind {
            FrameKind::Control => {
                let reply = Reply::decode(&frame.payload).map_err(|e| e.message)?;
                // The helper counted the call off before this frame.
                self.permits.remove(&frame.call);
                match std::mem::replace(slot, Slot::Answered) {
                    Slot::Waiting(tx) => {
                        // Room is always left for the last frame; a full
                        // channel means the helper sent past its credit.
                        return tx.try_send(Incoming::Reply(reply)).or_else(|e| match e {
                            mpsc::error::TrySendError::Closed(_) => Ok(()),
                            mpsc::error::TrySendError::Full(_) => {
                                Err("a call's frames past its credit".to_string())
                            }
                        });
                    }
                    Slot::Discarding => {
                        self.calls.remove(&frame.call);
                        return Ok(());
                    }
                    Slot::Answered => return Err("a second last frame for a call".to_string()),
                }
            }
            FrameKind::Schema => Incoming::Schema(frame.payload),
            FrameKind::Batch => Incoming::Batch(frame.payload),
        };
        match slot {
            Slot::Waiting(tx) => tx.try_send(incoming).or_else(|e| match e {
                mpsc::error::TrySendError::Closed(_) => Ok(()),
                mpsc::error::TrySendError::Full(_) => {
                    Err("a call's frames past its credit".to_string())
                }
            }),
            Slot::Discarding => Ok(()),
            Slot::Answered => Err("rows after a call's last frame".to_string()),
        }
    }
}

/// How the connection ended, as [`Conn::closed`] reports it.
#[derive(Clone)]
enum Ending {
    Open,
    /// `close`, or the helper let go while closing.
    AsAsked,
    /// The helper ended without being asked: what every call is told.
    Lost(String),
}

/// Why the exit watcher should stop waiting for the helper.
enum Signal {
    /// The helper's output ended or a write to it failed.
    WireEnded,
    /// The helper broke the protocol, or `close` gave up on a helper that
    /// didn't take it: kill it.
    Kill(String),
    /// `close` stopped waiting for a helper that took it: let it go.
    Detach,
}

/// One helper process and its calls.
pub(super) struct Conn {
    state: Mutex<State>,
    /// Whole frames for the writer task.
    out: mpsc::UnboundedSender<Vec<u8>>,
    /// Frames queued for the writer task and not yet written. Raised under
    /// the state lock, lowered by the writer once a frame is written; a
    /// caller holding the state lock that sees 0 may write itself.
    queued: Arc<AtomicUsize>,
    /// The helper's stdin, written by the writer task, or by a caller that
    /// found nothing queued.
    stdin: Arc<Mutex<ChildStdin>>,
    signals: mpsc::UnboundedSender<Signal>,
    exited: watch::Receiver<bool>,
    stderr_bytes: Arc<AtomicU64>,
    /// `close` was called: the exit is expected.
    closing: std::sync::atomic::AtomicBool,
    /// The helper's output ended: after `close`, the helper took it.
    output_ended: std::sync::atomic::AtomicBool,
    /// [`MAX_READ_ONLY_CALLS`] permits; closed once the connection is dead
    /// or closing, so a waiting call stops waiting.
    read_only: Arc<Semaphore>,
    /// Set by the exit watcher; its sender goes with the watcher.
    ending: watch::Receiver<Ending>,
    /// The file's hold against a second open from this process
    /// ([`closing::claim`]), let go by whichever comes first: the exit
    /// watcher, once the helper is gone or handed to the closing list, or
    /// the driver's drop ([`Conn::let_go`]; aborting the watcher is
    /// asynchronous).
    claim: Mutex<Option<Claim>>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn closed(message: impl Into<String>) -> DbError {
    DbError {
        message: message.into(),
        code: CONNECTION_CLOSED.to_string(),
    }
}

impl Conn {
    /// Takes over a started helper: its tasks join `started.tasks`, which
    /// the driver holds.
    /// `key` is the database's file ([`closing::key`]): a helper let go
    /// while closing it is kept there until it exits.
    /// `claim` holds the file against a second open from this process
    /// until the helper is gone (or, closing, handed to the closing list).
    pub(super) fn run(
        started: Started,
        key: Option<closing::FileKey>,
        claim: Option<Claim>,
    ) -> (Arc<Conn>, JoinSet<()>) {
        let Started {
            child,
            stdin,
            stdout,
            mut tasks,
            stderr_bytes,
            started,
        } = started;
        let (out, out_rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        let stdin = Arc::new(Mutex::new(stdin));
        let (signals, signals_rx) = mpsc::unbounded_channel();
        let (exited_tx, exited) = watch::channel(false);
        let (ending_tx, ending) = watch::channel(Ending::Open);
        let conn = Arc::new(Conn {
            state: Mutex::new(State {
                calls: HashMap::new(),
                permits: HashMap::new(),
                next: OPEN_CALL + 1,
                dead: None,
            }),
            out,
            queued: queued.clone(),
            stdin: stdin.clone(),
            signals: signals.clone(),
            exited,
            stderr_bytes,
            closing: std::sync::atomic::AtomicBool::new(false),
            output_ended: std::sync::atomic::AtomicBool::new(false),
            read_only: Arc::new(Semaphore::new(MAX_READ_ONLY_CALLS)),
            ending,
            claim: Mutex::new(claim),
        });
        // Spawned on the runtime the open ran on (`JoinSet::spawn` needs
        // one, which `process::start` checked).
        tasks.spawn(write(stdin, out_rx, queued, signals.clone()));
        tasks.spawn(read(stdout, conn.clone(), signals));
        tasks.spawn(watch_exit(
            child,
            key,
            conn.clone(),
            signals_rx,
            Announce {
                exited: exited_tx,
                ending: ending_tx,
            },
            started,
        ));
        (conn, tasks)
    }

    /// The error every call gets once the connection is dead.
    pub(super) fn closed_error(&self) -> DbError {
        closed(
            lock(&self.state)
                .dead
                .clone()
                .unwrap_or_else(|| "The DuckDB helper stopped. Reconnect to continue.".into()),
        )
    }

    /// Sends `request` as a new call. Refused before anything is sent when
    /// the connection is dead or the request can't fit a frame.
    pub(super) fn start(self: &Arc<Self>, request: &Request) -> Result<Call, DbError> {
        self.start_with(request, None)
    }

    /// [`Conn::start`] for a call the helper runs on a clone of its own
    /// (`readOnly`, `explainReadOnly`): first waits for one of the
    /// [`MAX_READ_ONLY_CALLS`] slots, which the call holds until its last
    /// frame arrives. Dropping the future while it waits takes no slot.
    pub(super) async fn start_read_only(
        self: &Arc<Self>,
        request: &Request,
    ) -> Result<Call, DbError> {
        // Refused before waiting, as `start` would: a request too large
        // for a frame, or a dead connection.
        let payload = request.encode()?;
        if payload.len() > MAX_PAYLOAD {
            return self.start_with(request, None);
        }
        match self.read_only.clone().acquire_owned().await {
            Ok(permit) => self.start_with(request, Some(permit)),
            // Closed: the connection is dead or closing.
            Err(_) => Err(self.refusal()),
        }
    }

    /// Why a new call is refused now.
    fn refusal(&self) -> DbError {
        if self.closing.load(Ordering::SeqCst) {
            closed(CLOSED_MESSAGE)
        } else {
            self.closed_error()
        }
    }

    fn start_with(
        self: &Arc<Self>,
        request: &Request,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Result<Call, DbError> {
        let payload = request.encode()?;
        if payload.len() > MAX_PAYLOAD {
            return Err(DbError {
                message: format!(
                    "The statement and its values take {} bytes, more than the {MAX_PAYLOAD} \
                     bytes the DuckDB helper takes in one call.",
                    payload.len()
                ),
                code: "INVALID_ARGUMENT".to_string(),
            });
        }
        let mut state = lock(&self.state);
        if self.closing.load(Ordering::SeqCst) {
            return Err(closed(CLOSED_MESSAGE));
        }
        if let Some(dead) = &state.dead {
            return Err(closed(dead.clone()));
        }
        let id = loop {
            let id = state.next;
            state.next = state.next.wrapping_add(1).max(OPEN_CALL + 1);
            if !state.calls.contains_key(&id) {
                break id;
            }
        };
        let (tx, rx) = mpsc::channel(CHANNEL);
        state.calls.insert(id, Slot::Waiting(tx));
        if let Some(permit) = permit {
            state.permits.insert(id, permit);
        }
        // Under the lock, so it is ordered with every cancel: written now
        // when nothing is queued ahead of it, else queued.
        let frame = control_frame(id, &payload)?;
        let written = if self.queued.load(Ordering::SeqCst) == 0 {
            write_now(&self.stdin, &frame)
        } else {
            0
        };
        if written < frame.len() && !self.queue(frame[written..].to_vec()) {
            state.calls.remove(&id);
            state.permits.remove(&id);
            drop(state);
            return Err(self.closed_error());
        }
        Ok(Call {
            conn: self.clone(),
            id,
            rx,
        })
    }

    /// Queues bytes for the writer task. Under the state lock, as every
    /// frame. `false` once the writer is gone.
    fn queue(&self, bytes: Vec<u8>) -> bool {
        self.queued.fetch_add(1, Ordering::SeqCst);
        if self.out.send(bytes).is_err() {
            self.queued.fetch_sub(1, Ordering::SeqCst);
            return false;
        }
        true
    }

    /// Queues a control frame for a call whose id the caller holds live.
    /// Called under the state lock.
    fn post(&self, id: u32, request: &Request) {
        if let Some(frame) = request
            .encode()
            .ok()
            .and_then(|payload| control_frame(id, &payload).ok())
        {
            self.queue(frame);
        }
    }

    /// A call's handle is gone: an unanswered call is cancelled and its
    /// frames dropped from now on; an answered one frees its id.
    fn release(&self, id: u32) {
        let mut state = lock(&self.state);
        match state.calls.get(&id) {
            Some(Slot::Waiting(_)) => {
                state.calls.insert(id, Slot::Discarding);
                // Posted, not awaited, under the lock: on the wire before
                // any call started after this.
                self.post(id, &Request::Cancel);
            }
            Some(Slot::Answered) => {
                state.calls.remove(&id);
            }
            Some(Slot::Discarding) | None => {}
        }
    }

    /// Hands a frame from the helper to its call ([`State::deliver`]).
    fn deliver(&self, frame: Frame) -> Result<(), String> {
        lock(&self.state).deliver(frame)
    }

    /// Every call fails from now on with `why`; the waiting ones see their
    /// channel close. The first reason stays.
    fn fail(&self, why: String) {
        let (calls, permits) = {
            let mut state = lock(&self.state);
            if state.dead.is_none() {
                state.dead = Some(why);
            }
            (
                std::mem::take(&mut state.calls),
                std::mem::take(&mut state.permits),
            )
        };
        self.read_only.close();
        drop(calls);
        drop(permits);
    }

    /// Lets go of the file's claim: a new open of it may start a helper (it
    /// waits out DuckDB's lock while a killed helper is still exiting).
    pub(super) fn let_go(&self) {
        let claim = self
            .claim
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        drop(claim);
    }

    /// Ends when the connection does: `Some` with the error every call now
    /// gets when the helper was lost, `None` when it ended as asked (see
    /// the module docs). Holds no part of the connection.
    pub(super) fn closed(&self) -> seaquel_runtime::BoxFuture<'static, Option<DbError>> {
        let mut ending = self.ending.clone();
        Box::pin(async move {
            let ended = ending
                .wait_for(|e| !matches!(e, Ending::Open))
                .await
                .map(|e| e.clone());
            match ended {
                Ok(Ending::Lost(why)) => Some(closed(why)),
                // As asked, or the watcher was aborted (the driver dropped).
                Ok(_) | Err(_) => None,
            }
        })
    }

    /// Asks the helper to close the database and exit, and waits for it at
    /// most [`CLOSE_WAIT`]. Past that, a helper whose output ended (it took
    /// `close`, and is checkpointing) is let go to finish on its own; one
    /// whose output is still open is killed. New calls are refused from now
    /// on; calls in flight keep their slots and get whatever last frame the
    /// helper sends before it exits (the helper cancels them on `close`),
    /// and the exit fails what is left.
    pub(super) async fn close(&self) {
        {
            // Under the lock, so no call starts after `close` is queued.
            let _state = lock(&self.state);
            self.closing.store(true, Ordering::SeqCst);
            // An open of the same file waits for this helper from now on.
            if let Some(claim) = &*self.claim.lock().unwrap_or_else(PoisonError::into_inner) {
                claim.closing();
            }
            // Read-only calls still waiting for a slot are refused now.
            self.read_only.close();
            // `close` is answered by the helper's exit; its id is never
            // used.
            self.post(0, &Request::Close);
        }
        let mut exited = self.exited.clone();
        if timeout(CLOSE_WAIT, exited.wait_for(|done| *done))
            .await
            .is_err()
        {
            let _ = self
                .signals
                .send(if self.output_ended.load(Ordering::SeqCst) {
                    Signal::Detach
                } else {
                    Signal::Kill("it didn't close in time".to_string())
                });
            let _ = timeout(EXIT_WAIT, exited.wait_for(|done| *done)).await;
        }
    }
}

/// A call in flight. Dropping it before its last frame cancels the call
/// (posted at once, from `Drop`).
pub(super) struct Call {
    conn: Arc<Conn>,
    id: u32,
    rx: mpsc::Receiver<Incoming>,
}

impl Call {
    /// The call's next frame; `CONNECTION_CLOSED` once the helper is gone.
    pub(super) async fn recv(&mut self) -> Result<Incoming, DbError> {
        match self.rx.recv().await {
            Some(incoming) => Ok(incoming),
            None => Err(self.conn.closed_error()),
        }
    }

    /// Lets the helper send one more batch frame, unless the call's last
    /// frame is already in (a small result's batch and `done` come
    /// together). Under the state lock, as every post is.
    pub(super) fn grant(&self) {
        let state = lock(&self.conn.state);
        if state.wants_credit(self.id) {
            self.conn.post(self.id, &Request::Credit { frames: 1 });
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.conn.release(self.id);
    }
}

/// A control frame's bytes, header and payload.
fn control_frame(call: u32, payload: &[u8]) -> Result<Vec<u8>, DbError> {
    let mut frame = Vec::with_capacity(payload.len() + 9);
    write_frame(&mut frame, FrameKind::Control, call, payload)
        .map_err(|e| crate::wire::protocol_error(e.to_string()))?;
    Ok(frame)
}

/// Writes as much of `frame` as the pipe takes now, without waiting, and
/// returns how much that was (0 when the pipe is full or broken: the
/// writer task then meets the same). Unix only: elsewhere tokio writes a
/// child's pipe on its blocking pool, and everything goes through the
/// writer task.
#[cfg(unix)]
fn write_now(stdin: &Mutex<ChildStdin>, frame: &[u8]) -> usize {
    use std::task::{Context, Poll, Waker};
    use tokio::io::AsyncWrite;
    let mut stdin = stdin.lock().unwrap_or_else(PoisonError::into_inner);
    let mut cx = Context::from_waker(Waker::noop());
    let mut written = 0;
    while written < frame.len() {
        match std::pin::Pin::new(&mut *stdin).poll_write(&mut cx, &frame[written..]) {
            Poll::Ready(Ok(n)) if n > 0 => written += n,
            _ => break,
        }
    }
    written
}

#[cfg(not(unix))]
fn write_now(_: &Mutex<ChildStdin>, _: &[u8]) -> usize {
    0
}

/// The writer task: queued frames, in order, each counted off `queued`
/// once written. A failed write means the helper is gone. The stdin lock
/// is taken per poll, never across an await.
async fn write(
    stdin: Arc<Mutex<ChildStdin>>,
    mut queue: mpsc::UnboundedReceiver<Vec<u8>>,
    queued: Arc<AtomicUsize>,
    signals: mpsc::UnboundedSender<Signal>,
) {
    use tokio::io::AsyncWrite;
    while let Some(bytes) = queue.recv().await {
        let mut written = 0;
        let outcome = std::future::poll_fn(|cx| {
            let mut stdin = stdin.lock().unwrap_or_else(PoisonError::into_inner);
            while written < bytes.len() {
                match std::pin::Pin::new(&mut *stdin).poll_write(cx, &bytes[written..]) {
                    std::task::Poll::Ready(Ok(0)) => {
                        return std::task::Poll::Ready(Err(io::ErrorKind::WriteZero.into()))
                    }
                    std::task::Poll::Ready(Ok(n)) => written += n,
                    std::task::Poll::Ready(Err(e)) => return std::task::Poll::Ready(Err(e)),
                    std::task::Poll::Pending => return std::task::Poll::Pending,
                }
            }
            std::pin::Pin::new(&mut *stdin).poll_flush(cx)
        })
        .await;
        queued.fetch_sub(1, Ordering::SeqCst);
        if outcome.is_err() {
            let _ = signals.send(Signal::WireEnded);
            return;
        }
    }
}

/// The reader task: every frame to its call, never waiting on one.
async fn read(
    mut stdout: BufReader<ChildStdout>,
    conn: Arc<Conn>,
    signals: mpsc::UnboundedSender<Signal>,
) {
    loop {
        match read_frame_async(&mut stdout).await {
            Ok(Some(frame)) => {
                if let Err(why) = conn.deliver(frame) {
                    let _ = signals.send(Signal::Kill(why));
                    return;
                }
            }
            Ok(None) => {
                conn.output_ended.store(true, Ordering::SeqCst);
                let _ = signals.send(Signal::WireEnded);
                return;
            }
            // A frame over the limit or of an unknown kind.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => {
                let _ = signals.send(Signal::Kill("a frame that doesn't parse".to_string()));
                return;
            }
            // Cut inside a frame, or a read error: the helper is gone.
            Err(_) => {
                let _ = signals.send(Signal::WireEnded);
                return;
            }
        }
    }
}

/// How the exit watcher let the helper go.
enum Ended {
    /// It exited (or was killed): its status.
    Exited(io::Result<ExitStatus>),
    /// It took `close` and is still closing its database: left to finish.
    Detached,
}

/// What the exit watcher announces once the helper is gone.
struct Announce {
    /// For `close`, which waits for it.
    exited: watch::Sender<bool>,
    /// For [`Conn::closed`]: dropped with the watcher when the driver is.
    ending: watch::Sender<Ending>,
}

/// The exit watcher: once the helper is gone, every call fails, naming how
/// it ended. The file's claim is let go first, then the calls fail, then
/// [`Conn::closed`] hears how it ended, so whoever reacts to either finds
/// the file free (or in the closing list).
async fn watch_exit(
    mut child: HelperChild,
    key: Option<closing::FileKey>,
    conn: Arc<Conn>,
    mut signals: mpsc::UnboundedReceiver<Signal>,
    announce: Announce,
    started: Instant,
) {
    let Announce {
        exited,
        ending: ending_tx,
    } = announce;
    let first = tokio::select! {
        status = child.wait() => Ok(status),
        signal = signals.recv() => Err(signal),
    };
    let closing = || conn.closing.load(Ordering::SeqCst);
    let (ended, broken): (Ended, Option<String>) = match first {
        // Exited: let the reader drain what it wrote before failing calls.
        Ok(status) => {
            let _ = timeout(EXIT_WAIT, wire_ended(&mut signals)).await;
            (Ended::Exited(status), None)
        }
        // The output ended: wait for the exit. A helper that took `close`
        // and hasn't exited by then is closing its database.
        Err(Some(Signal::WireEnded)) => {
            let deadline = tokio::time::sleep(EXIT_WAIT);
            tokio::pin!(deadline);
            loop {
                tokio::select! {
                    status = child.wait() => break (Ended::Exited(status), None),
                    () = &mut deadline => break if closing() {
                        (Ended::Detached, None)
                    } else {
                        (Ended::Exited(process::kill(&mut child).await), None)
                    },
                    signal = signals.recv() => match signal {
                        Some(Signal::Detach) => break (Ended::Detached, None),
                        Some(Signal::Kill(why)) => {
                            break (Ended::Exited(process::kill(&mut child).await), Some(why))
                        }
                        Some(Signal::WireEnded) => {}
                        None => break (Ended::Exited(process::kill(&mut child).await), None),
                    },
                }
            }
        }
        // Only sent once the output ended, so a closing helper.
        Err(Some(Signal::Detach)) => (Ended::Detached, None),
        Err(Some(Signal::Kill(why))) => (Ended::Exited(process::kill(&mut child).await), Some(why)),
        // Every sender is gone: nothing can use this helper.
        Err(None) => (Ended::Exited(process::kill(&mut child).await), None),
    };
    let stderr = conn.stderr_bytes.load(Ordering::Relaxed);
    let ms = started.elapsed().as_millis() as u64;
    let how = match ended {
        Ended::Exited(status) => process::describe(&status),
        Ended::Detached => {
            let child = child.detach();
            if let Some(key) = key {
                // The next open of this file waits for it.
                closing::register(key, child);
            }
            // Registered first, so an open never finds the file in neither
            // list.
            conn.let_go();
            info!(activity = "duckdb.helper", event = "detach", stderr_bytes = stderr, ms = ms; "DuckDB helper still closing its database; left to finish");
            conn.fail(CLOSED_MESSAGE.to_string());
            ending_tx.send_replace(Ending::AsAsked);
            let _ = exited.send(true);
            return;
        }
    };
    if closing() {
        info!(activity = "duckdb.helper", event = "exit", status = how.as_str(), stderr_bytes = stderr, ms = ms; "DuckDB helper exited");
    } else {
        warn!(activity = "duckdb.helper", event = "crash", status = how.as_str(), stderr_bytes = stderr, ms = ms, protocol = broken.is_some(); "DuckDB helper stopped");
    }
    conn.let_go();
    // Read once: `close` racing this exit makes it asked for, never lost.
    let asked = closing();
    let why = match broken {
        None if asked => CLOSED_MESSAGE.to_string(),
        Some(why) => format!(
            "The DuckDB helper broke the protocol ({why}) and was stopped ({how}). Reconnect to \
             continue."
        ),
        None => format!("The DuckDB helper stopped ({how}). Reconnect to continue."),
    };
    conn.fail(why.clone());
    ending_tx.send_replace(if asked {
        Ending::AsAsked
    } else {
        Ending::Lost(why)
    });
    let _ = exited.send(true);
}

/// Waits for the wire's end (or for every sender to go).
async fn wire_ended(signals: &mut mpsc::UnboundedReceiver<Signal>) {
    while let Some(signal) = signals.recv().await {
        if matches!(signal, Signal::WireEnded) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State {
        State {
            calls: HashMap::new(),
            permits: HashMap::new(),
            next: OPEN_CALL + 1,
            dead: None,
        }
    }

    fn rows(call: u32) -> Frame {
        Frame {
            kind: FrameKind::Batch,
            call,
            payload: vec![1, 2, 3],
        }
    }

    fn done(call: u32) -> Frame {
        Frame {
            kind: FrameKind::Control,
            call,
            payload: Reply::Done.encode().unwrap(),
        }
    }

    /// A frame for a call that isn't running breaks the wire, also once
    /// the connection is dead: `close` keeps the calls' slots, so their
    /// last frames always find them.
    #[test]
    fn frames_for_no_call_break_the_wire() {
        let mut s = state();
        assert!(s.deliver(rows(5)).is_err());
        assert!(s.deliver(done(5)).is_err());
        s.dead = Some("closed".to_string());
        assert!(s.deliver(rows(5)).is_err());
        assert!(s.deliver(done(5)).is_err());
    }

    /// The channel holds the schema, the credit window and the last frame;
    /// a helper that sends one batch more is caught.
    #[test]
    fn frames_past_the_credit_break_the_wire() {
        let mut s = state();
        let (tx, _rx) = mpsc::channel(CHANNEL);
        s.calls.insert(7, Slot::Waiting(tx));
        let schema = Frame {
            kind: FrameKind::Schema,
            call: 7,
            payload: vec![],
        };
        assert!(s.deliver(schema).is_ok());
        for _ in 0..STREAM_CREDIT {
            assert!(s.deliver(rows(7)).is_ok());
        }
        assert!(s.deliver(rows(7)).is_ok(), "the last frame's room");
        assert!(s.deliver(done(7)).is_err());
    }

    /// The bytes `cat` wrote to `file` once they equal `want`, waiting up
    /// to 5 s on this thread: the runtime doesn't run meanwhile, so only
    /// what was written without it shows up.
    #[cfg(unix)]
    fn wire_reaches(file: &std::path::Path, want: &[u8]) -> Vec<u8> {
        let mut got = Vec::new();
        for _ in 0..500 {
            got = std::fs::read(file).unwrap_or_default();
            if got.len() >= want.len() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        got
    }

    #[cfg(unix)]
    fn frame(call: u32, request: &Request) -> Vec<u8> {
        let mut out = Vec::new();
        crate::wire::write_frame(
            &mut out,
            FrameKind::Control,
            call,
            &request.encode().unwrap(),
        )
        .unwrap();
        out
    }

    /// A call's request goes on the wire from `start`
    /// itself when nothing is queued ahead of it, without waiting for the
    /// writer task (that hand-off was about 4 µs of a `SELECT 1`). With a
    /// frame queued (a dropped call's `cancel`), the next request queues
    /// behind it, so the order on the wire stays the order of the calls.
    #[cfg(unix)]
    #[test]
    fn a_request_is_written_by_its_caller_when_nothing_is_queued() {
        use std::process::Stdio;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _runtime = rt.enter();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("wire");
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("cat > \"$0\"")
            .arg(&file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        // As the handshake does before the connection starts: tokio knows
        // the pipe is writable once something went through it.
        let mut stdin = child.stdin.take().unwrap();
        let hello = frame(0, &Request::Close);
        rt.block_on(crate::wire::write_frame_async(
            &mut stdin,
            FrameKind::Control,
            0,
            &Request::Close.encode().unwrap(),
        ))
        .unwrap();
        assert_eq!(wire_reaches(&file, &hello), hello);
        let started = Started {
            stdin,
            stdout: BufReader::new(child.stdout.take().unwrap()),
            child: HelperChild::new(child),
            tasks: JoinSet::new(),
            stderr_bytes: Arc::new(AtomicU64::new(0)),
            started: Instant::now(),
        };
        let (conn, _tasks) = Conn::run(started, None, None);
        let query = |sql: &str| Request::Query {
            sql: sql.into(),
            params: vec![],
        };

        let first = conn.start(&query("SELECT 1")).unwrap();
        let mut want = hello;
        want.extend(frame(first.id, &query("SELECT 1")));
        assert_eq!(wire_reaches(&file, &want), want, "written by `start`");

        // Dropped unanswered: its cancel is queued for the writer task,
        // which can't run here. The next request must not overtake it.
        let first_id = first.id;
        drop(first);
        let second = conn.start(&query("SELECT 2")).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(std::fs::read(&file).unwrap(), want, "overtook the cancel");

        rt.block_on(async {
            let mut want = want.clone();
            want.extend(frame(first_id, &Request::Cancel));
            want.extend(frame(second.id, &query("SELECT 2")));
            for _ in 0..500 {
                if std::fs::read(&file).unwrap_or_default().len() >= want.len() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(std::fs::read(&file).unwrap(), want);
        });
    }

    /// Credit is granted only to a call the helper is still sending to:
    /// once its last frame is in, a `credit` frame would only cost the
    /// helper a wake-up (one per small query).
    #[test]
    fn only_a_call_still_answering_wants_credit() {
        let mut s = state();
        let (tx, _rx) = mpsc::channel(CHANNEL);
        s.calls.insert(3, Slot::Waiting(tx));
        assert!(s.wants_credit(3));
        assert!(s.deliver(done(3)).is_ok());
        assert!(!s.wants_credit(3), "answered");
        s.calls.insert(4, Slot::Discarding);
        assert!(!s.wants_credit(4), "discarding");
        assert!(!s.wants_credit(5), "unknown");
    }

    /// A call nobody waits for: its frames are dropped, and its last one
    /// frees its id.
    #[test]
    fn a_discarded_call_drops_frames_until_its_last() {
        let mut s = state();
        s.calls.insert(9, Slot::Discarding);
        assert!(s.deliver(rows(9)).is_ok());
        assert!(s.calls.contains_key(&9));
        assert!(s.deliver(done(9)).is_ok());
        assert!(!s.calls.contains_key(&9));
        // Waited for and answered, the id stays until the handle goes.
        let (tx, mut rx) = mpsc::channel(CHANNEL);
        s.calls.insert(10, Slot::Waiting(tx));
        assert!(s.deliver(done(10)).is_ok());
        assert!(matches!(s.calls.get(&10), Some(Slot::Answered)));
        assert!(matches!(rx.try_recv(), Ok(Incoming::Reply(Reply::Done))));
        assert!(s.deliver(rows(10)).is_err(), "rows after the last frame");
    }
}
