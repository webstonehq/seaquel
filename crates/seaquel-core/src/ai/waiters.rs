//! The turns a workspace runs and what each waits for: an
//! approval or a client tool's answer, keyed by the turn's stream id and
//! the call's id. `ai.respond` reaches only this workspace's turns; a
//! second answer to the same call is ignored; and a turn's waiters go with
//! it, on every ending (its [`TurnGuard`] is dropped with the turn, a
//! cancel and a dropped stream included).

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};

use futures::channel::oneshot;
use seaquel_workspace::ai::AiDecision;

use crate::CoreError;

/// What a waiter accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Approval,
    Client,
}

struct Pending {
    kind: Kind,
    tx: oneshot::Sender<AiDecision>,
}

#[derive(Default)]
struct Turn {
    /// The chat it answers in: one turn per chat at a time.
    chat_id: String,
    pending: HashMap<String, Pending>,
    /// Calls already answered, so a repeated answer is ignored rather than
    /// refused (at most 20 per turn).
    answered: HashSet<String>,
}

/// One workspace's running turns, by stream id.
#[derive(Default)]
pub(crate) struct Waiters {
    turns: Mutex<HashMap<String, Turn>>,
}

/// `NOT_FOUND`: a respond for a stream or call this workspace doesn't have.
pub const NOT_FOUND: &str = "NOT_FOUND";

/// `TURN_IN_PROGRESS`: the chat already has a turn running (409 on web).
pub const TURN_IN_PROGRESS: &str = "TURN_IN_PROGRESS";

fn not_found() -> CoreError {
    CoreError::new(NOT_FOUND, "No turn here is waiting for that answer.")
}

impl Waiters {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Turn>> {
        self.turns.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Registers a turn, unless `cap` turns run already
    /// (`TOO_MANY_REQUESTS`), one runs under this stream id
    /// (`INVALID_ARGUMENT`) or in this chat (`TURN_IN_PROGRESS`).
    pub(crate) fn begin(
        &self,
        stream_id: &str,
        chat_id: &str,
        cap: Option<usize>,
    ) -> Result<TurnGuard<'_>, CoreError> {
        let mut turns = self.lock();
        if cap.is_some_and(|cap| turns.len() >= cap) {
            return Err(CoreError::new(
                super::TOO_MANY_REQUESTS,
                "Too many assistant turns are running here; wait for one to finish.",
            ));
        }
        if turns.contains_key(stream_id) {
            return Err(CoreError::new(
                super::INVALID_ARGUMENT,
                "A turn with this stream id is already running.",
            ));
        }
        if turns.values().any(|t| t.chat_id == chat_id) {
            return Err(CoreError::new(
                TURN_IN_PROGRESS,
                "This chat is already answering; wait for it or stop it.",
            ));
        }
        turns.insert(
            stream_id.to_string(),
            Turn {
                chat_id: chat_id.to_string(),
                ..Turn::default()
            },
        );
        Ok(TurnGuard {
            waiters: self,
            stream_id: stream_id.to_string(),
        })
    }

    /// Waits for `call_id`'s answer. The receiver ends without one when the
    /// turn goes.
    pub(crate) fn wait(
        &self,
        stream_id: &str,
        call_id: &str,
        kind: Kind,
    ) -> oneshot::Receiver<AiDecision> {
        let (tx, rx) = oneshot::channel();
        if let Some(turn) = self.lock().get_mut(stream_id) {
            turn.pending
                .insert(call_id.to_string(), Pending { kind, tx });
        }
        rx
    }

    /// `ai.respond`: hands `decision` to the waiting call. An answer of the
    /// wrong kind (a client result for an approval) is refused and the call
    /// keeps waiting; a call already answered is ignored.
    pub(crate) fn respond(
        &self,
        stream_id: &str,
        call_id: &str,
        decision: AiDecision,
    ) -> Result<(), CoreError> {
        let mut turns = self.lock();
        let turn = turns.get_mut(stream_id).ok_or_else(not_found)?;
        if turn.answered.contains(call_id) {
            return Ok(());
        }
        let Some(pending) = turn.pending.get(call_id) else {
            return Err(not_found());
        };
        let fits = matches!(
            (pending.kind, &decision),
            (Kind::Approval, AiDecision::Approval(_)) | (Kind::Client, AiDecision::Client(_))
        );
        if !fits {
            return Err(CoreError::new(
                super::INVALID_ARGUMENT,
                match pending.kind {
                    Kind::Approval => "This call waits for allow, deny or allowAll.",
                    Kind::Client => "This call waits for the page's result.",
                },
            ));
        }
        let pending = turn.pending.remove(call_id).expect("checked above");
        turn.answered.insert(call_id.to_string());
        // A turn that went meanwhile dropped its receiver: nothing to do.
        let _ = pending.tx.send(decision);
        Ok(())
    }

    /// Calls waiting, in every turn (tests).
    pub(crate) fn pending(&self) -> usize {
        self.lock().values().map(|t| t.pending.len()).sum()
    }

    /// Turns running.
    pub(crate) fn turns(&self) -> usize {
        self.lock().len()
    }
}

/// A running turn's registration: dropping it removes the turn and every
/// call it still waits for.
pub(crate) struct TurnGuard<'a> {
    waiters: &'a Waiters,
    stream_id: String,
}

impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        self.waiters.lock().remove(&self.stream_id);
    }
}
