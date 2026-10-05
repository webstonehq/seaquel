//! Time spent waiting on the secret store, which the terminal binaries'
//! timeouts leave out and the TUI shows (moved from `seaquel-mcp` in phase 7a).
//!
//! On macOS the first read of a keychain item from `seaquel-cli` or
//! `seaquel-tui` can show a prompt ("seaquel-cli wants to use your
//! confidential information…") that waits for the user, and so can a save
//! into an item another binary created (finding it reads it). That wait isn't
//! the database's, so it shouldn't use up a call's timeout:
//! [`SecretWait::watch`] wraps the workspace's secret store, and the MCP
//! server's timeout adds the time any call was pending ([`SecretWait::waited`]).
//! A call pending longer than [`SecretWait::limit`] (5 minutes by default: a
//! prompt nobody answers) ends the MCP call with `TIMEOUT` and a message
//! about the prompt. The TUI watches [`SecretWait::changed`] to say it's
//! waiting for the keychain while [`SecretWait::pending`].
//!
//! Every store call is counted: `get`, `set` and `delete`. The wait is
//! counted per `SecretWait`, not per call: while one call waits on a
//! prompt, every other running call gets the same extension. That only ever
//! makes a timeout later, never earlier.
//!
//! This crate is native only (it links the OS keychain), so it times with
//! the platform clock rather than an executor's.
#![allow(clippy::disallowed_types)]

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::{SecretError, SecretStore};

/// How long one secret read may stay pending by default.
pub const DEFAULT_SECRET_WAIT_LIMIT: Duration = Duration::from_secs(300);

/// Counts the time secret store calls were pending. Share one between the
/// wrapped store ([`SecretWait::watch`]) and whatever times out or shows it.
#[derive(Debug)]
pub struct SecretWait {
    limit: Duration,
    state: Mutex<State>,
    /// Whether a call is pending, for [`SecretWait::changed`]. Sent while
    /// `state` is locked, so its order matches the count's.
    pending_tx: tokio::sync::watch::Sender<bool>,
}

#[derive(Debug, Default)]
struct State {
    /// Reads in progress.
    pending: usize,
    /// When `pending` last went from 0 to 1.
    since: Option<Instant>,
    /// The time some read was pending, up to `since`.
    total: Duration,
}

impl SecretWait {
    /// A `SecretWait` with the default limit.
    pub fn new() -> Arc<Self> {
        Self::with_limit(DEFAULT_SECRET_WAIT_LIMIT)
    }

    /// A `SecretWait` whose reads may stay pending for `limit`.
    pub fn with_limit(limit: Duration) -> Arc<Self> {
        Arc::new(Self {
            limit,
            state: Mutex::default(),
            pending_tx: tokio::sync::watch::Sender::new(false),
        })
    }

    /// How long one read may stay pending before its call gives up.
    pub fn limit(&self) -> Duration {
        self.limit
    }

    /// `store`, with every read counted here.
    pub fn watch(self: &Arc<Self>, store: Arc<dyn SecretStore>) -> Arc<dyn SecretStore> {
        Arc::new(Watched {
            inner: store,
            wait: self.clone(),
        })
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn begin(self: &Arc<Self>) -> Pending {
        let mut state = self.state();
        if state.pending == 0 {
            state.since = Some(Instant::now());
            self.pending_tx.send_replace(true);
        }
        state.pending += 1;
        Pending(self.clone())
    }

    fn end(&self) {
        let mut state = self.state();
        state.pending = state.pending.saturating_sub(1);
        if state.pending == 0 {
            if let Some(since) = state.since.take() {
                state.total += since.elapsed();
            }
            self.pending_tx.send_replace(false);
        }
    }

    /// Whether a store call (`get`, `set` or `delete`) is in progress: a
    /// keychain dialog may be waiting for the user.
    pub fn pending(&self) -> bool {
        self.state().pending > 0
    }

    /// A receiver that changes to `true` when a call starts while none
    /// was pending, and to `false` when the last one ends, so a screen can
    /// redraw.
    pub fn changed(&self) -> tokio::sync::watch::Receiver<bool> {
        self.pending_tx.subscribe()
    }

    /// The total time some call was pending, the current one included.
    pub fn waited(&self) -> Duration {
        let state = self.state();
        state.total + state.since.map_or(Duration::ZERO, |s| s.elapsed())
    }

    /// How long the calls in progress have been pending, if any are.
    pub fn pending_for(&self) -> Option<Duration> {
        self.state().since.map(|s| s.elapsed())
    }
}

/// Ends a call's count when the call finishes or its future is dropped.
struct Pending(Arc<SecretWait>);

impl Drop for Pending {
    fn drop(&mut self) {
        self.0.end();
    }
}

struct Watched {
    inner: Arc<dyn SecretStore>,
    wait: Arc<SecretWait>,
}

#[seaquel_runtime::async_trait]
impl SecretStore for Watched {
    async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let _pending = self.wait.begin();
        self.inner.get(key).await
    }

