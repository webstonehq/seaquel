//! The DuckDB helper's loop: what the
//! `seaquel-duckdb` binary runs over its stdin and stdout, in `wire.rs`'s
//! frames.
//!
//! Threads:
//!
//! - **The reader** reads frames off the input and hands them to the
//!   dispatcher. It is a thread of its own so the input's end is seen
//!   whatever DuckDB or the output is doing: at the end it cancels every
//!   call itself, then tells the dispatcher, which ends [`serve`].
//! - **The dispatcher** ([`serve`]'s own thread) answers `hello` and `open`,
//!   binds parameters, registers each call, queues it and maps `cancel` and
//!   `credit` to it. It never runs SQL and **never waits for the output**:
//!   its replies go into the output queue.
//! - **The output** is written by whoever holds the turn: the writer thread,
//!   or else a call thread that queues a frame while no one is writing,
//!   which then writes it itself, along with any frames queued ahead of it
//!   (at most [`MAX_QUEUED_BYTES`] of them), instead of waking the writer
//!   thread. Frames go out whole and in queue order, with one flush per
//!   turn. Call threads wait for room in the queue when it holds
//!   [`MAX_QUEUED_BYTES`]; the dispatcher never writes.
//! - **The watchdog** counts the time frames have waited with none written.
//!   Past [`Limits::wedge`] the client counts as wedged (it stopped reading),
//!   and the helper ends with [`EXIT_WEDGED`].
//! - **The main session** runs `query`, `stream`, `execute` and
//!   `transaction` one at a time on the database's main connection, in the
//!   order they came, taking turns ([`crate::blocking`]). It is also the thread that opens the database.
//! - **One thread per read-only call** (`readOnly`, `explainReadOnly`), each
//!   on a clone of its own, dropped after the call; at most
//!   [`MAX_READ_ONLY_CALLS`] at once.
//!
//! Every call (and `open`) ends with exactly one control frame, `done`,
//! `executed`, `committed`, `opened` or `error`, after its rows; its id is
//! live until then. A `cancel` drops the call's [`blocking::Call`]: DuckDB
//! is interrupted only while that call holds its connection, and a call
//! cancelled before its turn never runs, so a late cancel can't reach the
//! next call. Rows go as Arrow IPC, one schema frame and then batch frames
//! under the call's credit ([`STREAM_CREDIT`] at first, then what the
//! client grants), each at most [`MAX_BATCH_FRAME`] bytes unless one row is
//! larger.
//!
//! **The end.** On `close`, at the input's end or when the output closes,
//! every call is interrupted, the output gets a second to write what's
//! queued, and [`serve`] then waits for DuckDB's connections to close (the
//! last one closes the database and checkpoints its WAL into the file) for
//! up to [`SESSION_CLOSE_WAIT`]. After
//! a wedge it doesn't wait, after a broken wire or a refusal it waits
//! [`SESSION_EXIT_WAIT`], and a write the client never reads stops the wait
//! too: that close can't come.
//!
//! The output carries frames only. Nothing here logs: SQL, values and
//! DuckDB's messages go back to the client in frames and nowhere else.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::ExitCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use arrow_ipc::writer::{
    write_message, CompressionContext, DictionaryTracker, EncodedData, IpcDataGenerator,
    IpcWriteOptions,
};
use duckdb::arrow::array::StructArray;
use duckdb::arrow::record_batch::RecordBatch;
use duckdb::types::Value as DuckValue;
use duckdb::{Connection, InterruptHandle, Statement};
use seaquel_engine::{DbError, ExpectRows, TransactionError};

use crate::blocking::{self, Call, Op, Worker};
use crate::kinds;
use crate::session::{self, ChunkSink, Execution, Flow};
use crate::wire::{
    protocol_error, read_frame, schema_payload, write_frame_buffered, Frame, FrameKind, OpenParams,
    Reply, Request, MAX_BATCH_FRAME, MAX_FRAME, MAX_PAYLOAD, PROTOCOL, STREAM_CREDIT,
};

/// The input ended, the client sent `close`, or the output was closed.
pub const EXIT_OK: u8 = 0;
/// The first frame wasn't a `hello` this helper speaks.
pub const EXIT_REFUSED: u8 = 3;
/// The wire broke: a frame over the limit, a frame kind or a control message
/// that doesn't parse, a second `hello`, a call id reused while its call (or
/// `open`) runs.
pub const EXIT_PROTOCOL: u8 = 4;
/// The client stopped reading: frames waited [`Limits::wedge`] with none
/// written.
pub const EXIT_WEDGED: u8 = 5;

// Read-only calls running at once: `wire::MAX_READ_ONLY_CALLS` (16). Past
// it, `TOO_MANY_REQUESTS`; the remote client queues instead of sending more.
use crate::wire::MAX_READ_ONLY_CALLS;

/// What [`serve_with`] allows; [`serve`] uses the defaults.
#[derive(Clone, Copy)]
pub(crate) struct Limits {
    /// How long queued frames may wait with none written before the client
    /// counts as wedged. The client always reads (it has to, or a
    /// stream's credit stalls), so this only ends a helper whose client hung.
    pub wedge: Duration,
    pub max_read_only: usize,
    /// How long a clean end waits for DuckDB's close
    /// ([`SESSION_CLOSE_WAIT`]).
    pub close_wait: Duration,
    /// How long the main session waits before it closes the database
    /// (tests only: a slow close checkpoint).
    #[cfg(test)]
    pub close_delay: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            wedge: Duration::from_secs(30),
            max_read_only: MAX_READ_ONLY_CALLS,
            close_wait: SESSION_CLOSE_WAIT,
            #[cfg(test)]
            close_delay: Duration::ZERO,
        }
    }
}

/// SQL holding this panics inside the call (tests only).
#[cfg(test)]
const PANIC_MARKER: &str = "/* seaquel-helper-test-panic */";

/// The bytes of frames the output queue holds before call threads wait for
/// room. Control replies never wait (the dispatcher's included), so the
/// queue can pass this by their size.
const MAX_QUEUED_BYTES: usize = 2 * MAX_FRAME;

/// How often the watchdog looks at the output.
const WATCHDOG_TICK: Duration = Duration::from_millis(100);

/// How long [`serve`] waits, once it is ending, for queued frames to be
/// written (a refusal's frame, a cancelled call's `error`). Not after a
/// wedge, whose frames can't be written.
const FLUSH_WAIT: Duration = Duration::from_secs(1);

/// How long [`serve`] waits, ending after a broken wire or a refusal, for
/// DuckDB's connections to close. An interrupted call is quick to stop.
const SESSION_EXIT_WAIT: Duration = Duration::from_millis(500);

/// How long [`serve`] waits, ending cleanly (`close`, the input's end, the
/// output closed), for DuckDB's connections to close: the last one to go
/// closes the database, and its checkpoint writes the WAL into the file
/// (a 2.4 GB WAL took about 800 ms, and
/// a WAL cut off is replayed by the next open, which can then outlast the
/// client's 30 s connect timeout). The client stops waiting for the exit
/// after 2 s and leaves the helper to finish; this bound is what makes it
/// exit. After a wedge (exit 5) [`serve`] doesn't wait at all.
const SESSION_CLOSE_WAIT: Duration = Duration::from_secs(60);

/// How often the end looks whether the connections are closed.
const SESSION_EXIT_POLL: Duration = Duration::from_millis(20);

/// The longest DuckDB message an `error` frame carries. One quoting a huge
/// statement is cut, so the frame always fits.
const MAX_ERROR_MESSAGE: usize = 1024 * 1024;

/// Serves one client: frames in on `input`, frames out on `output`, until
/// the input ends, the client sends `close`, the output closes, the client
/// stops reading or the wire breaks. Returns the process's exit code
/// ([`EXIT_OK`], [`EXIT_REFUSED`], [`EXIT_PROTOCOL`], [`EXIT_WEDGED`]).
/// `version` is the app version `helloOk` reports, which the client checks
/// against its own.
///
/// When it returns, every running call has been interrupted and the output
/// takes no more frames; a call DuckDB is slow to stop, or a write the
/// client never reads, may still be running on its thread, which the
/// process's exit ends.
pub fn serve(
    input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
    version: &str,
) -> ExitCode {
    serve_with(input, output, version, Limits::default())
}

/// [`serve`] with explicit limits.
pub(crate) fn serve_with(
    input: impl Read + Send + 'static,
    output: impl Write + Send + 'static,
    version: &str,
    limits: Limits,
) -> ExitCode {
    let (events_tx, events) = mpsc::channel();
    let shared = Arc::new(Shared {
        out: Output::new(events_tx.clone(), output),
        calls: Mutex::new(HashMap::new()),
        read_only_calls: AtomicUsize::new(0),
    });
    let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
        std::thread::Builder::new().name(name.into()).spawn(f)
    };
    let writer = {
        let shared = shared.clone();
        spawn(
            "duckdb-helper-output",
            Box::new(move || shared.out.write_all()),
        )
    };
    let reader = {
        let shared = shared.clone();
        let events = events_tx.clone();
        spawn(
            "duckdb-helper-input",
            Box::new(move || read_input(input, &shared, events)),
        )
    };
    let watchdog = {
        let shared = shared.clone();
        let events = events_tx.clone();
        spawn(
            "duckdb-helper-watchdog",
            Box::new(move || watch_output(&shared.out, limits.wedge, events)),
        )
    };
    if writer.is_err() || reader.is_err() || watchdog.is_err() {
        shared.cancel_all();
        shared.out.finish(Duration::ZERO);
        return ExitCode::from(EXIT_OK);
    }
    let mut dispatcher = Dispatcher {
        shared,
        events: events_tx,
        version: version.to_string(),
        limits,
        greeted: false,
        session: Session::Closed,
        session_thread: None,
    };
    let code = dispatcher.run(&events);
    dispatcher.shut_down(code);
    ExitCode::from(code)
}

/// The reader thread: frames from `input` to the dispatcher. At the input's
/// end it stops every call itself, so nothing the dispatcher is doing can
/// delay that, then says how the input ended.
fn read_input(input: impl Read, shared: &Shared, events: Sender<Event>) {
    let mut input = BufReader::with_capacity(64 * 1024, input);
    loop {
        let event = match read_frame(&mut input) {
            Ok(Some(frame)) => Event::Frame(frame),
            Ok(None) => Event::InputEnded,
            // A frame over the limit or of an unknown kind.
            Err(e) if e.kind() == io::ErrorKind::InvalidData => Event::InputBroken,
            // Cut inside a frame, or a read error: the client is gone.
            Err(_) => Event::InputEnded,
        };
        let last = !matches!(event, Event::Frame(_));
        if last {
            shared.cancel_all();
        }
        if events.send(event).is_err() || last {
            return;
        }
    }
}

/// The watchdog: [`Event::Wedged`] once frames have waited `wedge` with
/// none written. Ends when the output closes.
fn watch_output(out: &Output, wedge: Duration, events: Sender<Event>) {
    let ticks = (wedge.as_millis() / WATCHDOG_TICK.as_millis()).max(1);
    let mut stalled = 0;
    let mut last_written = 0;
    loop {
        std::thread::sleep(WATCHDOG_TICK);
        let Some((waiting, written)) = out.progress() else {
            return;
        };
        if waiting && written == last_written {
            stalled += 1;
            if stalled >= ticks {
                let _ = events.send(Event::Wedged);
                return;
            }
        } else {
            stalled = 0;
        }
        last_written = written;
    }
}

/// What the dispatcher hears about.
enum Event {
    Frame(Frame),
    InputEnded,
    InputBroken,
    OutputClosed,
    Wedged,
    /// The main session opened the database (or didn't), for `open`'s call.
    Opened {
        call: u32,
        outcome: Result<Opened, DbError>,
    },
}

/// What the main session hands back once the database is open.
struct Opened {
    /// The connection read-only calls clone from (it runs nothing itself).
    read_only_source: Connection,
    /// The main connection's interrupt, for its calls' guards.
    interrupt: Arc<InterruptHandle>,
}

/// What the dispatcher and the call threads share.
struct Shared {
    out: Output,
    /// The calls (and the `open`) that haven't sent their last frame, by
    /// the client's id.
    calls: Mutex<HashMap<u32, Live>>,
    /// Read-only calls running now.
    read_only_calls: AtomicUsize,
}

/// A call that hasn't ended. `call` is taken by a cancel (dropping it stops
/// the call); the entry stays until the call's last frame, so its id isn't
/// reused under it. A pending `open` has no `call`.
struct Live {
    call: Option<Call>,
    credit: Arc<Credit>,
}

impl Shared {
    fn is_live(&self, call: u32) -> bool {
        blocking::lock(&self.calls).contains_key(&call)
    }

