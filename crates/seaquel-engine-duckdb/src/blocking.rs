//! Runs DuckDB calls off the async runtime.
//!
//! duckdb-rs is synchronous, so every call runs on a `spawn_blocking` thread
//! that holds the connection's mutex for the whole call. Two things make that
//! safe to cancel and to panic in:
//!
//! - **Cancel.** A `spawn_blocking` task keeps running after its future is
//!   dropped. The future side holds a [`Call`]; dropping it before the call
//!   finished interrupts DuckDB (`duckdb_interrupt`) and flags the worker, which
//!   checks the flag between rows. Both only happen while this call holds the
//!   connection, so a late drop can't interrupt the next caller's query, and a
//!   call dropped while still waiting for the mutex never runs.
//!
//!   DuckDB's interrupt is a flag on the connection that
//!   `ClientContext::InitialCleanup` clears at the start of every prepare and
//!   every execute. So an interrupt only stops the statement running (or
//!   being prepared) when it lands; one that lands between prepare and
//!   execute would be forgotten, which is why the driver checks the cancelled
//!   flag after every prepare (`driver::prepare`). A window of a few
//!   microseconds remains between that check and DuckDB's reset. The same
//!   reset makes a late interrupt harmless: one fired after a stream's final
//!   batch was sent, while the worker is still Running (dropping the
//!   statement), can't reach the next call's statement, which clears the
//!   flag when it starts.
//! - **Panics.** duckdb-rs panics on some values (see `driver::read_cell`).
//!   The worker catches them while it still holds the mutex, so it's never
//!   poisoned; a poisoned mutex is recovered anyway. The connection stays
//!   usable: a panic while reading a row leaves DuckDB's state alone, and the
//!   statement is dropped as usual.

use std::any::Any;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use duckdb::{Connection, InterruptHandle};
use seaquel_engine::DbError;

/// Locks `m`, recovering it if a panic poisoned it. Nothing here keeps state
/// across calls that a panic could leave half-written.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The text of a panic payload.
pub(crate) fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// What a call does, for its error code.
#[derive(Clone, Copy)]
pub(crate) enum Op {
    Connect,
    Query,
    Execute,
}

impl Op {
    pub(crate) fn error(self, msg: impl std::fmt::Display) -> DbError {
        match self {
            Op::Connect => DbError::connection_error(msg),
            Op::Query => DbError::query_error(msg),
            Op::Execute => DbError::execute_error(msg),
        }
    }

    /// A blocking task that didn't return: it panicked outside the worker's
    /// guard, or the runtime is shutting down.
    pub(crate) fn join_error(self, e: tokio::task::JoinError) -> DbError {
        if e.is_panic() {
            self.error(format!(
                "DuckDB panicked: {}",
                panic_message(&*e.into_panic())
            ))
        } else {
            self.error("the DuckDB task was cancelled")
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the connection.
    Waiting,
    /// Holds the connection.
    Running,
    /// Finished; the connection may already belong to another call.
    Done,
}

struct State {
    phase: Mutex<Phase>,
    cancelled: AtomicBool,
    interrupt: Arc<InterruptHandle>,
}

/// The future's side of a blocking call. Dropping it before the call is done
/// cancels the call.
pub(crate) struct Call(Arc<State>);

/// The blocking thread's side of a call.
pub(crate) struct Worker(Arc<State>);

pub(crate) fn call(interrupt: Arc<InterruptHandle>) -> (Call, Worker) {
    let state = Arc::new(State {
        phase: Mutex::new(Phase::Waiting),
        cancelled: AtomicBool::new(false),
        interrupt,
    });
    (Call(state.clone()), Worker(state))
}

impl Drop for Call {
    fn drop(&mut self) {
        self.0.cancelled.store(true, Ordering::SeqCst);
        // Held while interrupting: the worker can't finish (and release the
        // connection to someone else) until the interrupt has landed.
        let phase = lock(&self.0.phase);
        if *phase == Phase::Running {
            self.0.interrupt.interrupt();
        }
    }
}

impl Worker {
    /// Whether the future side has gone away. Row loops check it per row.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::SeqCst)
    }

    /// Runs `f` on the connection. A panic in `f` becomes an `op` error. A
    /// call cancelled before it got the connection doesn't run; nobody is
    /// waiting for its result.
    pub(crate) fn run<T>(
        self,
        conn: &Mutex<Connection>,
        op: Op,
        f: impl FnOnce(&Connection, &Worker) -> Result<T, DbError>,
    ) -> Result<T, DbError> {
        let conn = lock(conn);
        {
            let mut phase = lock(&self.0.phase);
            if self.is_cancelled() {
                *phase = Phase::Done;
                return Err(op.error("cancelled"));
            }
            *phase = Phase::Running;
        }
        let out = catch_unwind(AssertUnwindSafe(|| f(&conn, &self))).unwrap_or_else(|payload| {
            Err(op.error(format!("DuckDB panicked: {}", panic_message(&*payload))))
        });
        // Before the connection is released (`conn` drops after this).
        *lock(&self.0.phase) = Phase::Done;
        drop(conn);
        out
    }
}