    // A save or delete into an item another binary created reads it
    // first, which can show the same dialog (phase 7a).
    async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let _pending = self.wait.begin();
        self.inner.set(key, value).await
    }

    async fn delete(&self, key: &str) -> Result<(), SecretError> {
        let _pending = self.wait.begin();
        self.inner.delete(key).await
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::MemoryStore;
    use tokio::sync::Notify;

    #[tokio::test]
    async fn reads_are_counted_and_pass_through() {
        let wait = SecretWait::new();
        let inner = MemoryStore::new();
        inner.set("db:x", "pw").await.unwrap();
        let store = wait.watch(Arc::new(inner));
        assert_eq!(store.get("db:x").await.unwrap().as_deref(), Some("pw"));
        assert_eq!(wait.pending_for(), None);
        let first = wait.waited();

        let pending = wait.begin();
        assert!(wait.pending_for().is_some());
        std::thread::sleep(Duration::from_millis(20));
        assert!(wait.waited() >= first + Duration::from_millis(20));
        drop(pending);
        assert_eq!(wait.pending_for(), None);
        assert!(wait.waited() >= first + Duration::from_millis(20));
    }

    /// A store whose every call waits until released, like a keychain
    /// dialog nobody has answered yet.
    #[derive(Default)]
    struct Blocking {
        inner: MemoryStore,
        entered: Notify,
        release: Notify,
    }

    impl Blocking {
        async fn hold(&self) {
            let released = self.release.notified();
            self.entered.notify_one();
            released.await;
        }
    }

    #[seaquel_runtime::async_trait]
    impl SecretStore for Blocking {
        async fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
            self.hold().await;
            self.inner.get(key).await
        }
        async fn set(&self, key: &str, value: &str) -> Result<(), SecretError> {
            self.hold().await;
            self.inner.set(key, value).await
        }
        async fn delete(&self, key: &str) -> Result<(), SecretError> {
            self.hold().await;
            self.inner.delete(key).await
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Op {
        Get,
        Set,
        Delete,
    }

    #[tokio::test]
    async fn a_blocking_get_set_or_delete_is_pending_and_counted() {
        for op in [Op::Get, Op::Set, Op::Delete] {
            let wait = SecretWait::new();
            let inner = Arc::new(Blocking::default());
            let store = wait.watch(inner.clone());
            let mut changed = wait.changed();
            assert!(!*changed.borrow_and_update(), "{op:?}");
            assert!(!wait.pending());

            let call = tokio::spawn(async move {
                match op {
                    Op::Get => store.get("db:x").await.map(drop),
                    Op::Set => store.set("db:x", "pw").await,
                    Op::Delete => store.delete("db:x").await,
                }
            });
            inner.entered.notified().await;
            assert!(wait.pending(), "{op:?}");
            assert!(wait.pending_for().is_some(), "{op:?}");
            changed.changed().await.unwrap();
            assert!(*changed.borrow_and_update(), "{op:?}: started");
            tokio::time::sleep(Duration::from_millis(20)).await;

            inner.release.notify_one();
            call.await.unwrap().unwrap();
            assert!(!wait.pending(), "{op:?}");
            changed.changed().await.unwrap();
            assert!(!*changed.borrow_and_update(), "{op:?}: ended");
            assert!(wait.waited() >= Duration::from_millis(20), "{op:?}");
        }
    }

    #[tokio::test]
    async fn a_dropped_call_ends_its_count() {
        let wait = SecretWait::new();
        let inner = Arc::new(Blocking::default());
        let store = wait.watch(inner.clone());
        let call = tokio::spawn(async move { store.set("db:x", "pw").await });
        inner.entered.notified().await;
        assert!(wait.pending());
        call.abort();
        let _ = call.await;
        assert!(!wait.pending());
        assert!(!*wait.changed().borrow());
    }
}