    fn register(&self, id: u32, call: Option<Call>) -> Arc<Credit> {
        let credit = Arc::new(Credit::new());
        blocking::lock(&self.calls).insert(
            id,
            Live {
                call,
                credit: credit.clone(),
            },
        );
        credit
    }

    /// Stops a call: its credit wait ends and its guard drops, which
    /// interrupts DuckDB if the call holds its connection. Unknown or
    /// finished calls are ignored.
    fn cancel(&self, id: u32) {
        let taken = blocking::lock(&self.calls).get_mut(&id).map(|live| {
            live.credit.stop();
            live.call.take()
        });
        // Dropped outside the lock: the guard waits for the call's phase.
        drop(taken);
    }

    fn cancel_all(&self) {
        let taken: Vec<_> = blocking::lock(&self.calls)
            .values_mut()
            .map(|live| {
                live.credit.stop();
                live.call.take()
            })
            .collect();
        drop(taken);
    }

    fn grant(&self, id: u32, frames: u32) {
        if let Some(live) = blocking::lock(&self.calls).get(&id) {
            live.credit.grant(frames);
        }
    }

    /// The call is over: forget it, then queue its last frame. For the
    /// dispatcher, which never writes.
    fn end(&self, id: u32, reply: Reply) {
        let live = blocking::lock(&self.calls).remove(&id);
        drop(live);
        let _ = self.out.control(id, reply);
    }

    /// [`Shared::end`] from the call's own thread, with the rows its sink
    /// still held: written by the call thread itself when no one else is
    /// writing ([`Output::last`]).
    fn end_on_call_thread(&self, ended: Ended) {
        let live = blocking::lock(&self.calls).remove(&ended.id);
        drop(live);
        let _ = self.out.last(ended.id, ended.held, ended.reply);
    }
}

/// Why the output didn't take a frame.
#[derive(Debug)]
enum Refused {
    /// Larger than a frame can carry: the call's error, not the output's.
    TooLarge(usize),
    /// The output is closed or closing.
    Closed,
}

/// The output: a queue of whole frames, written in order by whoever holds
/// the turn to write (`Queue::writing`): the writer thread
/// ([`Output::write_all`]), or a call thread that queued a frame while no
/// one was writing ([`Output::rows`], [`Output::last`]), which then writes
/// it itself instead of waking the writer thread (that
/// hand-off was a quarter of a `SELECT 1`'s overhead). A call thread
/// writes up to and including its own frames, so it also writes any other
/// calls' frames queued ahead of them, at most the queue limit's worth;
/// frames queued after its own are left to the writer thread. The
/// dispatcher only queues ([`Output::control`]), so it never waits for the
/// client. Whoever writes flushes once per turn.
struct Output {
    queue: Mutex<Queue>,
    changed: Condvar,
    events: Sender<Event>,
    /// The client's end, used only by the holder of the turn. `None` once
    /// the output is closed and its last writer is done with it, which
    /// closes the client's read end.
    sink: Mutex<Option<BufWriter<Box<dyn Write + Send>>>>,
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Frame>,
    /// The payload bytes in `frames`.
    bytes: usize,
    /// Someone holds the turn to write (the writer thread or a call thread).
    writing: bool,
    /// Frames written so far, for the watchdog.
    written: u64,
    /// No new frames; what's queued is written, then the output closes.
    closing: bool,
    /// Nothing more is written.
    closed: bool,
}

/// Who queues a frame, which decides whether it may write it itself.
#[derive(Clone, Copy, PartialEq)]
enum By {
    /// The dispatcher: only queues, never waits.
    Dispatcher,
    /// A call thread: waits for room for rows, and writes when no one else
    /// is.
    Call,
}

impl Output {
    fn new(events: Sender<Event>, output: impl Write + Send + 'static) -> Self {
        Output {
            queue: Mutex::new(Queue::default()),
            changed: Condvar::new(),
            events,
            sink: Mutex::new(Some(BufWriter::with_capacity(
                64 * 1024,
                Box::new(output) as Box<dyn Write + Send>,
            ))),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        blocking::lock(&self.queue)
    }

    fn wait<'a>(&self, queue: MutexGuard<'a, Queue>) -> MutexGuard<'a, Queue> {
        self.changed
            .wait(queue)
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues a control reply from the dispatcher. Never waits and never
    /// writes.
    fn control(&self, call: u32, reply: Reply) -> Result<(), Refused> {
        let frame = (FrameKind::Control, encode_reply(reply)?);
        self.push(call, vec![frame], By::Dispatcher)
    }

    /// A call thread's last frames: the rows its sink still held, then its
    /// control reply. Queued without waiting for room, so the queue can
    /// pass its limit by them: the sink can still hold a whole schema frame
    /// plus batches under [`HOLD_BYTES`]. Written by the call thread itself
    /// when no one is writing.
    fn last(
        &self,
        call: u32,
        mut held: Vec<(FrameKind, Vec<u8>)>,
        reply: Reply,
    ) -> Result<(), Refused> {
        held.push((FrameKind::Control, encode_reply(reply)?));
        self.push(call, held, By::Call)
    }

    /// A call thread's schema and batch frames: wait while the queue is
    /// full, then are queued, and written by the call thread itself when no
    /// one is writing.
    fn rows(&self, call: u32, frames: Vec<(FrameKind, Vec<u8>)>) -> Result<(), Refused> {
        self.push(call, frames, By::Call)
    }

    fn push(&self, call: u32, frames: Vec<(FrameKind, Vec<u8>)>, by: By) -> Result<(), Refused> {
        if let Some((_, payload)) = frames.iter().find(|(_, p)| p.len() > MAX_PAYLOAD) {
            return Err(Refused::TooLarge(payload.len()));
        }
        let bytes: usize = frames.iter().map(|(_, p)| p.len()).sum();
        // Only rows wait for room; a call's last frames don't (the queue
        // can pass the limit by them), and neither does the dispatcher.
        let wait = frames.iter().all(|(kind, _)| *kind != FrameKind::Control);
        let mut queue = self.lock();
        while wait
            && !queue.closing
            && !queue.closed
            && queue.bytes > 0
            && queue.bytes + bytes > MAX_QUEUED_BYTES
        {
            queue = self.wait(queue);
        }
        if queue.closing || queue.closed {
            return Err(Refused::Closed);
        }
        queue.bytes += bytes;
        queue
            .frames
            .extend(frames.into_iter().map(|(kind, payload)| Frame {
                kind,
                call,
                payload,
            }));
        if by == By::Call && !queue.writing {
            // Up to its own frame: the call thread then goes back to its
            // call, and anything queued after is the writer thread's.
            let mine = queue.frames.len();
            queue.writing = true;
            drop(queue);
            self.write_queued(mine);
        } else {
            self.changed.notify_all();
        }
        Ok(())
    }

    /// Whether frames are waiting or being written, and how many have been
    /// written; `None` once the output is closed.
    fn progress(&self) -> Option<(bool, u64)> {
        let queue = self.lock();
        (!queue.closed).then(|| (!queue.frames.is_empty() || queue.writing, queue.written))
    }

    /// The writer thread: takes the turn whenever frames are queued and no
    /// call thread is writing them, and writes them. Ends once the output
    /// is closed.
    fn write_all(&self) {
        loop {
            {
                let mut queue = self.lock();
                loop {
                    if queue.closing && queue.frames.is_empty() && !queue.writing {
                        queue.closed = true;
                        self.changed.notify_all();
                    }
                    if queue.closed {
                        if !queue.writing {
                            drop(queue);
                            // No one can take the turn any more: the
                            // client's read end closes here.
                            blocking::lock(&self.sink).take();
                        }
                        return;
                    }
                    if !queue.writing && !queue.frames.is_empty() {
                        queue.writing = true;
                        break;
                    }
                    queue = self.wait(queue);
                }
            }
            self.write_queued(usize::MAX);
        }
    }

    /// Writes queued frames, at most `budget` of them, flushes once, and
    /// gives the turn up (to the writer thread, when frames are left). The
    /// caller holds the turn. A failed write (the client closed its end:
    /// `EPIPE`) closes the output and tells the dispatcher; once the output
    /// is closed, the client's end is dropped.
    fn write_queued(&self, mut budget: usize) {
        let mut sink = blocking::lock(&self.sink);
        loop {
            let frame = {
                let mut queue = self.lock();
                if queue.closed {
                    queue.writing = false;
                    queue.frames.clear();
                    queue.bytes = 0;
                    self.changed.notify_all();
                    drop(queue);
                    sink.take();
                    return;
                }
                if budget == 0 {
                    None
                } else {
                    budget -= 1;
                    queue.frames.pop_front().inspect(|frame| {
                        queue.bytes -= frame.payload.len();
                        self.changed.notify_all();
                    })
                }
            };
            let written = match (frame, sink.as_mut()) {
                (Some(frame), Some(out)) => {
                    write_frame_buffered(out, frame.kind, frame.call, &frame.payload).map(|()| true)
                }
                // Nothing left, or the budget spent: flush, then give the
                // turn up (unless a frame came in meanwhile and the budget
                // allows it).
                (None, Some(out)) => out.flush().map(|()| false),
                (_, None) => Err(io::ErrorKind::BrokenPipe.into()),
            };
            let mut queue = self.lock();
            match written {
                Ok(true) => {
                    queue.written += 1;
                    self.changed.notify_all();
                }
                Ok(false) => {
                    if queue.frames.is_empty() || budget == 0 {
                        queue.writing = false;
                        self.changed.notify_all();
                        return;
                    }
                }
                // A frame too large is refused before it is queued
                // (`Refused::TooLarge`), so this can't happen; if it did,
                // it would be that frame's problem, not a closed output.
                Err(e) if e.kind() == io::ErrorKind::InvalidData => {}
                Err(_) => {
                    let was_closed = std::mem::replace(&mut queue.closed, true);
                    queue.frames.clear();
                    queue.bytes = 0;
                    queue.writing = false;
                    self.changed.notify_all();
                    drop(queue);
                    sink.take();
                    if !was_closed {
                        let _ = self.events.send(Event::OutputClosed);
                    }
                    return;
                }
            }
        }
    }

    /// Whether someone holds the turn to write: after [`Output::finish`],
    /// a write still in progress.
    fn writing(&self) -> bool {
        self.lock().writing
    }

