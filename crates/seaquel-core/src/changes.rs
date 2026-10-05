//! `StorageChanged` (phase 5d): what a workspace tells
//! its subscribers after every stored write, and the change sequence that
//! orders writes and reads.
//!
//! **The sequence.** Each workspace has a [`ChangeSeq`] whose `epoch` is its
//! random id. A write takes the next `n` while it holds the storage's write
//! lock (right after `Storage::write` returns), so `n` follows commit order;
//! after its commit it completes that number. A read records the
//! *published* `n` before its SELECTs.
//!
//! The published `n` is the highest number below which every number taken
//! has completed (committed, failed or been dropped). So a read that
//! records `n` sees every write numbered `n` or lower, even when writes
//! complete out of order or some writes (the storage group's single
//! statements, history appends) take their number after they committed.
//! The counter is two atomics and a small set of numbers in flight behind a
//! mutex that's never held across an await.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

pub use seaquel_workspace::library::{
    ChangeSeq, Seqd, StoredKind, MAX_EVENT_IDS, MAX_EVENT_IDS_BYTES, MAX_EVENT_ID_BYTES,
};

/// Which window or tab made a write: the desktop's webview
/// label, the web's `X-Seaquel-Origin`. Its `Debug` never shows the value,
/// and it's never logged. A value that isn't 1–64 of `[A-Za-z0-9_-]` is
/// dropped ([`WriteOrigin::new`]), so an event never carries one.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct WriteOrigin(Option<String>);

impl WriteOrigin {
    /// The origin `value`, or none when it isn't 1–64 characters of
    /// `[A-Za-z0-9_-]`.
    pub fn new(value: Option<&str>) -> Self {
        Self(value.filter(|v| is_origin(v)).map(str::to_string))
    }

    /// No origin: Core's own writes.
    pub fn none() -> Self {
        Self(None)
    }

    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// `^[A-Za-z0-9_-]{1,64}$`.
pub fn is_origin(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl fmt::Debug for WriteOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.0.is_some() {
            "WriteOrigin(<set>)"
        } else {
            "WriteOrigin(None)"
        })
    }
}

/// One stored write, announced after its commit. Ids, the kind and the
/// origin only: never a name, a key's value, text or a secret.
#[derive(Clone, PartialEq, Eq)]
pub struct StorageChange {
    pub kind: StoredKind,
    /// The project, connection or chat the ids belong to.
    pub scope: Option<String>,
    /// The rows written. `None`: reload the kind within the scope (also
    /// when more than [`MAX_EVENT_IDS`] rows were written).
    pub ids: Option<Vec<String>>,
    /// The writer's window or tab; `None` for Core's own writes.
    pub origin: Option<String>,
    pub seq: ChangeSeq,
}

impl fmt::Debug for StorageChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageChange")
            .field("kind", &self.kind)
            .field("scope", &self.scope)
            .field("ids", &self.ids)
            .field("origin", &self.origin.as_ref().map(|_| "<set>"))
            .field("seq", &self.seq)
            .finish()
    }
}

/// `ids`, or `None` (a kind reload) past [`MAX_EVENT_IDS`] ids, when one
/// is over [`MAX_EVENT_ID_BYTES`], or when together they're over
/// [`MAX_EVENT_IDS_BYTES`]. The storage group turns caller-chosen keys into
/// ids (`appStateSet`'s key, …), so without the byte bounds one user could
/// make every event, and every socket's queue, as large as a request body
/// (c6).
pub(crate) fn event_ids(ids: Vec<String>) -> Option<Vec<String>> {
    if ids.len() > MAX_EVENT_IDS || ids.iter().any(|id| id.len() > MAX_EVENT_ID_BYTES) {
        return None;
    }
    let total: usize = ids.iter().map(String::len).sum();
    (total <= MAX_EVENT_IDS_BYTES).then_some(ids)
}

/// An event's scope and ids within the size bounds: [`event_ids`], and a
/// scope over [`MAX_EVENT_ID_BYTES`] widens the event to a reload of the
/// kind in every scope (`None`, `None`).
pub(crate) fn event_target(
    scope: Option<String>,
    ids: Option<Vec<String>>,
) -> (Option<String>, Option<Vec<String>>) {
    if scope.as_ref().is_some_and(|s| s.len() > MAX_EVENT_ID_BYTES) {
        return (None, None);
    }
    (scope, ids.and_then(event_ids))
}

/// A workspace's change counter (see the module docs).
pub(crate) struct ChangeCounter {
    epoch: String,
    /// The last number handed out.
    next: AtomicU64,
    /// Every number up to this one has completed.
    published: AtomicU64,
    /// Numbers handed out and not completed yet.
    pending: Mutex<BTreeSet<u64>>,
}

impl ChangeCounter {
    pub(crate) fn new(epoch: String) -> Self {
        Self {
            epoch,
            next: AtomicU64::new(0),
            published: AtomicU64::new(0),
            pending: Mutex::default(),
        }
    }