    /// Takes no more frames, waits up to `wait` for the queued ones to be
    /// written, then closes the output. The wait is measured on a clock, not
    /// counted in waits: where timers are coalesced (a busy CI machine, a
    /// background process on macOS) a 10 ms wait can last far longer.
    fn finish(&self, wait: Duration) {
        // tokio's clock (std's `Instant` is kept out of the engine crates).
        let deadline = tokio::time::Instant::now() + wait;
        let mut queue = self.lock();
        queue.closing = true;
        self.changed.notify_all();
        while (!queue.frames.is_empty() || queue.writing) && !queue.closed {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            if left.is_zero() {
                break;
            }
            queue = self
                .changed
                .wait_timeout(queue, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        queue.closed = true;
        queue.frames.clear();
        queue.bytes = 0;
        let idle = !queue.writing;
        self.changed.notify_all();
        drop(queue);
        if idle {
            // No one holds the turn, and no one can take it now.
            blocking::lock(&self.sink).take();
        }
    }
}

/// A reply as a control frame's payload.
fn encode_reply(reply: Reply) -> Result<Vec<u8>, Refused> {
    match fit(reply).encode() {
        Ok(bytes) => Ok(bytes),
        Err(e) => error_reply(e, None).encode().map_err(|_| Refused::Closed),
    }
}

/// A reply whose frame fits: a DuckDB message past [`MAX_ERROR_MESSAGE`] is
/// cut.
fn fit(reply: Reply) -> Reply {
    match reply {
        Reply::Error { mut error, index } if error.message.len() > MAX_ERROR_MESSAGE => {
            let mut end = MAX_ERROR_MESSAGE;
            while !error.message.is_char_boundary(end) {
                end -= 1;
            }
            error.message.truncate(end);
            error.message.push('…');
            Reply::Error { error, index }
        }
        other => other,
    }
}

fn error_reply(error: DbError, index: Option<usize>) -> Reply {
    Reply::Error { error, index }
}

/// A call's batch frames ahead of the client: [`STREAM_CREDIT`] at first,
/// then what `credit` frames grant (saturating).
struct Credit {
    state: Mutex<CreditState>,
    changed: Condvar,
}

struct CreditState {
    frames: u32,
    stopped: bool,
}

impl Credit {
    fn new() -> Self {
        Credit {
            state: Mutex::new(CreditState {
                frames: STREAM_CREDIT,
                stopped: false,
            }),
            changed: Condvar::new(),
        }
    }

    fn grant(&self, frames: u32) {
        let mut state = blocking::lock(&self.state);
        state.frames = state.frames.saturating_add(frames);
        self.changed.notify_all();
    }

    fn stop(&self) {
        blocking::lock(&self.state).stopped = true;
        self.changed.notify_all();
    }

    /// One frame's credit if the call has it now, without waiting.
    fn try_take(&self) -> bool {
        let mut state = blocking::lock(&self.state);
        if state.stopped || state.frames == 0 {
            return false;
        }
        state.frames -= 1;
        true
    }

    /// Waits for one frame's credit. `false` once the call was stopped.
    fn take(&self) -> bool {
        let mut state = blocking::lock(&self.state);
        loop {
            if state.stopped {
                return false;
            }
            if state.frames > 0 {
                state.frames -= 1;
                return true;
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// One call for a session thread.
struct Job {
    id: u32,
    worker: Worker,
    credit: Arc<Credit>,
    work: Work,
}

enum Work {
    Rows {
        sql: String,
        bound: Vec<DuckValue>,
        execution: Execution,
    },
    Execute {
        sql: String,
        bound: Vec<DuckValue>,
    },
    Transaction {
        statements: Vec<(String, Vec<DuckValue>, Option<ExpectRows>)>,
    },
    ReadOnly {
        sql: String,
        limit: usize,
    },
    /// `sql` is the EXPLAIN statement the helper made of the user's.
    ExplainReadOnly {
        sql: String,
        bound: Vec<DuckValue>,
    },
}

/// The tests' way to make a call panic where DuckDB would.
#[cfg(test)]
fn test_panic(sql: &str) {
    if sql.contains(PANIC_MARKER) {
        panic!("a test panic");
    }
}

#[cfg(not(test))]
fn test_panic(_: &str) {}

/// Runs a call on `conn` and returns its last frame's reply. A panic in
/// DuckDB or in the sink becomes the call's error ([`Worker::run`]), so the
/// thread survives it.
fn run_job(shared: &Shared, conn: &Mutex<Connection>, job: Job) -> Ended {
    let Job {
        id,
        worker,
        credit,
        work,
    } = job;
    // Rows the sink still holds go out with the call's last frame.
    let mut held = Vec::new();
    let mut rows = |sink: FrameSink<'_>, outcome: Result<(), DbError>| {
        held = sink.held;
        match outcome {
            Ok(()) => Reply::Done,
            Err(e) => error_reply(e, None),
        }
    };
    let reply = match work {
        Work::Rows {
            sql,
            bound,
            execution,
        } => {
            // A streamed result goes out batch by batch; a materialized one
            // is ready at once, so its small results go out whole.
            let hold = matches!(execution, Execution::Materialized);
            let mut sink = FrameSink::new(shared, id, &credit, hold);
            let outcome = worker.run(conn, Op::Query, |c, w| {
                test_panic(&sql);
                session::rows(c, w, &sql, &bound, execution, &mut sink)
            });
            rows(sink, outcome)
        }
        Work::Execute { sql, bound } => {
            match worker.run(conn, Op::Execute, |c, w| {
                session::execute(c, w, &sql, &bound)
            }) {
                Ok(rows_affected) => Reply::Executed { rows_affected },
                Err(e) => error_reply(e, None),
            }
        }
        Work::Transaction { statements } => {
            match worker.run(conn, Op::Execute, |c, w| {
                Ok(session::transaction(c, w, &statements))
            }) {
                Ok(Ok(rows_affected)) => Reply::Committed { rows_affected },
                Ok(Err(TransactionError { index, error })) => error_reply(error, index),
                Err(e) => error_reply(e, None),
            }
        }
        Work::ReadOnly { sql, limit } => {
            let mut sink = FrameSink::new(shared, id, &credit, true);
            let outcome = worker.run(conn, Op::Query, |c, w| {
                test_panic(&sql);
                session::read_only(c, w, &sql, limit, &mut sink)
            });
            rows(sink, outcome)
        }
        Work::ExplainReadOnly { sql, bound } => {
            let mut sink = FrameSink::new(shared, id, &credit, true);
            let outcome = worker.run(conn, Op::Query, |c, w| {
                session::explain_read_only(c, w, &sql, &bound, &mut sink)
            });
            rows(sink, outcome)
        }
    };
    Ended { id, held, reply }
}

/// A call that ran: the rows its sink still held and its last frame.
struct Ended {
    id: u32,
    held: Vec<(FrameKind, Vec<u8>)>,
    reply: Reply,
}

/// The rows a sink holds back, at most, so a small result goes out in one
/// write with its last frame.
const HOLD_BYTES: usize = 64 * 1024;

/// The helper's [`ChunkSink`]: the statement's Arrow schema as one schema
/// frame, each chunk as batch frames under the call's credit.
///
/// Frames are held back and sent together, so a small result costs the
/// client one read: the schema until the first batch (or the end); with
/// `hold` (a materialized result, which DuckDB has ready), batches too,
/// while they come to less than [`HOLD_BYTES`] and the call has credit for
/// them. What is still held at the end goes out with the call's last frame
/// ([`Ended`]). Before waiting for credit, everything held is sent, since
/// the client grants credit only for frames it has read.
struct FrameSink<'a> {
    shared: &'a Shared,
    id: u32,
    credit: &'a Credit,
    hold: bool,
    held: Vec<(FrameKind, Vec<u8>)>,
    held_bytes: usize,
    ipc: IpcDataGenerator,
    dictionaries: DictionaryTracker,
    options: IpcWriteOptions,
    compression: CompressionContext,
}

impl<'a> FrameSink<'a> {
    fn new(shared: &'a Shared, id: u32, credit: &'a Credit, hold: bool) -> Self {
        FrameSink {
            shared,
            id,
            credit,
            hold,
            held: Vec::new(),
            held_bytes: 0,
            ipc: IpcDataGenerator::default(),
            dictionaries: DictionaryTracker::new(false),
            options: IpcWriteOptions::default(),
            compression: CompressionContext::default(),
        }
    }

    /// Holds or sends one frame; a batch frame first takes its credit (a
    /// frame of dictionaries alone counts too), sending what is held before
    /// it waits for some. A cancelled call or a closed output stops the
    /// result; a frame too large is `RESULT_TOO_LARGE`.
    fn send(&mut self, kind: FrameKind, payload: Vec<u8>) -> Result<(), DbError> {
        if payload.len() > MAX_PAYLOAD {
            return Err(too_large(payload.len()));
        }
        if kind == FrameKind::Batch && !self.credit.try_take() {
            self.release()?;
            if !self.credit.take() {
                return Err(DbError::query_error("cancelled"));
            }
        }
        self.held_bytes += payload.len();
        self.held.push((kind, payload));
        if kind == FrameKind::Batch && (!self.hold || self.held_bytes >= HOLD_BYTES) {
            self.release()?;
        }
        Ok(())
    }

    /// Sends what is held.
    fn release(&mut self) -> Result<(), DbError> {
        if self.held.is_empty() {
            return Ok(());
        }
        self.held_bytes = 0;
        let frames = std::mem::take(&mut self.held);
        self.shared
            .out
            .rows(self.id, frames)
            .map_err(|refused| match refused {
                Refused::TooLarge(bytes) => too_large(bytes),
                Refused::Closed => DbError::query_error("cancelled"),
            })
    }

    fn message(&self, encoded: EncodedData, into: &mut Vec<u8>) -> Result<(), DbError> {
        write_message(into, encoded, &self.options)
            .map(|_| ())
            .map_err(ipc_error)
    }

    /// One batch as frames of at most [`MAX_BATCH_FRAME`] bytes: sent whole
    /// when it fits, else sliced by rows (each slice again, until it fits
    /// or is one row). The batch's new dictionaries (an ENUM's labels) go
    /// first, in its frame or, when it is sliced, in a frame of their own;
    /// the slices share them and don't send them again.
    fn batch(&mut self, batch: &RecordBatch) -> Result<(), DbError> {
        let (dictionaries, message) = self
            .ipc
            .encode(
                batch,
                &mut self.dictionaries,
                &self.options,
                &mut self.compression,
            )
            .map_err(ipc_error)?;
        let mut head = Vec::new();
        for dictionary in dictionaries {
            self.message(dictionary, &mut head)?;
        }
        let mut body = Vec::new();
        self.message(message, &mut body)?;

        let rows = batch.num_rows();
        if head.len() + body.len() <= MAX_BATCH_FRAME || rows <= 1 {
            head.extend_from_slice(&body);
            return self.send(FrameKind::Batch, head);
        }
        if !head.is_empty() {
            self.send(FrameKind::Batch, head)?;
        }
        let parts = body.len().div_ceil(MAX_BATCH_FRAME).max(2);
        let per_part = rows.div_ceil(parts);
        let mut offset = 0;
        while offset < rows {
            let len = per_part.min(rows - offset);
            self.batch(&batch.slice(offset, len))?;
            offset += len;
        }
        Ok(())
    }
}

impl ChunkSink for FrameSink<'_> {
    /// The schema frame: the columns' kinds from DuckDB's logical types
    /// ([`kinds::of`]), then the Arrow schema message. The
    /// client decodes by the kinds, so a session setting that changes the
    /// Arrow carriers (`arrow_lossless_conversion`) can't change the cells.
    fn columns(&mut self, stmt: &Statement<'_>) -> Result<(), DbError> {
        let schema = stmt.schema();
        let encoded = self.ipc.schema_to_bytes_with_dictionary_tracker(
            &schema,
            &mut self.dictionaries,
            &self.options,
        );
        let mut bytes = Vec::new();
        self.message(encoded, &mut bytes)?;
        let payload = schema_payload(&kinds::of(stmt), &bytes)?;
        self.send(FrameKind::Schema, payload)
    }

    fn chunk(&mut self, chunk: StructArray) -> Result<Flow, DbError> {
        self.batch(&RecordBatch::from(&chunk))?;
        Ok(Flow::Continue)
    }

    fn finish(&mut self) -> Result<(), DbError> {
        Ok(())
    }
}

fn ipc_error(e: impl std::fmt::Display) -> DbError {
    DbError::query_error(format!("Couldn't write DuckDB's result: {e}"))
}

/// A row too large for any frame.
fn too_large(bytes: usize) -> DbError {
    DbError {
        message: format!(
            "A row of the result needs {bytes} bytes, more than the {MAX_PAYLOAD} bytes the \
             DuckDB helper can send at once. Select less of it, e.g. with substr()."
        ),
        code: "RESULT_TOO_LARGE".to_string(),
    }
}

/// DuckDB's library version, as `PRAGMA version` prints it (`v1.5.0`).
fn duckdb_version() -> String {
    // SAFETY: `duckdb_library_version` returns a pointer to a static,
    // NUL-terminated string, valid for the life of the process.
    let version = unsafe { std::ffi::CStr::from_ptr(duckdb::ffi::duckdb_library_version()) };
    version.to_string_lossy().into_owned()
}

/// Turns DuckDB's progress bar off on a connection the helper opens.
/// Defence in depth: the bar is off by default, and the binary points fd 1
/// at stderr, where it would draw it (`seaquel-duckdb`'s `main.rs`).
/// Best effort; a statement can turn it back on.
fn no_progress_bar(conn: &Connection) {
    let _ = conn.execute_batch("SET enable_progress_bar = false");
}

/// The main session's state, as the dispatcher sees it.
enum Session {
    /// No database (before `open`, or after one failed).
    Closed,
    /// `open` is running on the main session's thread; its jobs queue is
    /// handed over once it succeeds.
    Opening(Sender<Job>),
    Open {
        jobs: Sender<Job>,
        interrupt: Arc<InterruptHandle>,
        read_only_source: Connection,
    },
}

struct Dispatcher {
    shared: Arc<Shared>,
    events: Sender<Event>,
    version: String,
    limits: Limits,
    greeted: bool,
    session: Session,
    session_thread: Option<JoinHandle<()>>,
}

/// Stop serving, with this exit code.
type Stop = u8;

impl Dispatcher {
    fn run(&mut self, events: &Receiver<Event>) -> u8 {
        while let Ok(event) = events.recv() {
            let outcome = match event {
                Event::Frame(frame) => self.frame(frame),
                Event::InputEnded | Event::OutputClosed => Err(EXIT_OK),
                Event::InputBroken => Err(EXIT_PROTOCOL),
                Event::Wedged => Err(EXIT_WEDGED),
                Event::Opened { call, outcome } => {
                    self.opened(call, outcome);
                    Ok(())
                }
            };
            if let Err(code) = outcome {
                return code;
            }
        }
        EXIT_OK
    }

    /// Interrupts every call, lets the output write what's queued (not
    /// after a wedge) and closes it, and gives the main session a moment to
    /// drop its connection.
    fn shut_down(&mut self, code: u8) {
        self.shared.cancel_all();
        let flush = if code == EXIT_WEDGED {
            Duration::ZERO
        } else {
            FLUSH_WAIT
        };
        self.shared.out.finish(flush);
        // Closing the jobs queue ends the main session's loop once the
        // interrupted call returns; it then drops the main connection. The
        // read-only clones drop theirs as their interrupted calls end.
        // Whichever goes last closes the database.
        self.session = Session::Closed;
        let wait = match code {
            EXIT_WEDGED => Duration::ZERO,
            EXIT_OK => self.limits.close_wait,
            _ => SESSION_EXIT_WAIT,
        };
        // tokio's clock (std's `Instant` is kept out of the engine crates).
        let deadline = tokio::time::Instant::now() + wait;
        let closed = |thread: &Option<JoinHandle<()>>| {
            thread.as_ref().is_none_or(JoinHandle::is_finished)
                && self.shared.read_only_calls.load(Ordering::SeqCst) == 0
        };
        // A write the client doesn't read can hold a call thread, and with
        // it a connection, for good: once the output stays busy past
        // `SESSION_EXIT_WAIT`, the client hung (as in a wedge), and the
        // close can't come.
        let mut busy_since = None;
        loop {
            if closed(&self.session_thread) {
                break;
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                break;
            }
            if self.shared.out.writing() {
                let since = *busy_since.get_or_insert(now);
                if now - since >= SESSION_EXIT_WAIT {
                    break;
                }
            } else {
                busy_since = None;
            }
            std::thread::sleep(SESSION_EXIT_POLL.min(deadline - now));
        }
    }

    fn reply(&self, call: u32, reply: Reply) {
        let _ = self.shared.out.control(call, reply);
    }

    fn frame(&mut self, frame: Frame) -> Result<(), Stop> {
        let id = frame.call;
        if frame.kind != FrameKind::Control {
            return Err(EXIT_PROTOCOL);
        }
        let request = match Request::decode(&frame.payload) {
            Ok(request) => request,
            Err(e) => {
                self.reply(id, error_reply(e, None));
                return Err(EXIT_PROTOCOL);
            }
        };
        if !self.greeted {
            return self.hello(id, request);
        }
        match request {
            Request::Hello { .. } => {
                self.reply(id, error_reply(protocol_error("a second hello"), None));
                Err(EXIT_PROTOCOL)
            }
            Request::Open(params) => {
                if self.shared.is_live(id) {
                    return Err(EXIT_PROTOCOL);
                }
                self.open(id, params);
                Ok(())
            }
            Request::Cancel => {
                self.shared.cancel(id);
                Ok(())
            }
            Request::Credit { frames } => {
                self.shared.grant(id, frames);
                Ok(())
            }
            Request::Close => Err(EXIT_OK),
            call => self.call(id, call),
        }
    }

    /// The first frame: `hello` with this helper's protocol, or the end.
    fn hello(&mut self, id: u32, request: Request) -> Result<(), Stop> {
        match request {
            Request::Hello { protocol, .. } if protocol == PROTOCOL => {
                self.greeted = true;
                self.reply(
                    id,
                    Reply::HelloOk {
                        protocol: PROTOCOL,
                        version: self.version.clone(),
                        duckdb: duckdb_version(),
                    },
                );
                Ok(())
            }
            Request::Hello { protocol, .. } => {
                self.reply(
                    id,
                    error_reply(
                        protocol_error(format!(
                            "this DuckDB helper speaks protocol {PROTOCOL}, not {protocol}"
                        )),
                        None,
                    ),
                );
                Err(EXIT_REFUSED)
            }
            _ => {
                self.reply(
                    id,
                    error_reply(protocol_error("the first frame must be hello"), None),
                );
                Err(EXIT_REFUSED)
            }
        }
    }

    /// Opens the database on the main session's thread, which then serves
    /// the main session's calls. `open`'s id is live until its answer, which
    /// comes back as [`Event::Opened`].
    fn open(&mut self, id: u32, params: OpenParams) {
        if !matches!(self.session, Session::Closed) {
            self.reply(
                id,
                error_reply(
                    DbError::connection_error("the database is already open"),
                    None,
                ),
            );
            return;
        }
        self.shared.register(id, None);
        let (jobs_tx, jobs) = mpsc::channel();
        let shared = self.shared.clone();
        let events = self.events.clone();
        #[cfg(test)]
        let close_delay = self.limits.close_delay;
        let thread = std::thread::Builder::new()
            .name("duckdb-session".into())
            .spawn(move || {
                main_session(
                    shared,
                    events,
                    id,
                    params,
                    jobs,
                    #[cfg(test)]
                    close_delay,
                )
            });
        match thread {
            Ok(thread) => {
                self.session = Session::Opening(jobs_tx);
                self.session_thread = Some(thread);
            }
            Err(e) => self
                .shared
                .end(id, error_reply(DbError::connection_error(e), None)),
        }
    }

    fn opened(&mut self, id: u32, outcome: Result<Opened, DbError>) {
        let session = std::mem::replace(&mut self.session, Session::Closed);
        match (session, outcome) {
            (Session::Opening(jobs), Ok(opened)) => {
                self.session = Session::Open {
                    jobs,
                    interrupt: opened.interrupt,
                    read_only_source: opened.read_only_source,
                };
                self.shared.end(id, Reply::Opened);
            }
            (_, Err(e)) => {
                self.session_thread = None;
                self.shared.end(id, error_reply(e, None));
            }
            // Not opening (can't happen): leave it closed.
            (_, Ok(_)) => self.shared.end(
                id,
                error_reply(DbError::connection_error("open was abandoned"), None),
            ),
        }
    }

    /// A call: refused when its id is live (the wire is broken), answered
    /// with an error when the database isn't open or a value doesn't bind,
    /// else registered and queued.
    fn call(&mut self, id: u32, request: Request) -> Result<(), Stop> {
        if self.shared.is_live(id) {
            return Err(EXIT_PROTOCOL);
        }
        let Session::Open {
            jobs,
            interrupt,
            read_only_source,
        } = &self.session
        else {
            self.reply(
                id,
                error_reply(DbError::query_error("the database isn't open"), None),
            );
            return Ok(());
        };
        let main = |sql: String,
                    params: Vec<seaquel_engine::Value>,
                    make: fn(String, Vec<DuckValue>) -> Work| {
            session::bind_all(&params).map(|bound| make(sql, bound))
        };
        let work = match request {
            Request::Query { sql, params } => main(sql, params, |sql, bound| Work::Rows {
                sql,
                bound,
                execution: Execution::Materialized,
            }),
            Request::Stream { sql, params } => main(sql, params, |sql, bound| Work::Rows {
                sql,
                bound,
                execution: Execution::Streaming,
            }),
            Request::Execute { sql, params } => {
                main(sql, params, |sql, bound| Work::Execute { sql, bound })
            }
            Request::Transaction { statements } => {
                let bound = statements
                    .into_iter()
                    .enumerate()
                    .map(|(index, s)| {
                        let bound = session::bind_all(&s.params)
                            .map_err(|e| TransactionError::at(index, e))?;
                        Ok((s.sql, bound, s.expect_rows))
                    })
                    .collect::<Result<Vec<_>, TransactionError>>();
                match bound {
                    Ok(statements) => Ok(Work::Transaction { statements }),
                    Err(TransactionError { index, error }) => {
                        self.reply(id, error_reply(error, index));
                        return Ok(());
                    }
                }
            }
            Request::ReadOnly { sql, limit } => {
                self.read_only(id, read_only_source, Work::ReadOnly { sql, limit });
                return Ok(());
            }
            Request::ExplainReadOnly { sql, params } => {
                // One statement only: duckdb-rs's `prepare`
                // runs every statement but the last itself, outside the
                // read-only transaction. Then the helper makes the EXPLAIN.
                if seaquel_sql::scan::split_statements(&sql, seaquel_sql::SqlEngine::Duckdb).len()
                    > 1
                {
                    self.reply(
                        id,
                        error_reply(
                            DbError::read_only(seaquel_engine::EXPLAIN_ONE_STATEMENT),
                            None,
                        ),
                    );
                    return Ok(());
                }
                let work = match session::bind_all(&params) {
                    Ok(bound) => Work::ExplainReadOnly {
                        sql: crate::introspect::explain_sql(&sql, false),
                        bound,
                    },
                    Err(e) => {
                        self.reply(id, error_reply(e, None));
                        return Ok(());
                    }
                };
                self.read_only(id, read_only_source, work);
                return Ok(());
            }
            Request::Hello { .. }
            | Request::Open(_)
            | Request::Cancel
            | Request::Credit { .. }
            | Request::Close => unreachable!("handled by `frame`"),
        };
        let work = match work {
            Ok(work) => work,
            Err(e) => {
                self.reply(id, error_reply(e, None));
                return Ok(());
            }
        };
        let (call, worker) = blocking::call(interrupt.clone());
        let credit = self.shared.register(id, Some(call));
        let job = Job {
            id,
            worker,
            credit,
            work,
        };
        if jobs.send(job).is_err() {
            self.shared.end(
                id,
                error_reply(DbError::query_error("the DuckDB session stopped"), None),
            );
        }
        Ok(())
    }

    /// A read-only call on a clone of its own, on a thread of its own; at
    /// most [`Limits::max_read_only`] at once.
    fn read_only(&self, id: u32, source: &Connection, work: Work) {
        let running = &self.shared.read_only_calls;
        if running.load(Ordering::SeqCst) >= self.limits.max_read_only {
            self.reply(
                id,
                error_reply(
                    DbError {
                        message: format!(
                            "Too many read-only DuckDB queries at once (at most {}). \
                             Wait for one to finish.",
                            self.limits.max_read_only
                        ),
                        code: "TOO_MANY_REQUESTS".to_string(),
                    },
                    None,
                ),
            );
            return;
        }
        let conn = match source.try_clone() {
            Ok(conn) => conn,
            Err(e) => {
                self.reply(id, error_reply(DbError::query_error(e), None));
                return;
            }
        };
        no_progress_bar(&conn);
        let (call, worker) = blocking::call(conn.interrupt_handle());
        let credit = self.shared.register(id, Some(call));
        let job = Job {
            id,
            worker,
            credit,
            work,
        };
        running.fetch_add(1, Ordering::SeqCst);
        let shared = self.shared.clone();
        let spawned = std::thread::Builder::new()
            .name("duckdb-read-only".into())
            .spawn(move || {
                let conn = Mutex::new(conn);
                let ended = run_job(&shared, &conn, job);
                // The clone goes before the count: at the end, `serve`
                // waits for the count, and the last connection to go
                // closes the database.
                drop(conn);
                // Before the last frame, so the client can start another
                // call as soon as it reads it.
                shared.read_only_calls.fetch_sub(1, Ordering::SeqCst);
                shared.end_on_call_thread(ended);
            });
        if let Err(e) = spawned {
            running.fetch_sub(1, Ordering::SeqCst);
            self.shared
                .end(id, error_reply(DbError::query_error(e), None));
        }
    }
}

/// The main session's thread: opens the database, reports it, then runs
/// the main session's calls in order until the dispatcher drops the queue.
fn main_session(
    shared: Arc<Shared>,
    events: Sender<Event>,
    id: u32,
    params: OpenParams,
    jobs: Receiver<Job>,
    #[cfg(test)] close_delay: Duration,
) {
    let opened = catch_unwind(AssertUnwindSafe(|| {
        session::open_sessions(&params.config())
    }))
    .unwrap_or_else(|payload| {
        Err(DbError::connection_error(format!(
            "DuckDB panicked: {}",
            blocking::panic_message(&*payload)
        )))
    });
    let main = match opened {
        Ok((main, read_only_source)) => {
            no_progress_bar(&main);
            let opened = Opened {
                read_only_source,
                interrupt: main.interrupt_handle(),
            };
            if events
                .send(Event::Opened {
                    call: id,
                    outcome: Ok(opened),
                })
                .is_err()
            {
                return;
            }
            Mutex::new(main)
        }
        Err(e) => {
            let _ = events.send(Event::Opened {
                call: id,
                outcome: Err(e),
            });
            return;
        }
    };
    for job in jobs {
        let ended = run_job(&shared, &main, job);
        shared.end_on_call_thread(ended);
    }
    // A slow close checkpoint, before `main` closes the database.
    #[cfg(test)]
    std::thread::sleep(close_delay);
}

// The helper is native only (`helper` links DuckDB), and so are its
// tests: threads and the wall clock are what they measure.
#[cfg(test)]
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::{PipeReader, PipeWriter};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use arrow_array::RecordBatch;
    use seaquel_engine::{BatchStatement, DbError, ExpectRows, RowCap, Value};
    use seaquel_engine_testkit::same_value;

    use super::*;
    use crate::ipc::IpcStream;
    use crate::test_reference as reference;
    use crate::wire::{
        read_frame, read_schema_payload, write_frame, Frame, FrameKind, OpenParams, Reply, Request,
        HELPER_PROTOCOL, MAX_BATCH_FRAME, MAX_FRAME, PROTOCOL, STREAM_CREDIT,
    };

    const VERSION: &str = "2026.10.7-test";

    /// How long a test waits for any one frame before it fails.
    const FRAME_WAIT: Duration = Duration::from_secs(20);

    /// A client talking to [`serve`] over two pipes, in this process.
    /// Dropping it closes the helper's input, which ends `serve`.
    struct Client {
        to: Option<PipeWriter>,
        frames: mpsc::Receiver<std::io::Result<Option<Frame>>>,
        exit: mpsc::Receiver<ExitCode>,
    }

    impl Client {
        fn start() -> Self {
            Self::start_with(Limits::default())
        }

        fn start_with(limits: Limits) -> Self {
            let (helper_in, to) = std::io::pipe().unwrap();
            let (from, helper_out) = std::io::pipe().unwrap();
            Self::over(helper_in, to, from, helper_out, limits)
        }

        fn over(
            helper_in: PipeReader,
            to: PipeWriter,
            from: PipeReader,
            out: PipeWriter,
            limits: Limits,
        ) -> Self {
            let (exit_tx, exit) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = exit_tx.send(serve_with(helper_in, out, VERSION, limits));
            });
            let (tx, frames) = mpsc::channel();
            std::thread::spawn(move || {
                let mut from = from;
                loop {
                    let frame = read_frame(&mut from);
                    let end = !matches!(frame, Ok(Some(_)));
                    if tx.send(frame).is_err() || end {
                        return;
                    }
                }
            });
            Client {
                to: Some(to),
                frames,
                exit,
            }
        }

        /// A client past `hello` and `open` (in memory, one thread, so
        /// results come in order).
        fn open() -> Self {
            Self::open_with_limits(Limits::default())
        }

        fn open_with_limits(limits: Limits) -> Self {
            let mut c = Self::start_with(limits);
            c.hello();
            let reply = c.open_with(OpenParams {
                duckdb_config: Some(BTreeMap::from([("threads".into(), "1".into())])),
                ..OpenParams::default()
            });
            assert!(matches!(reply, Reply::Opened), "{reply:?}");
            c
        }

        fn hello(&mut self) -> Reply {
            self.send(
                1,
                &Request::Hello {
                    protocol: PROTOCOL,
                    version: VERSION.into(),
                },
            );
            self.reply(1)
        }

        fn open_with(&mut self, params: OpenParams) -> Reply {
            self.send(2, &Request::Open(params));
            self.reply(2)
        }

        fn send(&mut self, call: u32, request: &Request) {
            let to = self.to.as_mut().expect("input closed");
            write_frame(to, FrameKind::Control, call, &request.encode().unwrap()).unwrap();
        }

        fn credit(&mut self, call: u32, frames: u32) {
            self.send(call, &Request::Credit { frames });
        }

        /// Closes the helper's input.
        fn close_input(&mut self) {
            self.to = None;
        }

        fn next(&self) -> Frame {
            self.next_within(FRAME_WAIT)
                .expect("no frame from the helper in time")
        }

        fn next_within(&self, wait: Duration) -> Option<Frame> {
            match self.frames.recv_timeout(wait) {
                Ok(Ok(Some(frame))) => Some(frame),
                Ok(Ok(None)) => panic!("the helper's output ended"),
                Ok(Err(e)) => panic!("the helper's output broke: {e}"),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the output reader is gone"),
            }
        }

        /// Whether the helper's output ended (every frame before it read).
        fn output_ended_within(&self, wait: Duration) -> bool {
            match self.frames.recv_timeout(wait) {
                Ok(Ok(None)) | Err(mpsc::RecvTimeoutError::Disconnected) => true,
                Ok(Ok(Some(frame))) => panic!("a frame after the end: {frame:?}"),
                Ok(Err(e)) => panic!("the helper's output broke: {e}"),
                Err(mpsc::RecvTimeoutError::Timeout) => false,
            }
        }

        /// The control reply to `call`, the next frame.
        fn reply(&self, call: u32) -> Reply {
            let frame = self.next();
            assert_eq!(
                (frame.kind, frame.call),
                (FrameKind::Control, call),
                "{frame:?}"
            );
            Reply::decode(&frame.payload).unwrap()
        }

        /// `serve`'s exit code, within `wait`.
        fn exit_within(&self, wait: Duration) -> Option<ExitCode> {
            self.exit.recv_timeout(wait).ok()
        }

        /// A call's rows, read to its end and granting credit as each batch
        /// arrives; frames of other calls are an error.
        fn rows(&mut self, call: u32) -> Result<Rows, DbError> {
            let mut rows = Rows::default();
            let mut stream: Option<IpcStream> = None;
            let mut batches: Vec<RecordBatch> = Vec::new();
            loop {
                let frame = self.next();
                assert_eq!(frame.call, call, "{frame:?}");
                match frame.kind {
                    FrameKind::Schema => {
                        assert!(stream.is_none(), "a second schema");
                        let (kinds, ipc) = read_schema_payload(frame.payload)?;
                        let (s, b) = IpcStream::start_with_kinds(ipc, kinds)?;
                        stream = Some(s);
                        batches.extend(b);
                    }
                    FrameKind::Batch => {
                        rows.frame_sizes.push(frame.payload.len());
                        let s = stream.as_mut().expect("a batch before the schema");
                        batches.extend(s.push(frame.payload)?);
                        self.credit(call, 1);
                    }
                    FrameKind::Control => match Reply::decode(&frame.payload).unwrap() {
                        Reply::Done => break,
                        Reply::Error { error, .. } => return Err(error),
                        other => panic!("{other:?}"),
                    },
                }
            }
            let mut s = stream.expect("no schema");
            batches.extend(s.finish()?);
            let columns = s.columns();
            rows.columns = columns.names.clone();
            let cap = RowCap::fail(seaquel_engine::max_query_rows());
            let mut kept = 0;
            for batch in &batches {
                columns.collect(batch, cap, &mut rows.rows, &mut kept)?;
            }
            Ok(rows)
        }

        fn query(&mut self, call: u32, sql: &str) -> Result<Rows, DbError> {
            self.send(
                call,
                &Request::Query {
                    sql: sql.into(),
                    params: vec![],
                },
            );
            self.rows(call)
        }
    }