    fn pending(&self) -> std::sync::MutexGuard<'_, BTreeSet<u64>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The next number, for a write about to commit. Dropping the ticket
    /// without [`SeqTicket::publish`] completes it all the same (the write
    /// failed or was refused), so later numbers can be published.
    pub(crate) fn take(&self) -> SeqTicket<'_> {
        let mut pending = self.pending();
        let n = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        pending.insert(n);
        SeqTicket {
            counter: self,
            n,
            done: false,
        }
    }

    fn complete(&self, n: u64) {
        let mut pending = self.pending();
        pending.remove(&n);
        let safe = match pending.first() {
            Some(&oldest) => oldest - 1,
            None => self.next.load(Ordering::SeqCst),
        };
        self.published.fetch_max(safe, Ordering::SeqCst);
    }

    /// The published sequence: every write numbered up to it has
    /// completed.
    pub(crate) fn published(&self) -> ChangeSeq {
        self.seq(self.published.load(Ordering::SeqCst))
    }

    fn seq(&self, n: u64) -> ChangeSeq {
        ChangeSeq {
            epoch: self.epoch.clone(),
            n,
        }
    }
}

/// A number taken for one write (see [`ChangeCounter::take`]).
pub(crate) struct SeqTicket<'a> {
    counter: &'a ChangeCounter,
    n: u64,
    done: bool,
}

impl SeqTicket<'_> {
    /// The write committed: completes the number and returns it.
    pub(crate) fn publish(mut self) -> ChangeSeq {
        self.done = true;
        self.counter.complete(self.n);
        self.counter.seq(self.n)
    }
}

impl Drop for SeqTicket<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.counter.complete(self.n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_waits_for_every_lower_number() {
        let c = ChangeCounter::new("e".into());
        let a = c.take();
        let b = c.take();
        assert_eq!(b.publish().n, 2);
        // 1 is still in flight: a read can't claim 2 yet.
        assert_eq!(c.published().n, 0);
        drop(a);
        assert_eq!(c.published().n, 2);
        let d = c.take();
        assert_eq!(d.publish().n, 3);
        assert_eq!(c.published().n, 3);
    }

    #[test]
    fn event_ids_past_any_bound_are_a_kind_reload() {
        let ids = |n: usize, len: usize| (0..n).map(|_| "x".repeat(len)).collect::<Vec<_>>();
        // Within every bound: kept.
        assert_eq!(event_ids(ids(3, 10)), Some(ids(3, 10)));
        assert_eq!(
            event_ids(ids(MAX_EVENT_IDS, 100)).map(|v| v.len()),
            Some(MAX_EVENT_IDS)
        );
        // Too many ids.
        assert_eq!(event_ids(ids(MAX_EVENT_IDS + 1, 1)), None);
        // One id over 1 KiB (an 8 MiB `appStateSet` key, the probe's c6).
        assert_eq!(
            event_ids(ids(1, MAX_EVENT_ID_BYTES)).map(|v| v.len()),
            Some(1)
        );
        assert_eq!(event_ids(ids(1, MAX_EVENT_ID_BYTES + 1)), None);
        assert_eq!(event_ids(ids(1, 8 * 1024 * 1024)), None);
        // Each under 1 KiB, together over 16 KiB.
        assert_eq!(
            event_ids(ids(16, MAX_EVENT_IDS_BYTES / 16)).map(|v| v.len()),
            Some(16)
        );
        assert_eq!(event_ids(ids(17, MAX_EVENT_IDS_BYTES / 16)), None);
    }

    #[test]
    fn an_oversized_scope_widens_the_event_to_every_scope() {
        let big = "s".repeat(MAX_EVENT_ID_BYTES + 1);
        assert_eq!(
            event_target(Some(big), Some(vec!["a".into()])),
            (None, None)
        );
        let ok = "s".repeat(MAX_EVENT_ID_BYTES);
        assert_eq!(
            event_target(Some(ok.clone()), Some(vec!["a".into()])),
            (Some(ok), Some(vec!["a".to_string()]))
        );
        assert_eq!(
            event_target(
                Some("p".into()),
                Some(vec!["i".repeat(MAX_EVENT_ID_BYTES + 1)])
            ),
            (Some("p".to_string()), None)
        );
    }

    #[test]
    fn origins_are_checked() {
        assert_eq!(WriteOrigin::new(Some("main")).as_deref(), Some("main"));
        assert_eq!(WriteOrigin::new(Some("a-B_9")).as_deref(), Some("a-B_9"));
        assert_eq!(WriteOrigin::new(Some("")).as_deref(), None);
        assert_eq!(WriteOrigin::new(Some("a\nb")).as_deref(), None);
        assert_eq!(WriteOrigin::new(Some(&"x".repeat(65))).as_deref(), None);
        assert_eq!(
            format!("{:?}", WriteOrigin::new(Some("main"))),
            "WriteOrigin(<set>)"
        );
    }
}