    impl Drop for Client {
        fn drop(&mut self) {
            self.close_input();
            // `serve` interrupts what it runs and returns; a test that
            // failed mid-call must not leave DuckDB busy behind it.
            let _ = self.exit.recv_timeout(Duration::from_secs(5));
        }
    }

    #[derive(Default, Debug)]
    struct Rows {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
        frame_sizes: Vec<usize>,
    }

    fn same_rows(a: &[Vec<Value>], b: &[Vec<Value>]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_value(p, q)))
    }

    fn exit_code(code: u8) -> ExitCode {
        ExitCode::from(code)
    }

    /// A query that runs for minutes on one thread, in a few kilobytes.
    const ENDLESS: &str = "SELECT max(md5(i::VARCHAR)) AS m FROM range(3000000000) t(i)";

    /// A streamed query with many batches.
    const MANY_BATCHES: &str = "SELECT i, md5(i::VARCHAR) AS h FROM range(10000000) t(i)";

    // ── Handshake ──

    #[test]
    fn hello_answers_with_the_helper_s_versions() {
        let mut c = Client::start();
        match c.hello() {
            Reply::HelloOk {
                protocol,
                version,
                duckdb,
            } => {
                assert_eq!(protocol, PROTOCOL);
                assert_eq!(version, VERSION);
                assert!(duckdb.starts_with("v1."), "{duckdb}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// Another protocol, or anything but `hello` first: one refusal frame,
    /// nothing else on the output, and exit 3.
    #[test]
    fn a_wrong_protocol_is_refused_with_exit_3() {
        let first_frames = [
            Request::Hello {
                protocol: PROTOCOL + 1,
                version: VERSION.into(),
            },
            Request::Query {
                sql: "SELECT 1".into(),
                params: vec![],
            },
        ];
        for first in first_frames {
            let mut c = Client::start();
            c.send(7, &first);
            match c.reply(7) {
                Reply::Error { error, index } => {
                    assert_eq!((error.code.as_str(), index), (HELPER_PROTOCOL, None));
                }
                other => panic!("{other:?}"),
            }
            assert_eq!(
                c.exit_within(Duration::from_secs(5)),
                Some(exit_code(EXIT_REFUSED))
            );
            assert!(c.output_ended_within(Duration::from_secs(5)));
        }
    }

    #[test]
    fn restricted_open_refuses_options_off_its_allowlist() {
        let mut c = Client::start();
        c.hello();
        let reply = c.open_with(OpenParams {
            restricted: Some(true),
            duckdb_config: Some(BTreeMap::from([("allowed_directories".into(), "/".into())])),
            ..OpenParams::default()
        });
        match reply {
            Reply::Error { error, .. } => assert_eq!(error.code, "INVALID_CONNECTION"),
            other => panic!("{other:?}"),
        }
        // A failed open leaves the helper waiting for another.
        let reply = c.open_with(OpenParams {
            restricted: Some(true),
            duckdb_config: Some(BTreeMap::from([("threads".into(), "1".into())])),
            ..OpenParams::default()
        });
        assert!(matches!(reply, Reply::Opened), "{reply:?}");
        // Restricted: no files.
        let e = c
            .query(10, "SELECT * FROM read_csv('/etc/hosts')")
            .unwrap_err();
        assert!(e.message.contains("disabled"), "{}", e.message);
        // A missing file without create_if_missing.
        let mut c = Client::start();
        c.hello();
        let reply = c.open_with(OpenParams {
            path: Some("/nonexistent/seaquel-helper-test.duckdb".into()),
            ..OpenParams::default()
        });
        match reply {
            Reply::Error { error, .. } => assert_eq!(error.code, "FILE_NOT_FOUND"),
            other => panic!("{other:?}"),
        }
    }

    // ── Each call, against the reference ──

    /// `sql` run by `execute`: its rows affected.
    fn executed(c: &mut Client, id: u32, sql: &str, params: Vec<Value>) -> u64 {
        c.send(
            id,
            &Request::Execute {
                sql: sql.into(),
                params,
            },
        );
        match c.reply(id) {
            Reply::Executed { rows_affected } => rows_affected,
            other => panic!("{sql}: {other:?}"),
        }
    }

    /// Every call kind answers as written down: rows decoded through
    /// `ipc::Columns` against the frozen typed-cell fixture and literal
    /// results (`test_reference`), rows affected, a transaction's counts
    /// and failing index, the read-only and EXPLAIN paths.
    #[test]
    fn each_call_answers_as_the_reference_says() {
        let mut c = Client::open();
        let mut call = 100;
        let mut next = || {
            call += 1;
            call
        };

        let create = "CREATE TABLE t (id INTEGER PRIMARY KEY, name VARCHAR, n DECIMAL(10, 2))";
        assert_eq!(executed(&mut c, next(), create, vec![]), 0);
        let insert = "INSERT INTO t VALUES (1, 'a', 1.5), (2, 'b', NULL), (3, 'c', -2.25)";
        assert_eq!(executed(&mut c, next(), insert, vec![]), 3);

        // query and stream, with binds, over every typed-cell case.
        let mut failures = Vec::new();
        let mut cases: Vec<(reference::Case, Vec<Value>)> =
            reference::all().into_iter().map(|c| (c, vec![])).collect();
        cases.push((
            reference::Case {
                name: "binds".into(),
                setup: vec![],
                teardown: vec![],
                select: "SELECT * FROM t WHERE id >= ? AND name <> ? ORDER BY id".into(),
                expect: reference::Expect::Whole {
                    columns: vec!["id".into(), "name".into(), "n".into()],
                    rows: vec![
                        vec![Value::Int(2), Value::Text("b".into()), Value::Null],
                        vec![
                            Value::Int(3),
                            Value::Text("c".into()),
                            Value::Decimal("-2.25".into()),
                        ],
                    ],
                },
            },
            vec![Value::Int(2), Value::Text("zz".into())],
        ));
        for (case, params) in &cases {
            for sql in &case.setup {
                executed(&mut c, next(), sql, vec![]);
            }
            for stream in [false, true] {
                let id = next();
                let request = if stream {
                    Request::Stream {
                        sql: case.select.clone(),
                        params: params.clone(),
                    }
                } else {
                    Request::Query {
                        sql: case.select.clone(),
                        params: params.clone(),
                    }
                };
                c.send(id, &request);
                match c.rows(id) {
                    Ok(got) => {
                        if let Err(e) = case.check(&got.columns, &got.rows) {
                            failures.push(format!("{e} (stream {stream})"));
                        }
                    }
                    Err(e) => failures.push(format!("{} (stream {stream}): {e:?}", case.select)),
                }
            }
            for sql in &case.teardown {
                executed(&mut c, next(), sql, vec![]);
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));

        // execute: rows affected.
        let update = "UPDATE t SET name = upper(name) WHERE id <= ?";
        assert_eq!(executed(&mut c, next(), update, vec![Value::Int(2)]), 2);

        // transaction: the counts, then a failure naming its statement.
        let ok = vec![
            BatchStatement {
                sql: "INSERT INTO t VALUES (?, 'd', 4)".into(),
                params: vec![Value::Int(4)],
                expect_rows: Some(ExpectRows { min: 1 }),
            },
            BatchStatement {
                sql: "DELETE FROM t WHERE id > 100".into(),
                params: vec![],
                expect_rows: None,
            },
        ];
        let id = next();
        c.send(id, &Request::Transaction { statements: ok });
        match c.reply(id) {
            Reply::Committed { rows_affected } => assert_eq!(rows_affected, [1, 0]),
            other => panic!("{other:?}"),
        }
        let failing = vec![
            BatchStatement {
                sql: "UPDATE t SET name = 'x' WHERE id = 1".into(),
                params: vec![],
                expect_rows: None,
            },
            BatchStatement {
                sql: "UPDATE t SET name = 'y' WHERE id = 999".into(),
                params: vec![],
                expect_rows: Some(ExpectRows { min: 1 }),
            },
        ];
        let id = next();
        c.send(
            id,
            &Request::Transaction {
                statements: failing,
            },
        );
        match c.reply(id) {
            Reply::Error { error, index } => {
                assert_eq!((index, error.code.as_str()), (Some(1), "NO_ROWS_AFFECTED"));
            }
            other => panic!("{other:?}"),
        }
        // Rolled back.
        let check = "SELECT name FROM t WHERE id = 1";
        let got = c.query(next(), check).unwrap();
        assert_eq!(got.rows, vec![vec![Value::Text("A".into())]]);

        // readOnly: DuckDB reads at most limit + 1 rows; the client caps.
        let read = "SELECT id, name FROM t ORDER BY id";
        let id = next();
        c.send(
            id,
            &Request::ReadOnly {
                sql: read.into(),
                limit: 2,
            },
        );
        let got = c.rows(id).unwrap();
        assert_eq!(got.columns, ["id", "name"]);
        assert_eq!(
            got.rows,
            vec![
                vec![Value::Int(1), Value::Text("A".into())],
                vec![Value::Int(2), Value::Text("B".into())],
                vec![Value::Int(3), Value::Text("c".into())],
            ]
        );
        // …and refuses a write.
        let id = next();
        c.send(
            id,
            &Request::ReadOnly {
                sql: "DELETE FROM t".into(),
                limit: 10,
            },
        );
        let got = c.rows(id).unwrap_err();
        assert_eq!(got.code, "READ_ONLY");
        assert!(
            got.message.contains("Expected a single SELECT statement"),
            "{}",
            got.message
        );

        // explainReadOnly: the EXPLAIN's rows, as the plain EXPLAIN gives
        // them on the same session, and one statement only. The client
        // sends the user's SQL; the helper wraps it.
        let user_sql = "SELECT * FROM t WHERE id = ?";
        let id = next();
        c.send(
            id,
            &Request::Query {
                sql: crate::introspect::explain_sql(user_sql, false),
                params: vec![Value::Int(1)],
            },
        );
        let want = c.rows(id).unwrap();
        assert_eq!(want.columns, ["explain_key", "explain_value"]);
        assert_eq!(want.rows.len(), 1, "{want:?}");
        let id = next();
        c.send(
            id,
            &Request::ExplainReadOnly {
                sql: user_sql.into(),
                params: vec![Value::Int(1)],
            },
        );
        let got = c.rows(id).unwrap();
        assert_eq!(got.columns, want.columns);
        assert!(same_rows(&got.rows, &want.rows), "{got:?}");
        let plan = crate::introspect::parse_explain(
            &seaquel_engine::QueryResult {
                columns: got.columns.clone(),
                rows: got.rows.clone(),
            },
            false,
        );
        assert!(
            format!("{plan:?}").contains("SEQ_SCAN") || format!("{plan:?}").contains("SCAN"),
            "{plan:?}"
        );
        let id = next();
        c.send(
            id,
            &Request::ExplainReadOnly {
                sql: "SELECT 1; DELETE FROM t".into(),
                params: vec![],
            },
        );
        let e = c.rows(id).unwrap_err();
        assert_eq!(e.code, "READ_ONLY");
        assert_eq!(
            c.query(next(), "SELECT count(*) AS n FROM t").unwrap().rows,
            vec![vec![Value::Int(4)]]
        );

        // A bind that can't be made fails the call, not the helper.
        let id = next();
        c.send(
            id,
            &Request::Query {
                sql: "SELECT ?".into(),
                params: vec![Value::Array(vec![])],
            },
        );
        assert_eq!(c.rows(id).unwrap_err().code, "QUERY_ERROR");
        assert_eq!(
            c.query(next(), "SELECT 7 AS n").unwrap().rows,
            vec![vec![Value::Int(7)]]
        );
    }

    /// The column kinds the helper sends for `sql` (its schema frame),
    /// as JSON. The rest of the answer is read and dropped.
    fn sent_kinds(c: &mut Client, id: u32, sql: &str) -> serde_json::Value {
        c.send(
            id,
            &Request::Query {
                sql: sql.into(),
                params: vec![],
            },
        );
        let mut kinds = None;
        loop {
            let frame = c.next();
            assert_eq!(frame.call, id, "{frame:?}");
            match frame.kind {
                FrameKind::Schema => {
                    let (k, _) = read_schema_payload(frame.payload).unwrap();
                    kinds = Some(serde_json::to_value(k).unwrap());
                }
                FrameKind::Batch => c.credit(id, 1),
                FrameKind::Control => match Reply::decode(&frame.payload).unwrap() {
                    Reply::Done => break,
                    other => panic!("{sql}: {other:?}"),
                },
            }
        }
        kinds.expect("no schema frame")
    }

    /// The schema frames carry the column kinds recorded in
    /// `tests/fixtures/kinds.json` for every reference case and the
    /// lossy-Arrow ones, so moving where the kinds are
    /// read (`kinds.rs`) changed nothing the client sees.
    /// `SEAQUEL_RECORD_KINDS=1` wrote the file before the move, while the
    /// native driver's `Decoder::of` read them; `kinds.rs`'s own test reads
    /// them through [`kinds::of`] directly.
    #[test]
    fn schema_frames_carry_the_recorded_kinds() {
        let mut c = Client::open();
        let mut id = 1000;
        let mut got = Vec::new();
        for case in crate::kinds::snapshot_cases() {
            for sql in &case.setup {
                id += 1;
                c.send(
                    id,
                    &Request::Execute {
                        sql: sql.clone(),
                        params: vec![],
                    },
                );
                assert!(matches!(c.reply(id), Reply::Executed { .. }), "{sql}");
            }
            id += 1;
            got.push(serde_json::json!({
                "select": case.select,
                "kinds": sent_kinds(&mut c, id, &case.select),
            }));
            for sql in &case.teardown {
                id += 1;
                c.send(
                    id,
                    &Request::Execute {
                        sql: sql.clone(),
                        params: vec![],
                    },
                );
                assert!(matches!(c.reply(id), Reply::Executed { .. }), "{sql}");
            }
        }
        crate::kinds::check_snapshot(got);
    }

    // ── Credit, interleaving, cancel ──

    /// Without `credit` frames, a call sends exactly its window of batch
    /// frames and waits; a `cancel` then ends it with one `error`, and the
    /// main session is free again. Every call that returns rows has the
    /// window, not only `stream`.
    #[test]
    fn a_call_without_credit_sends_its_window_and_waits() {
        let mut c = Client::open();
        let calls = [
            Request::Stream {
                sql: MANY_BATCHES.into(),
                params: vec![],
            },
            Request::Query {
                sql: "SELECT i FROM range(100000) t(i)".into(),
                params: vec![],
            },
            Request::ReadOnly {
                sql: "SELECT i FROM range(100000) t(i)".into(),
                limit: 100_000,
            },
        ];
        for (n, request) in calls.into_iter().enumerate() {
            let call = 10 + n as u32;
            c.send(call, &request);
            let schema = c.next();
            assert_eq!((schema.kind, schema.call), (FrameKind::Schema, call));
            for _ in 0..STREAM_CREDIT {
                let batch = c.next();
                assert_eq!(
                    (batch.kind, batch.call),
                    (FrameKind::Batch, call),
                    "{request:?}"
                );
            }
            assert!(
                c.next_within(Duration::from_millis(500)).is_none(),
                "{request:?} sent past its credit"
            );
            // One credit, one more frame.
            c.credit(call, 1);
            let batch = c.next();
            assert_eq!((batch.kind, batch.call), (FrameKind::Batch, call));
            assert!(c.next_within(Duration::from_millis(300)).is_none());

            c.send(call, &Request::Cancel);
            match c.reply(call) {
                Reply::Error { .. } => {}
                other => panic!("{other:?}"),
            }
            assert!(c.next_within(Duration::from_millis(300)).is_none());
            assert_eq!(
                c.query(50 + call, "SELECT 42 AS n").unwrap().rows,
                vec![vec![Value::Int(42)]]
            );
        }
    }

    /// A read-only call runs on a clone of its own, so it answers while a
    /// main-session stream waits for credit.
    #[test]
    fn a_read_only_call_answers_while_a_stream_waits_for_credit() {
        let mut c = Client::open();
        c.send(
            20,
            &Request::Stream {
                sql: MANY_BATCHES.into(),
                params: vec![],
            },
        );
        assert_eq!(c.next().kind, FrameKind::Schema);
        for _ in 0..STREAM_CREDIT {
            assert_eq!(c.next().kind, FrameKind::Batch);
        }
        c.send(
            21,
            &Request::ReadOnly {
                sql: "SELECT 5 AS n".into(),
                limit: 10,
            },
        );
        let got = c.rows(21).unwrap();
        assert_eq!(got.rows, vec![vec![Value::Int(5)]]);
        // The stream is still where it was, and goes on with credit.
        c.credit(20, 1);
        let frame = c.next();
        assert_eq!((frame.kind, frame.call), (FrameKind::Batch, 20));
        c.send(20, &Request::Cancel);
        assert!(matches!(c.reply(20), Reply::Error { .. }));
    }

    /// `cancel(1)` and `query(2)` back to back: call 1 ends with an error,
    /// call 2 runs to completion (a late interrupt can't reach it). Also
    /// for a call 1 still waiting for the main session: it never runs.
    #[test]
    fn a_cancel_cannot_reach_the_next_call() {
        let mut c = Client::open();
        for round in 0..10u32 {
            let first = 1000 + round * 2;
            let second = first + 1;
            c.send(
                first,
                &Request::Query {
                    sql: ENDLESS.into(),
                    params: vec![],
                },
            );
            // Let it start running (on some rounds), then cancel.
            std::thread::sleep(Duration::from_millis(u64::from(round % 3) * 20));
            c.send(first, &Request::Cancel);
            c.send(
                second,
                &Request::Query {
                    sql: "SELECT 9 AS n".into(),
                    params: vec![],
                },
            );
            // Call 1's end, then call 2's rows; frames of the two never
            // interleave since they take turns on the main session.
            loop {
                let frame = c.next();
                assert_eq!(frame.call, first, "call 2 answered first: {frame:?}");
                // It may have sent its schema before the cancel.
                if frame.kind == FrameKind::Control {
                    assert!(matches!(
                        Reply::decode(&frame.payload).unwrap(),
                        Reply::Error { .. }
                    ));
                    break;
                }
            }
            assert_eq!(c.rows(second).unwrap().rows, vec![vec![Value::Int(9)]]);
        }

        // Queued behind a long call: cancelled before its turn, never runs.
        c.send(
            3000,
            &Request::Query {
                sql: ENDLESS.into(),
                params: vec![],
            },
        );
        c.send(
            3001,
            &Request::Execute {
                sql: "CREATE TABLE never (a INTEGER)".into(),
                params: vec![],
            },
        );
        c.send(3001, &Request::Cancel);
        c.send(3000, &Request::Cancel);
        let mut ended = Vec::new();
        while ended.len() < 2 {
            let frame = c.next();
            if frame.kind == FrameKind::Control {
                assert!(matches!(
                    Reply::decode(&frame.payload).unwrap(),
                    Reply::Error { .. }
                ));
                ended.push(frame.call);
            }
        }
        ended.sort();
        assert_eq!(ended, vec![3000, 3001]);
        let e = c.query(3002, "SELECT * FROM never").unwrap_err();
        assert!(e.message.contains("never"), "{}", e.message);
    }

    // ── Frames ──

    /// A chunk too large for one frame is sliced into frames of at most
    /// `MAX_BATCH_FRAME`; a row larger than that goes alone; a row larger
    /// than a frame can hold is `RESULT_TOO_LARGE`.
    #[test]
    fn batches_are_sliced_to_fit_a_frame() {
        let mut c = Client::open();

        let got = c
            .query(
                30,
                "SELECT i, repeat('x', 65536) || i AS s FROM range(2048) t(i)",
            )
            .unwrap();
        assert_eq!(got.rows.len(), 2048);
        assert!(got.frame_sizes.len() > 1, "{:?}", got.frame_sizes);
        assert!(
            got.frame_sizes.iter().all(|s| *s <= MAX_BATCH_FRAME),
            "{:?}",
            got.frame_sizes
        );
        for (i, row) in got.rows.iter().enumerate() {
            assert_eq!(row[0], Value::Int(i as i64));
            let Value::Text(s) = &row[1] else {
                panic!("{:?}", row[1])
            };
            assert_eq!(s.len(), 65536 + i.to_string().len());
        }

        let big = 12 * 1024 * 1024;
        let got = c
            .query(
                31,
                &format!("SELECT repeat('y', {big}) || i AS s FROM range(2) t(i)"),
            )
            .unwrap();
        assert_eq!(got.rows.len(), 2);
        assert_eq!(got.frame_sizes.len(), 2, "{:?}", got.frame_sizes);
        for size in &got.frame_sizes {
            assert!(*size > MAX_BATCH_FRAME && *size <= MAX_FRAME, "{size}");
        }
        assert!(matches!(&got.rows[1][0], Value::Text(s) if s.len() == big + 1));

        let e = c
            .query(32, "SELECT repeat('z', 20 * 1024 * 1024) AS s")
            .unwrap_err();
        assert_eq!(e.code, "RESULT_TOO_LARGE");
        // The helper is fine afterwards.
        assert_eq!(
            c.query(33, "SELECT 1 AS n").unwrap().rows,
            vec![vec![Value::Int(1)]]
        );
    }

    /// An ENUM's dictionary reaches the client, also when its chunk is
    /// sliced.
    #[test]
    fn dictionaries_survive_slicing() {
        let mut c = Client::open();
        let got = c
            .query(
                40,
                "SELECT (['sad', 'ok'][1 + (i % 2)])::ENUM('sad', 'ok') AS e, \
                 repeat('x', 65536) AS pad FROM range(2048) t(i)",
            )
            .unwrap();
        assert!(got.frame_sizes.len() > 1);
        assert_eq!(got.rows.len(), 2048);
        assert_eq!(got.rows[0][0], Value::Text("sad".into()));
        assert_eq!(got.rows[2047][0], Value::Text("ok".into()));
    }

    // ── Lifecycle ──

    /// The input ending while DuckDB runs a long query: `serve` returns
    /// within a second, with exit 0, and it interrupted the query: the
    /// main session ended before `serve` gave up waiting for it
    /// ([`SESSION_EXIT_WAIT`]).
    #[test]
    fn serve_returns_soon_after_its_input_ends_mid_query() {
        for request in [
            Request::Query {
                sql: ENDLESS.into(),
                params: vec![],
            },
            Request::ReadOnly {
                sql: ENDLESS.into(),
                limit: 10,
            },
        ] {
            let mut c = Client::open();
            c.send(60, &request);
            std::thread::sleep(Duration::from_millis(200));
            let started = Instant::now();
            c.close_input();
            assert_eq!(
                c.exit_within(Duration::from_secs(1)),
                Some(exit_code(EXIT_OK)),
                "after {:?}",
                started.elapsed()
            );
            assert!(
                started.elapsed() < SESSION_EXIT_WAIT,
                "the session didn't stop: {:?}",
                started.elapsed()
            );
        }
    }

    /// `close` ends the helper with exit 0.
    #[test]
    fn close_ends_the_helper() {
        let mut c = Client::open();
        c.send(70, &Request::Close);
        assert_eq!(
            c.exit_within(Duration::from_secs(2)),
            Some(exit_code(EXIT_OK))
        );
    }

    /// A file database with a WAL to checkpoint: `checkpoint_threshold`
    /// raised so the writes stay in the WAL until the database closes.
    fn open_with_wal(c: &mut Client, file: &std::path::Path) {
        c.hello();
        let reply = c.open_with(OpenParams {
            path: Some(file.to_string_lossy().into_owned()),
            create_if_missing: Some(true),
            duckdb_config: Some(BTreeMap::from([("threads".into(), "1".into())])),
            ..OpenParams::default()
        });
        assert!(matches!(reply, Reply::Opened), "{reply:?}");
        for (call, sql) in [
            (3, "SET checkpoint_threshold = '1GB'"),
            (
                4,
                "CREATE TABLE t AS SELECT i, md5(i::VARCHAR) AS h FROM range(100000) r(i)",
            ),
        ] {
            c.send(
                call,
                &Request::Execute {
                    sql: sql.into(),
                    params: vec![],
                },
            );
            let reply = c.reply(call);
            assert!(matches!(reply, Reply::Executed { .. }), "{reply:?}");
        }
    }

    /// On `close`, and at the input's end, `serve` waits for
    /// DuckDB's close (its checkpoint) past the old 500 ms, up to
    /// [`SESSION_CLOSE_WAIT`], so the file is left without a WAL. The
    /// test's main session takes `close_delay` before it closes.
    #[test]
    fn a_slow_close_is_waited_for_on_close_and_at_the_end_of_input() {
        for by_close in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let file = dir.path().join("slow.duckdb");
            let wal = dir.path().join("slow.duckdb.wal");
            let delay = Duration::from_millis(1500);
            let mut c = Client::start_with(Limits {
                close_delay: delay,
                ..Limits::default()
            });
            open_with_wal(&mut c, &file);
            assert!(wal.exists(), "nothing to checkpoint");
            let started = Instant::now();
            if by_close {
                c.send(9, &Request::Close);
            }
            c.close_input();
            assert_eq!(
                c.exit_within(Duration::from_secs(10)),
                Some(exit_code(EXIT_OK)),
                "close: {by_close}"
            );
            assert!(
                started.elapsed() >= delay,
                "returned before the database closed: {:?} (close: {by_close})",
                started.elapsed()
            );
            assert!(!wal.exists(), "the WAL is left (close: {by_close})");
        }
    }

    /// The wait is bounded by `close_wait`, measured on a clock: a close
    /// slower than it is cut there (the process then exits under it).
    #[test]
    fn a_close_slower_than_the_bound_is_cut_at_the_bound() {
        let bound = Duration::from_millis(300);
        let mut c = Client::start_with(Limits {
            close_wait: bound,
            close_delay: Duration::from_secs(3),
            ..Limits::default()
        });
        c.hello();
        let reply = c.open_with(OpenParams::default());
        assert!(matches!(reply, Reply::Opened), "{reply:?}");
        let started = Instant::now();
        c.send(9, &Request::Close);
        assert_eq!(
            c.exit_within(Duration::from_secs(5)),
            Some(exit_code(EXIT_OK))
        );
        let took = started.elapsed();
        assert!(
            took >= bound && took < bound + Duration::from_millis(400),
            "{took:?}"
        );
    }

    /// A wedge (exit 5) doesn't wait for the close: the client hung, and
    /// DuckDB replays the WAL on the next open. As
    /// `a_wedged_client_is_given_up_on`, with a main session that takes
    /// `close_delay` to close.
    #[test]
    fn a_wedge_does_not_wait_for_the_close() {
        let (helper_in, mut to) = std::io::pipe().unwrap();
        let (mut from, helper_out) = std::io::pipe().unwrap();
        let (exit_tx, exit) = mpsc::channel();
        let delay = Duration::from_secs(10);
        let limits = Limits {
            wedge: Duration::from_millis(500),
            close_delay: delay,
            ..Limits::default()
        };
        std::thread::spawn(move || {
            let _ = exit_tx.send(serve_with(helper_in, helper_out, VERSION, limits));
        });
        let mut send = |call: u32, request: Request| {
            write_frame(
                &mut to,
                FrameKind::Control,
                call,
                &request.encode().unwrap(),
            )
            .unwrap();
        };
        send(
            1,
            Request::Hello {
                protocol: PROTOCOL,
                version: VERSION.into(),
            },
        );
        read_frame(&mut from).unwrap().unwrap();
        send(2, Request::Open(OpenParams::default()));
        read_frame(&mut from).unwrap().unwrap();
        send(
            3,
            Request::Stream {
                sql: MANY_BATCHES.into(),
                params: vec![],
            },
        );
        send(3, Request::Credit { frames: u32::MAX });
        let started = Instant::now();
        assert_eq!(
            exit.recv_timeout(Duration::from_secs(5)).ok(),
            Some(exit_code(EXIT_WEDGED))
        );
        assert!(started.elapsed() < delay, "{:?}", started.elapsed());
        drop((to, from));
    }

    /// The output closing (the client gone) ends `serve` too, with exit 0.
    #[test]
    fn serve_returns_when_its_output_closes() {
        let (helper_in, mut to) = std::io::pipe().unwrap();
        let (from, helper_out) = std::io::pipe().unwrap();
        let (exit_tx, exit) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = exit_tx.send(serve(helper_in, helper_out, VERSION));
        });
        let mut from = from;
        let hello = Request::Hello {
            protocol: PROTOCOL,
            version: VERSION.into(),
        };
        write_frame(&mut to, FrameKind::Control, 1, &hello.encode().unwrap()).unwrap();
        read_frame(&mut from).unwrap().unwrap();
        write_frame(
            &mut to,
            FrameKind::Control,
            2,
            &Request::Open(OpenParams::default()).encode().unwrap(),
        )
        .unwrap();
        read_frame(&mut from).unwrap().unwrap();
        drop(from);
        write_frame(
            &mut to,
            FrameKind::Control,
            3,
            &Request::Stream {
                sql: MANY_BATCHES.into(),
                params: vec![],
            }
            .encode()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            exit.recv_timeout(Duration::from_secs(5)).ok(),
            Some(exit_code(EXIT_OK))
        );
    }

    /// A broken wire ends the helper with exit 4: a reused live call id, a
    /// control message that doesn't parse, a frame kind only the helper
    /// sends.
    #[test]
    fn a_broken_wire_is_exit_4() {
        let mut c = Client::open();
        c.send(
            80,
            &Request::Query {
                sql: ENDLESS.into(),
                params: vec![],
            },
        );
        c.send(
            80,
            &Request::Query {
                sql: "SELECT 1".into(),
                params: vec![],
            },
        );
        assert_eq!(
            c.exit_within(Duration::from_secs(2)),
            Some(exit_code(EXIT_PROTOCOL))
        );

        let mut c = Client::open();
        let to = c.to.as_mut().unwrap();
        write_frame(to, FrameKind::Control, 81, b"{\"type\":\"nope\"}").unwrap();
        assert_eq!(
            c.exit_within(Duration::from_secs(2)),
            Some(exit_code(EXIT_PROTOCOL))
        );

        let mut c = Client::open();
        let to = c.to.as_mut().unwrap();
        write_frame(to, FrameKind::Batch, 82, b"").unwrap();
        assert_eq!(
            c.exit_within(Duration::from_secs(2)),
            Some(exit_code(EXIT_PROTOCOL))
        );
    }

    /// Credit and cancel for a call that isn't running are ignored, and so
    /// is a second `credit` after the call ended.
    #[test]
    fn credit_and_cancel_for_unknown_calls_are_ignored() {
        let mut c = Client::open();
        c.send(90, &Request::Cancel);
        c.credit(91, 5);
        assert_eq!(
            c.query(92, "SELECT 3 AS n").unwrap().rows,
            vec![vec![Value::Int(3)]]
        );
        c.send(92, &Request::Cancel);
        c.credit(92, 1);
        assert_eq!(
            c.query(93, "SELECT 4 AS n").unwrap().rows,
            vec![vec![Value::Int(4)]]
        );
    }

    // ── The review's cases (I1, M2, M3, M4, M7) ──

    /// A client that stops reading. `hello` and `open` are read, then a
    /// stream with all the credit it wants is never read, and a second
    /// `open` comes after it (the reviewer's reproduction: the dispatcher
    /// used to block writing that reply). Closing stdin still ends `serve`
    /// within a bound.
    #[test]
    fn an_unread_output_does_not_stop_the_end_of_input() {
        let (helper_in, mut to) = std::io::pipe().unwrap();
        let (mut from, helper_out) = std::io::pipe().unwrap();
        let (exit_tx, exit) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = exit_tx.send(serve(helper_in, helper_out, VERSION));
        });
        let mut send = |call: u32, request: Request| {
            write_frame(
                &mut to,
                FrameKind::Control,
                call,
                &request.encode().unwrap(),
            )
            .unwrap();
        };
        send(
            1,
            Request::Hello {
                protocol: PROTOCOL,
                version: VERSION.into(),
            },
        );
        read_frame(&mut from).unwrap().unwrap();
        send(2, Request::Open(OpenParams::default()));
        read_frame(&mut from).unwrap().unwrap();
        send(
            3,
            Request::Stream {
                sql: MANY_BATCHES.into(),
                params: vec![],
            },
        );
        send(3, Request::Credit { frames: u32::MAX });
        // Let the stream fill the pipe.
        std::thread::sleep(Duration::from_millis(500));
        send(4, Request::Open(OpenParams::default()));
        std::thread::sleep(Duration::from_millis(100));
        drop(to);
        assert_eq!(
            exit.recv_timeout(Duration::from_secs(3)).ok(),
            Some(exit_code(EXIT_OK))
        );
        drop(from);
    }

    /// The same client without closing stdin: past [`Limits::wedge`] with
    /// frames queued and none written, the helper gives up on it.
    #[test]
    fn a_wedged_client_is_given_up_on() {
        let (helper_in, mut to) = std::io::pipe().unwrap();
        let (mut from, helper_out) = std::io::pipe().unwrap();
        let (exit_tx, exit) = mpsc::channel();
        let limits = Limits {
            wedge: Duration::from_millis(500),
            ..Limits::default()
        };
        std::thread::spawn(move || {
            let _ = exit_tx.send(serve_with(helper_in, helper_out, VERSION, limits));
        });
        let mut send = |call: u32, request: Request| {
            write_frame(
                &mut to,
                FrameKind::Control,
                call,
                &request.encode().unwrap(),
            )
            .unwrap();
        };
        send(
            1,
            Request::Hello {
                protocol: PROTOCOL,
                version: VERSION.into(),
            },
        );
        read_frame(&mut from).unwrap().unwrap();
        send(2, Request::Open(OpenParams::default()));
        read_frame(&mut from).unwrap().unwrap();
        send(
            3,
            Request::Stream {
                sql: MANY_BATCHES.into(),
                params: vec![],
            },
        );
        send(3, Request::Credit { frames: u32::MAX });
        assert_eq!(
            exit.recv_timeout(Duration::from_secs(5)).ok(),
            Some(exit_code(EXIT_WEDGED))
        );
        drop((to, from));
    }

    /// At most [`MAX_READ_ONLY_CALLS`] read-only calls at once (each holds a
    /// DuckDB clone and a thread); the next is `TOO_MANY_REQUESTS`, and a
    /// slot frees when a call ends.
    #[test]
    fn read_only_calls_are_capped() {
        let mut c = Client::open();
        let paused = |c: &mut Client, id: u32| {
            c.send(
                id,
                &Request::ReadOnly {
                    sql: "SELECT i FROM range(100000) t(i)".into(),
                    limit: 100_000,
                },
            );
            assert_eq!(c.next().kind, FrameKind::Schema);
            for _ in 0..STREAM_CREDIT {
                assert_eq!(c.next().kind, FrameKind::Batch);
            }
        };
        for id in 0..MAX_READ_ONLY_CALLS as u32 {
            paused(&mut c, 100 + id);
        }
        c.send(
            200,
            &Request::ReadOnly {
                sql: "SELECT 1".into(),
                limit: 10,
            },
        );
        match c.reply(200) {
            Reply::Error { error, .. } => assert_eq!(error.code, "TOO_MANY_REQUESTS"),
            other => panic!("{other:?}"),
        }
        c.send(100, &Request::Cancel);
        assert!(matches!(c.reply(100), Reply::Error { .. }));
        c.send(
            201,
            &Request::ReadOnly {
                sql: "SELECT 2 AS n".into(),
                limit: 10,
            },
        );
        assert_eq!(c.rows(201).unwrap().rows, vec![vec![Value::Int(2)]]);
        // The main session isn't counted.
        assert_eq!(
            c.query(202, "SELECT 3 AS n").unwrap().rows,
            vec![vec![Value::Int(3)]]
        );
        for id in 101..100 + MAX_READ_ONLY_CALLS as u32 {
            c.send(id, &Request::Cancel);
            assert!(matches!(c.reply(id), Reply::Error { .. }));
        }
    }

    /// `open`'s call id is live until `opened`: reusing it at once is a
    /// broken wire.
    #[test]
    fn an_open_s_id_is_live_until_it_is_answered() {
        let mut c = Client::start();
        c.hello();
        let mut both = Vec::new();
        write_frame(
            &mut both,
            FrameKind::Control,
            2,
            &Request::Open(OpenParams::default()).encode().unwrap(),
        )
        .unwrap();
        write_frame(
            &mut both,
            FrameKind::Control,
            2,
            &Request::Query {
                sql: "SELECT 1".into(),
                params: vec![],
            }
            .encode()
            .unwrap(),
        )
        .unwrap();
        c.to.as_mut().unwrap().write_all(&both).unwrap();
        assert_eq!(
            c.exit_within(Duration::from_secs(5)),
            Some(exit_code(EXIT_PROTOCOL))
        );
    }

    /// A frame too large to write is the call's error; the output stays
    /// open for everyone else.
    /// What an [`Output`] wrote, and how often it was flushed.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<(Vec<u8>, usize)>>);

    impl Recorder {
        fn frames(&self) -> Vec<(FrameKind, u32)> {
            let bytes = self.0.lock().unwrap().0.clone();
            let mut input = &bytes[..];
            let mut out = Vec::new();
            while let Some(frame) = read_frame(&mut input).unwrap() {
                out.push((frame.kind, frame.call));
            }
            out
        }

        fn flushes(&self) -> usize {
            self.0.lock().unwrap().1
        }
    }

    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().0.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.lock().unwrap().1 += 1;
            Ok(())
        }
    }

    /// The H-1 latency fix: a call thread that finds no one writing writes
    /// its frames itself, with whatever was queued ahead of them, in order,
    /// flushing once per turn, instead of waking the writer thread. The
    /// dispatcher's replies are only queued: it never writes.
    #[test]
    fn a_call_thread_writes_when_no_one_else_is() {
        let (events, _rx) = mpsc::channel();
        let sink = Recorder::default();
        let out = Output::new(events, sink.clone());
        out.control(1, Reply::Opened).unwrap();
        assert!(sink.frames().is_empty(), "the dispatcher wrote");

        out.rows(2, vec![(FrameKind::Schema, b"schema".to_vec())])
            .unwrap();
        assert_eq!(
            sink.frames(),
            [(FrameKind::Control, 1), (FrameKind::Schema, 2)]
        );
        assert_eq!(sink.flushes(), 1);
        out.last(2, vec![], Reply::Done).unwrap();
        assert_eq!(
            sink.frames(),
            [
                (FrameKind::Control, 1),
                (FrameKind::Schema, 2),
                (FrameKind::Control, 2)
            ]
        );
        assert_eq!(sink.flushes(), 2);

        // Closed: nothing more is taken or written.
        out.finish(Duration::ZERO);
        assert!(matches!(
            out.rows(3, vec![(FrameKind::Batch, vec![1])]),
            Err(Refused::Closed)
        ));
        assert_eq!(sink.frames().len(), 3);
    }

    #[test]
    fn a_frame_too_large_is_the_call_s_error_not_a_closed_output() {
        let (events, _rx) = mpsc::channel();
        let out = Output::new(events, Recorder::default());
        match out.rows(1, vec![(FrameKind::Batch, vec![0; MAX_PAYLOAD + 1])]) {
            Err(Refused::TooLarge(bytes)) => assert_eq!(bytes, MAX_PAYLOAD + 1),
            Err(Refused::Closed) => panic!("closed"),
            Ok(()) => panic!("queued"),
        }
        assert!(out.rows(1, vec![(FrameKind::Batch, vec![0; 10])]).is_ok());
        assert!(out.control(1, Reply::Done).is_ok());
    }

    /// `close` while a stream waits for credit ends the helper.
    #[test]
    fn close_ends_the_helper_while_a_stream_is_paused() {
        let mut c = Client::open();
        c.send(
            70,
            &Request::Stream {
                sql: MANY_BATCHES.into(),
                params: vec![],
            },
        );
        assert_eq!(c.next().kind, FrameKind::Schema);
        for _ in 0..STREAM_CREDIT {
            assert_eq!(c.next().kind, FrameKind::Batch);
        }
        c.send(71, &Request::Close);
        assert_eq!(
            c.exit_within(Duration::from_secs(2)),
            Some(exit_code(EXIT_OK))
        );
    }

    /// Credit saturates: `u32::MAX` twice is all the credit there is, not
    /// an overflow.
    #[test]
    fn credit_saturates() {
        let mut c = Client::open();
        c.send(
            75,
            &Request::Stream {
                sql: "SELECT i FROM range(300000) t(i)".into(),
                params: vec![],
            },
        );
        c.credit(75, u32::MAX);
        c.credit(75, u32::MAX);
        let mut batches = 0;
        loop {
            let frame = c.next();
            assert_eq!(frame.call, 75);
            match frame.kind {
                FrameKind::Batch => batches += 1,
                FrameKind::Control => {
                    assert!(matches!(
                        Reply::decode(&frame.payload).unwrap(),
                        Reply::Done
                    ));
                    break;
                }
                FrameKind::Schema => {}
            }
        }
        assert!(batches > STREAM_CREDIT as usize, "{batches}");
        assert_eq!(
            c.query(76, "SELECT 1 AS n").unwrap().rows,
            vec![vec![Value::Int(1)]]
        );
    }

    /// Lengths that can't be frames: too short for a header, or over the
    /// limit, are a broken wire (exit 4); input cut inside a frame is the
    /// client gone (exit 0).
    #[test]
    fn bad_lengths_end_the_helper() {
        let cases: [(&[u8], u8); 3] = [
            (&[3, 0, 0, 0, 0, 0, 0], EXIT_PROTOCOL),
            (&[0xff, 0xff, 0xff, 0x7f, 0, 1, 0, 0, 0], EXIT_PROTOCOL),
            (&[40, 0, 0, 0, 0, 1, 0, 0, 0, b'{'], EXIT_OK),
        ];
        for (bytes, code) in cases {
            let mut c = Client::start();
            c.hello();
            c.to.as_mut().unwrap().write_all(bytes).unwrap();
            c.close_input();
            assert_eq!(
                c.exit_within(Duration::from_secs(5)),
                Some(exit_code(code)),
                "{bytes:?}"
            );
        }
    }

    /// A panic inside a call is that call's error; the main session's
    /// thread goes on.
    #[test]
    fn a_panic_in_a_call_is_its_error() {
        let mut c = Client::open();
        let e = c
            .query(80, &format!("SELECT 1 {PANIC_MARKER}"))
            .unwrap_err();
        assert!(e.message.contains("panicked"), "{}", e.message);
        assert_eq!(
            c.query(81, "SELECT 5 AS n").unwrap().rows,
            vec![vec![Value::Int(5)]]
        );
        c.send(
            82,
            &Request::ReadOnly {
                sql: format!("SELECT 1 {PANIC_MARKER}"),
                limit: 10,
            },
        );
        assert!(c.rows(82).unwrap_err().message.contains("panicked"));
    }

    /// Read-only calls on a restricted instance: a SELECT answers, a file
    /// read is refused.
    #[test]
    fn read_only_on_a_restricted_instance() {
        let mut c = Client::start();
        c.hello();
        let reply = c.open_with(OpenParams {
            restricted: Some(true),
            ..OpenParams::default()
        });
        assert!(matches!(reply, Reply::Opened), "{reply:?}");
        c.send(
            90,
            &Request::ReadOnly {
                sql: "SELECT 6 AS n".into(),
                limit: 10,
            },
        );
        assert_eq!(c.rows(90).unwrap().rows, vec![vec![Value::Int(6)]]);
        c.send(
            91,
            &Request::ReadOnly {
                sql: "SELECT * FROM read_csv('/etc/hosts')".into(),
                limit: 10,
            },
        );
        let e = c.rows(91).unwrap_err();
        assert!(e.message.contains("disabled"), "{}", e.message);
    }
}
